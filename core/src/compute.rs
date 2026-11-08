//! Tenant inventory service. Only these transactions mutate live registrations.
use crate::auth::{AuthPolicy, Principal};
use crate::workspace::State;
use axum::{
    extract::Path,
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use magicmtn_contracts::compute::{
    ComputeFields, ComputeList, ComputeResult, CreateCompute, TenantCompute, UpdateCompute,
};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

#[derive(Clone)]
pub struct ComputeApiState {
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
        "invalid non-secret compute configuration",
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
fn authenticate(s: &ComputeApiState, h: &HeaderMap) -> Result<Principal> {
    s.auth
        .authenticate(h)
        .map_err(|_| (StatusCode::FORBIDDEN, "inventory action denied"))
}
pub fn router<S: Clone + Send + Sync + 'static>(state: ComputeApiState) -> Router<S> {
    Router::new()
        .route("/v1/compute", get(list_http).post(create_http))
        .route("/v1/compute/:id", axum::routing::patch(update_http))
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
async fn list_http(State(s): State<ComputeApiState>, h: HeaderMap) -> Result<Json<ComputeList>> {
    let p = authenticate(&s, &h)?;
    list(&s.pool, &p).await.map(|mut result| {
        for item in &mut result.compute {
            if !crate::public_providers::contains(&item.fields.backend) {
                item.fields.display_name = "MagicMtn Cloud".into();
                item.fields.backend = "magicmtn".into();
                item.fields.binding_ref = format!("binding:{}", item.id);
                item.fields.description.clear();
                item.fields.configuration =
                    magicmtn_contracts::compute::Configuration { image: None };
            }
        }
        Json(result)
    })
}
async fn create_http(
    State(s): State<ComputeApiState>,
    h: HeaderMap,
    Json(b): Json<Value>,
) -> Result<Json<ComputeResult>> {
    let p = authenticate(&s, &h)?;
    authorize(&p, "compute_manage")?;
    let r = CreateCompute::parse(b).map_err(invalid)?;
    if !crate::public_providers::contains(&r.fields.backend) {
        return Err((StatusCode::NOT_FOUND, "provider not found in the Registry"));
    }
    create(&s.pool, &p, r).await.map(Json)
}
async fn update_http(
    State(s): State<ComputeApiState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<Value>,
) -> Result<Json<ComputeResult>> {
    let p = authenticate(&s, &h)?;
    authorize(&p, "compute_manage")?;
    let r = UpdateCompute::parse(b).map_err(invalid)?;
    let backend: Option<String> =
        sqlx::query_scalar("SELECT backend FROM tenant_compute WHERE id=$1")
            .bind(id)
            .fetch_optional(&s.pool)
            .await
            .map_err(internal)?;
    if !backend
        .as_deref()
        .is_some_and(crate::public_providers::contains)
        || !crate::public_providers::contains(&r.fields.backend)
    {
        return Err((StatusCode::NOT_FOUND, "provider not found in the Registry"));
    }
    update(&s.pool, &p, id, r).await.map(Json)
}
fn decode(row: sqlx::postgres::PgRow) -> Result<TenantCompute> {
    let fields = ComputeFields {
        display_name: row.get("display_name"),
        description: row.get("description"),
        backend: row.get("backend"),
        binding_ref: row.get("binding_ref"),
        configuration: serde_json::from_value(row.get("configuration")).map_err(internal)?,
        capabilities: serde_json::from_value(row.get("capabilities")).map_err(internal)?,
        status: row.get("status"),
    };
    // Fail closed rather than exposing historical out-of-contract serving data.
    fields.validate().map_err(internal)?;
    Ok(TenantCompute {
        readiness: magicmtn_contracts::compute::Readiness {
            state: "unknown".into(),
            checked_at: None,
        },
        id: row.get("id"),
        fields,
        generation: row.get("generation"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}
pub async fn list(pool: &PgPool, p: &Principal) -> Result<ComputeList> {
    authorize(p, "compute_read")?;
    let rows = sqlx::query(
        "SELECT * FROM tenant_compute WHERE retired_at IS NULL ORDER BY display_name,id",
    )
    .fetch_all(pool)
    .await
    .map_err(internal)?;
    Ok(ComputeList {
        compute: rows.into_iter().map(decode).collect::<Result<_>>()?,
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
) -> Result<Option<ComputeResult>> {
    let digest = crate::contracts::digest(body).map_err(invalid)?;
    sqlx::query("INSERT INTO compute_operations(principal_ref,operation,resource_key,idempotency_key,request_digest) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
        .bind(&p.principal_ref).bind(operation).bind(resource).bind(key).bind(&digest).execute(&mut **tx).await.map_err(internal)?;
    let row=sqlx::query("SELECT request_digest,response FROM compute_operations WHERE principal_ref=$1 AND operation=$2 AND resource_key=$3 AND idempotency_key=$4 FOR UPDATE")
        .bind(&p.principal_ref).bind(operation).bind(resource).bind(key).fetch_one(&mut **tx).await.map_err(internal)?;
    if row.get::<String, _>("request_digest") != digest {
        return Err(conflict());
    }
    let stored: Option<Value> = row.get("response");
    stored
        .map(|v| {
            let mut r: ComputeResult = serde_json::from_value(v).map_err(internal)?;
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
    r: ComputeResult,
) -> Result<ComputeResult> {
    sqlx::query("UPDATE compute_operations SET response=$5 WHERE principal_ref=$1 AND operation=$2 AND resource_key=$3 AND idempotency_key=$4")
        .bind(&p.principal_ref).bind(operation).bind(resource).bind(key).bind(serde_json::to_value(&r).map_err(internal)?).execute(&mut *tx).await.map_err(internal)?;
    tx.commit().await.map_err(internal)?;
    Ok(r)
}
pub async fn create(pool: &PgPool, p: &Principal, r: CreateCompute) -> Result<ComputeResult> {
    authorize(p, "compute_manage")?;
    // Reparse to validate even library callers and canonicalize capability sets.
    let r = CreateCompute::parse(serde_json::to_value(r).map_err(invalid)?).map_err(invalid)?;
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
    let row=sqlx::query("INSERT INTO tenant_compute(id,display_name,description,backend,binding_ref,configuration,capabilities,status) VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING *")
        .bind(Uuid::new_v4()).bind(&f.display_name).bind(&f.description).bind(&f.backend).bind(&f.binding_ref).bind(json!(f.configuration)).bind(json!(f.capabilities)).bind(&f.status).fetch_one(&mut *tx).await.map_err(|e| if e.as_database_error().is_some_and(|d| d.is_unique_violation()) { (StatusCode::CONFLICT, "backend already registered") } else { internal(e) })?;
    finish(
        tx,
        p,
        "create",
        "create",
        &r.idempotency_key,
        ComputeResult {
            compute: decode(row)?,
            outcome: "created".into(),
        },
    )
    .await
}
pub async fn update(
    pool: &PgPool,
    p: &Principal,
    id: Uuid,
    r: UpdateCompute,
) -> Result<ComputeResult> {
    authorize(p, "compute_manage")?;
    let r = UpdateCompute::parse(serde_json::to_value(r).map_err(invalid)?).map_err(invalid)?;
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
    let row =
        sqlx::query("SELECT * FROM tenant_compute WHERE id=$1 AND retired_at IS NULL FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal)?
            .ok_or((StatusCode::NOT_FOUND, "compute registration not found"))?;
    let current = decode(row)?;
    if current.generation != r.expected_generation {
        return Err(conflict());
    }
    let mut comparable = current.fields.clone();
    comparable.capabilities.operations.sort();
    let result = if comparable == r.fields {
        ComputeResult {
            compute: current,
            outcome: "unchanged".into(),
        }
    } else {
        let f = &r.fields;
        let row=sqlx::query("UPDATE tenant_compute SET display_name=$2,description=$3,backend=$4,binding_ref=$5,configuration=$6,capabilities=$7,status=$8,generation=generation+1,updated_at=now() WHERE id=$1 RETURNING *")
            .bind(id).bind(&f.display_name).bind(&f.description).bind(&f.backend).bind(&f.binding_ref).bind(json!(f.configuration)).bind(json!(f.capabilities)).bind(&f.status).fetch_one(&mut *tx).await.map_err(internal)?;
        ComputeResult {
            compute: decode(row)?,
            outcome: "updated".into(),
        }
    };
    finish(tx, p, "update", &resource, &r.idempotency_key, result).await
}

impl crate::workspace::WorkspaceState for ComputeApiState {
    fn with_pool(mut self, pool: PgPool) -> Self {
        self.pool = pool;
        self
    }
}
