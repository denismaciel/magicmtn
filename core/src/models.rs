//! Tenant inventory service. Only these transactions mutate live registrations.
use crate::auth::{AuthPolicy, Principal};
use crate::workspace::State;
use axum::{
    extract::Path,
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use magicmtn_contracts::models::{
    CreateModel, ModelFields, ModelList, ModelResult, TenantModel, UpdateModel,
};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

#[derive(Clone)]
pub struct ModelApiState {
    pub pool: PgPool,
    pub auth: AuthPolicy,
}

type Failure = (StatusCode, &'static str);
type Result<T> = std::result::Result<T, Failure>;
fn internal(_: impl std::fmt::Display) -> Failure {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "inventory operation failed",
    )
}
fn invalid(_: impl std::fmt::Display) -> Failure {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid non-secret model configuration",
    )
}
fn conflict() -> Failure {
    (
        StatusCode::CONFLICT,
        "generation or operation key conflicts",
    )
}
fn authorize(p: &Principal, action: &str) -> Result<()> {
    p.require_inventory(action)
        .map_err(|_| (StatusCode::FORBIDDEN, "inventory action denied"))
}
fn authenticate(s: &ModelApiState, h: &HeaderMap) -> Result<Principal> {
    s.auth
        .authenticate(h)
        .map_err(|_| (StatusCode::FORBIDDEN, "inventory action denied"))
}
pub fn router<S: Clone + Send + Sync + 'static>(state: ModelApiState) -> Router<S> {
    Router::new()
        .route("/v1/models", get(list_http))
        .route("/v1/models/:id", axum::routing::patch(update_http))
        .layer(axum::extract::DefaultBodyLimit::max(65536))
        .layer(axum::middleware::map_response(
            |mut response: axum::response::Response| async move {
                response.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
                );
                response.headers_mut().insert(
                    axum::http::header::VARY,
                    axum::http::HeaderValue::from_static("Authorization"),
                );
                response
            },
        ))
        .with_state(state)
}
async fn list_http(State(s): State<ModelApiState>, h: HeaderMap) -> Result<Json<ModelList>> {
    let p = authenticate(&s, &h)?;
    list(&s.pool, &p)
        .await
        .map(crate::public_providers::models)
        .map(Json)
}
async fn update_http(
    State(s): State<ModelApiState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<Value>,
) -> Result<Json<ModelResult>> {
    let p = authenticate(&s, &h)?;
    authorize(&p, "models_manage")?;
    let r = UpdateModel::parse(b).map_err(invalid)?;
    update(&s.pool, &p, id, r)
        .await
        .map(crate::public_providers::model_result)
        .map(Json)
}
fn decode(row: sqlx::postgres::PgRow) -> Result<TenantModel> {
    let config: Value = row.get("model_configuration");
    let caps: Value = row.get("capabilities");
    let fields = ModelFields {
        source: row.get("source"),
        source_model_id: row.get("source_model_id"),
        display_name: row.get("display_name"),
        description: row.get("description"),
        task_type: row.get("task_type"),
        model_configuration: serde_json::from_value(config).map_err(internal)?,
        connection_ref: row.get("connection_ref"),
        capabilities: serde_json::from_value(caps).map_err(internal)?,
        supported_catalog_id: row.get("supported_catalog_id"),
        status: row.get("status"),
    };
    // Fail closed rather than exposing historical out-of-contract serving data.
    fields.validate().map_err(internal)?;
    Ok(TenantModel {
        id: row.get("id"),
        fields,
        generation: row.get("generation"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}
pub async fn list(pool: &PgPool, p: &Principal) -> Result<ModelList> {
    authorize(p, "models_read")?;
    let rows = sqlx::query("SELECT * FROM tenant_models ORDER BY display_name,id")
        .fetch_all(pool)
        .await
        .map_err(internal)?;
    Ok(ModelList {
        models: rows.into_iter().map(decode).collect::<Result<_>>()?,
    })
}
// The unique receipt row serializes identical keys, including concurrent creates.
// Reservation and mutation commit together; failures leave no durable key.
async fn replay(
    tx: &mut Transaction<'_, Postgres>,
    p: &Principal,
    operation: &str,
    resource: &str,
    key: &str,
    body: &Value,
) -> Result<Option<ModelResult>> {
    let digest = crate::contracts::digest(body).map_err(invalid)?;
    sqlx::query("INSERT INTO model_operations(principal_ref,operation,resource_key,idempotency_key,request_digest) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
        .bind(&p.principal_ref).bind(operation).bind(resource).bind(key).bind(&digest).execute(&mut **tx).await.map_err(internal)?;
    let row=sqlx::query("SELECT request_digest,response FROM model_operations WHERE principal_ref=$1 AND operation=$2 AND resource_key=$3 AND idempotency_key=$4 FOR UPDATE")
        .bind(&p.principal_ref).bind(operation).bind(resource).bind(key).fetch_one(&mut **tx).await.map_err(internal)?;
    if row.get::<String, _>("request_digest") != digest {
        return Err(conflict());
    }
    let stored: Option<Value> = row.get("response");
    stored
        .map(|v| {
            let mut r: ModelResult = serde_json::from_value(v).map_err(internal)?;
            r.outcome = "replayed".into();
            Ok(r)
        })
        .transpose()
}
async fn finish(
    mut tx: Transaction<'_, Postgres>,
    p: &Principal,
    operation: &str,
    resource: &str,
    key: &str,
    r: ModelResult,
) -> Result<ModelResult> {
    sqlx::query("UPDATE model_operations SET response=$5 WHERE principal_ref=$1 AND operation=$2 AND resource_key=$3 AND idempotency_key=$4")
        .bind(&p.principal_ref).bind(operation).bind(resource).bind(key).bind(serde_json::to_value(&r).map_err(internal)?).execute(&mut *tx).await.map_err(internal)?;
    tx.commit().await.map_err(internal)?;
    Ok(r)
}
pub async fn create(pool: &PgPool, p: &Principal, r: CreateModel) -> Result<ModelResult> {
    authorize(p, "models_manage")?;
    // Reparse to validate even library callers and canonicalize capability sets.
    let r = CreateModel::parse(serde_json::to_value(r).map_err(invalid)?).map_err(invalid)?;
    if crate::registry_scope::organization_id().is_some() {
        return Err((
            StatusCode::METHOD_NOT_ALLOWED,
            "models are registered through provider addition",
        ));
    }
    let mut tx = pool.begin().await.map_err(internal)?;
    if let Some(saved) = replay(
        &mut tx,
        p,
        "create",
        "create",
        &r.idempotency_key,
        &json!(r),
    )
    .await?
    {
        tx.commit().await.map_err(internal)?;
        return Ok(saved);
    }
    let f = &r.fields;
    let row=sqlx::query("INSERT INTO tenant_models(id,source,source_model_id,display_name,description,task_type,model_configuration,connection_ref,capabilities,supported_catalog_id,status) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING *")
        .bind(Uuid::new_v4()).bind(&f.source).bind(&f.source_model_id).bind(&f.display_name).bind(&f.description).bind(&f.task_type)
        .bind(json!(f.model_configuration)).bind(&f.connection_ref).bind(json!(f.capabilities)).bind(f.supported_catalog_id).bind(&f.status)
        .fetch_one(&mut *tx).await.map_err(internal)?;
    finish(
        tx,
        p,
        "create",
        "create",
        &r.idempotency_key,
        ModelResult {
            model: decode(row)?,
            outcome: "created".into(),
        },
    )
    .await
}
pub async fn update(pool: &PgPool, p: &Principal, id: Uuid, r: UpdateModel) -> Result<ModelResult> {
    authorize(p, "models_manage")?;
    let mut r = UpdateModel::parse(serde_json::to_value(r).map_err(invalid)?).map_err(invalid)?;
    let resource = id.to_string();
    let mut tx = pool.begin().await.map_err(internal)?;
    if let Some(saved) = replay(
        &mut tx,
        p,
        "update",
        &resource,
        &r.idempotency_key,
        &json!(r),
    )
    .await?
    {
        tx.commit().await.map_err(internal)?;
        return Ok(saved);
    }
    let row = sqlx::query("SELECT * FROM tenant_models WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "model registration not found"))?;
    let current = decode(row)?;
    // Accept the customer projection for availability edits, but persist the
    // canonical serving identity. Never route using the presentation alias.
    let mut public = current.fields.clone();
    crate::public_providers::model_fields(id, &mut public);
    public.status = r.fields.status.clone();
    if r.fields == public {
        let status = r.fields.status.clone();
        r.fields = current.fields.clone();
        r.fields.status = status;
    }
    if crate::registry_scope::organization_id().is_some() {
        let mut selection = r.fields.clone();
        selection.status = current.fields.status.clone();
        if selection != current.fields {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                "only availability can be changed for Library model offerings",
            ));
        }
    }
    if current.generation != r.expected_generation {
        return Err(conflict());
    }
    if let Some(org) = crate::registry_scope::organization_id() {
        if r.fields.status == "enabled" {
            let registered: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM organization_providers WHERE organization_id=$1 AND provider=$2 AND status='enabled')")
                .bind(org).bind(&r.fields.model_configuration.provider).fetch_one(&mut *tx).await.map_err(internal)?;
            if !registered {
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "add the provider through the CLI before making this model available",
                ));
            }
        }
    }
    let mut comparable = current.fields.clone();
    comparable.capabilities.tasks.sort();
    let result = if comparable == r.fields {
        ModelResult {
            model: current,
            outcome: "unchanged".into(),
        }
    } else {
        let f = &r.fields;
        let row=sqlx::query("UPDATE tenant_models SET source=$2,source_model_id=$3,display_name=$4,description=$5,task_type=$6,model_configuration=$7,connection_ref=$8,capabilities=$9,supported_catalog_id=$10,status=$11,generation=generation+1,updated_at=now() WHERE id=$1 RETURNING *")
            .bind(id).bind(&f.source).bind(&f.source_model_id).bind(&f.display_name).bind(&f.description).bind(&f.task_type)
            .bind(json!(f.model_configuration)).bind(&f.connection_ref).bind(json!(f.capabilities)).bind(f.supported_catalog_id).bind(&f.status)
            .fetch_one(&mut *tx).await.map_err(internal)?;
        ModelResult {
            model: decode(row)?,
            outcome: "updated".into(),
        }
    };
    finish(tx, p, "update", &resource, &r.idempotency_key, result).await
}

impl crate::workspace::WorkspaceState for ModelApiState {
    fn with_pool(mut self, pool: PgPool) -> Self {
        self.pool = pool;
        self
    }
}
