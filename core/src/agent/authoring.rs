//! Mutable agent authoring. Runtime-specific data stays in the version envelope;
//! publication freezes it atomically with an existing execution artifact.
use super::definition::{self, Actor};
use axum::http::StatusCode;
use magicmtn_contracts::configuration::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;
pub type Failure = (StatusCode, String);
pub type Result<T> = std::result::Result<T, Failure>;
pub fn db(e: sqlx::Error) -> Failure {
    tracing::error!(%e, "agent authoring database error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "Could not update the agent.".into(),
    )
}
pub fn invalid(message: impl Into<String>) -> Failure {
    (StatusCode::UNPROCESSABLE_ENTITY, message.into())
}
pub fn conflict() -> Failure {
    (
        StatusCode::CONFLICT,
        "The draft or published version changed. Reload before saving.".into(),
    )
}
fn definition_error((s, m): definition::Failure) -> Failure {
    (s, m.into())
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Implementation {
    pub kind: String,
    pub adapter: String,
    pub adapter_version: String,
}
impl Default for Implementation {
    fn default() -> Self {
        Self {
            kind: "builtin".into(),
            adapter: "magicmtn-interactive".into(),
            adapter_version: "1".into(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DraftSpec {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub name: String,
    pub description: String,
    pub category: String,
    pub instructions: String,
    pub output_guidance: String,
    pub examples: Vec<String>,
    pub implementation: Implementation,
    pub capabilities: Vec<String>,
    pub model_ids: Vec<Uuid>,
    pub default_model_id: Option<Uuid>,
    pub compute_ids: Vec<Uuid>,
    pub default_compute_id: Option<Uuid>,
}
fn schema_version() -> u32 {
    1
}
impl DraftSpec {
    pub fn normalize(&mut self) {
        self.name = self.name.trim().into();
        self.description = self.description.trim().into();
        self.category = self.category.trim().into();
        self.model_ids.sort();
        self.model_ids.dedup();
        self.compute_ids.sort();
        self.compute_ids.dedup();
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return Err(invalid("Unsupported agent definition version."));
        }
        if self.name.is_empty() || self.name.chars().count() > 120 {
            return Err(invalid("Enter an agent name of at most 120 characters."));
        }
        if self.description.chars().count() > 2000 || self.category.chars().count() > 120 {
            return Err(invalid("Description or category is too long."));
        }
        if self.instructions.len() > 100000
            || self.output_guidance.len() > 5000
            || self.prompt().len() > 100000
        {
            return Err(invalid(
                "Instructions and expected output must total at most 100 KB.",
            ));
        }
        if self.examples.len() > 6 || self.examples.iter().any(|x| x.len() > 2000) {
            return Err(invalid(
                "Use at most six example tasks, each under 2,000 bytes.",
            ));
        }
        if self.implementation != Implementation::default() {
            return Err(invalid("This runtime adapter is not installed. Uploaded SDK packages need a supported adapter."));
        }
        if self.capabilities != vec!["sandbox_commands"] {
            return Err(invalid(
                "The built-in runtime currently supports sandbox commands and file editing.",
            ));
        }
        for (ids, default) in [
            (&self.model_ids, self.default_model_id),
            (&self.compute_ids, self.default_compute_id),
        ] {
            if ids.len() > 100
                || ids.iter().any(Uuid::is_nil)
                || default.is_some_and(|id| !ids.contains(&id))
            {
                return Err(invalid("Choose a default from the allowed resources."));
            }
        }
        Ok(())
    }
    pub fn prompt(&self) -> String {
        if self.output_guidance.trim().is_empty() {
            self.instructions.clone()
        } else {
            format!(
                "{}\n\nExpected output:\n{}",
                self.instructions, self.output_guidance
            )
        }
    }
}
#[derive(Debug, Serialize)]
pub struct Draft {
    pub agent_id: Uuid,
    pub generation: i64,
    pub document: DraftSpec,
    pub base_revision_id: Option<Uuid>,
    pub expected_active_revision_id: Option<Uuid>,
    pub updated_at: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prepare {
    pub expected_generation: i64,
    pub expected_base_revision_id: Option<Uuid>,
    pub expected_active_revision_id: Option<Uuid>,
    pub idempotency_key: String,
    pub publish: bool,
}
#[derive(Clone)]
pub struct Guard {
    pub generation: i64,
    pub document: Value,
}
fn decode(row: sqlx::postgres::PgRow) -> Result<Draft> {
    Ok(Draft {
        agent_id: row.get("agent_id"),
        generation: row.get("generation"),
        document: serde_json::from_value(row.get("document"))
            .map_err(|_| invalid("Stored draft needs an adapter upgrade."))?,
        base_revision_id: row.get("base_revision_id"),
        expected_active_revision_id: row.get("expected_active_revision_id"),
        updated_at: row
            .get::<chrono::DateTime<chrono::Utc>, _>("updated_at")
            .to_rfc3339(),
    })
}
pub async fn defaults(pool: &PgPool) -> Result<DraftSpec> {
    let rows=sqlx::query("SELECT id,model_configuration,capabilities FROM tenant_models WHERE status='enabled' AND task_type='chat' ORDER BY display_name,id").fetch_all(pool).await.map_err(db)?;
    let model=rows.iter().find(|r| crate::model_runtime::compatible(&json!({"model_configuration":r.get::<Value,_>("model_configuration"),"capabilities":r.get::<Value,_>("capabilities")})).is_ok()).map(|r|r.get::<Uuid,_>("id"));
    let compute:Option<Uuid>=sqlx::query_scalar("SELECT id FROM tenant_compute WHERE status='enabled' AND retired_at IS NULL ORDER BY display_name,id LIMIT 1").fetch_optional(pool).await.map_err(db)?;
    Ok(DraftSpec {
        schema_version: 1,
        name: "".into(),
        description: "".into(),
        category: "".into(),
        instructions: "".into(),
        output_guidance: "".into(),
        examples: vec![],
        implementation: Implementation::default(),
        capabilities: vec!["sandbox_commands".into()],
        model_ids: model.into_iter().collect(),
        default_model_id: model,
        compute_ids: compute.into_iter().collect(),
        default_compute_id: compute,
    })
}
pub async fn create(pool: &PgPool, id: Uuid, mut spec: DraftSpec) -> Result<Draft> {
    spec.normalize();
    spec.validate()?;
    let mut tx = pool.begin().await.map_err(db)?;
    sqlx::query(
        "INSERT INTO harnesses(id,name,description,workflow,created_by,owner_user_id) VALUES($1,$2,$3,'interactive',$4,usage_user_from_principal($4))",
    )
    .bind(id)
    .bind(&spec.name)
    .bind(&spec.description)
    .bind(crate::registry_scope::actor_reference("agent-editor"))
    .execute(&mut *tx)
    .await
    .map_err(db)?;
    sqlx::query("INSERT INTO agent_drafts(agent_id,document,updated_by) VALUES($1,$2,$3)")
        .bind(id)
        .bind(json!(spec))
        .bind(crate::registry_scope::actor_reference("agent-editor"))
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    tx.commit().await.map_err(db)?;
    read(pool, id).await
}
pub async fn read(pool: &PgPool, id: Uuid) -> Result<Draft> {
    if let Some(row) = sqlx::query("SELECT * FROM agent_drafts WHERE agent_id=$1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(db)?
    {
        return decode(row);
    }
    // Lazily seed the editor from an existing immutable version; never rewrite it.
    let row=sqlx::query("SELECT h.name,h.description,h.workflow,h.definition_revision_id,d.revision_id AS active,a.manifest FROM harnesses h LEFT JOIN harness_deployments d ON d.id=h.interactive_deployment_id LEFT JOIN artifact_manifests a ON a.revision_id=h.definition_revision_id WHERE h.id=$1").bind(id).fetch_optional(pool).await.map_err(db)?.ok_or((StatusCode::NOT_FOUND,"Agent not found.".into()))?;
    let manifest: Option<Value> = row.get("manifest");
    let a = manifest.unwrap_or(json!({}));
    let spec: Option<Value> = match row.get::<Option<Uuid>, _>("definition_revision_id") {
        Some(r) => {
            sqlx::query_scalar("SELECT document FROM agent_version_specs WHERE revision_id=$1")
                .bind(r)
                .fetch_optional(pool)
                .await
                .map_err(db)?
        }
        None => None,
    };
    let document = if let Some(spec) = spec {
        spec
    } else {
        let mut s = defaults(pool).await?;
        s.name = row.get("name");
        s.description = row
            .get::<Option<String>, _>("description")
            .unwrap_or_default();
        s.category = row
            .get::<Option<String>, _>("workflow")
            .filter(|w| w != "interactive")
            .unwrap_or_default();
        s.instructions = a["system_prompt"].as_str().unwrap_or("").into();
        if let Some(models) = a["model_manifest"].as_array() {
            s.model_ids = models
                .iter()
                .filter_map(|m| serde_json::from_value(m["model_id"].clone()).ok())
                .collect();
            s.default_model_id = serde_json::from_value(a["default_model_id"].clone()).ok();
        }
        if let Some(compute) = a["compute_manifest"].as_array() {
            s.compute_ids = compute
                .iter()
                .filter_map(|m| serde_json::from_value(m["compute_id"].clone()).ok())
                .collect();
            s.default_compute_id = serde_json::from_value(a["default_compute_id"].clone()).ok();
        }
        json!(s)
    };
    sqlx::query("INSERT INTO agent_drafts(agent_id,document,base_revision_id,expected_active_revision_id,updated_by) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING").bind(id).bind(document).bind(row.get::<Option<Uuid>,_>("definition_revision_id")).bind(row.get::<Option<Uuid>,_>("active")).bind(crate::registry_scope::actor_reference("agent-editor")).execute(pool).await.map_err(db)?;
    decode(
        sqlx::query("SELECT * FROM agent_drafts WHERE agent_id=$1")
            .bind(id)
            .fetch_one(pool)
            .await
            .map_err(db)?,
    )
}
pub async fn save(pool: &PgPool, id: Uuid, generation: i64, mut spec: DraftSpec) -> Result<Draft> {
    spec.normalize();
    spec.validate()?;
    let mut tx = pool.begin().await.map_err(db)?;
    sqlx::query("SELECT id FROM harnesses WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
    let row =
        sqlx::query("SELECT document,generation FROM agent_drafts WHERE agent_id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?
            .ok_or((StatusCode::NOT_FOUND, "Draft not found.".into()))?;
    if row.get::<i64, _>("generation") != generation {
        return Err(conflict());
    }
    if row.get::<Value, _>("document") != json!(spec) {
        sqlx::query("UPDATE agent_drafts SET document=$2,generation=generation+1,updated_by=$3,updated_at=now() WHERE agent_id=$1").bind(id).bind(json!(spec)).bind(crate::registry_scope::actor_reference("agent-editor")).execute(&mut *tx).await.map_err(db)?;
        sqlx::query("UPDATE harnesses SET name=$2,description=$3,metadata_generation=metadata_generation+1 WHERE id=$1").bind(id).bind(&spec.name).bind(&spec.description).execute(&mut *tx).await.map_err(db)?;
    }
    tx.commit().await.map_err(db)?;
    read(pool, id).await
}
pub async fn issues(pool: &PgPool, s: &DraftSpec) -> Result<Vec<String>> {
    let mut issues = vec![];
    if s.instructions.trim().is_empty() {
        issues.push("Add instructions describing what this agent should do.".into());
    }
    match s.default_model_id {
        None => issues.push("Choose an available model in Execution settings.".into()),
        Some(id) => {
            let row = sqlx::query(
                "SELECT status,model_configuration,capabilities FROM tenant_models WHERE id=$1",
            )
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(db)?;
            if !row.is_some_and(|r|r.get::<String,_>("status")=="enabled"&&crate::model_runtime::compatible(&json!({"model_configuration":r.get::<Value,_>("model_configuration"),"capabilities":r.get::<Value,_>("capabilities")})).is_ok()) {issues.push("The default model is unavailable or cannot use tools. Choose another model.".into());}
        }
    }
    match s.default_compute_id {
        None => issues.push("Choose a sandbox backend in Execution settings.".into()),
        Some(id) => {
            if !sqlx::query_scalar::<_, bool>(
                "SELECT status='enabled' AND retired_at IS NULL FROM tenant_compute WHERE id=$1",
            )
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(db)?
            .unwrap_or(false)
            {
                issues.push("The sandbox backend is unavailable. Choose another backend.".into());
            }
        }
    }
    Ok(issues)
}
pub async fn prepare(pool: &PgPool, id: Uuid, r: Prepare) -> Result<Value> {
    let draft = read(pool, id).await?;
    if draft.generation != r.expected_generation {
        return Err(conflict());
    }
    let s = &draft.document;
    s.validate()?;
    let issues = issues(pool, s).await?;
    if !issues.is_empty() {
        return Err(invalid(issues.join(" ")));
    }
    let compute = s.default_compute_id.unwrap();
    let configured: Option<String> =
        sqlx::query_scalar("SELECT configuration->>'image' FROM tenant_compute WHERE id=$1")
            .bind(compute)
            .fetch_optional(pool)
            .await
            .map_err(db)?
            .flatten();
    let previous: Option<String> = sqlx::query_scalar("SELECT manifest->'runtime_configuration'->>'sandbox_image' FROM artifact_manifests WHERE revision_id=$1").bind(draft.base_revision_id).fetch_optional(pool).await.map_err(db)?.flatten();
    let image = match configured.or(previous) {
        Some(image) => image,
        None => sqlx::query_scalar("SELECT repository||'@'||digest FROM organization_images WHERE status='enabled' ORDER BY is_default DESC,created_at LIMIT 1").fetch_optional(pool).await.map_err(db)?.ok_or_else(||invalid("Register a sandbox image on Compute → Images before testing."))?,
    };
    let runtime = crate::agent::runtime::interactive_runtime(&image);
    definition::save_authoring(
        pool,
        Actor::Local,
        id,
        SaveRequest {
            expected_base_revision_id: r.expected_base_revision_id,
            expected_active_revision_id: r.expected_active_revision_id,
            changes: Changes {
                model_selection: Some(ModelSelection {
                    model_ids: s.model_ids.clone(),
                    default_model_id: s.default_model_id.unwrap(),
                    refresh_snapshots: false,
                }),
                compute_selection: Some(ComputeSelection {
                    compute_ids: s.compute_ids.clone(),
                    default_compute_id: compute,
                    refresh_snapshots: false,
                }),
                prompt: Some(PromptChange {
                    system_prompt: s.prompt(),
                    library_prompt_id: None,
                    expected_library_content_digest: None,
                    update_library: false,
                }),
                runtime: Some(runtime),
            },
            activation: if r.publish {
                Activation::Required
            } else {
                Activation::Draft
            },
            idempotency_key: r.idempotency_key,
        },
        Guard {
            generation: r.expected_generation,
            document: json!(s),
        },
    )
    .await
    .map_err(definition_error)
}

/// Explicitly retain a user's draft after an external publication changed its base.
pub async fn rebase(pool: &PgPool, id: Uuid, r: Prepare) -> Result<Draft> {
    let mut tx = pool.begin().await.map_err(db)?;
    let row=sqlx::query("SELECT h.definition_revision_id,d.revision_id AS active FROM harnesses h LEFT JOIN harness_deployments d ON d.id=h.interactive_deployment_id WHERE h.id=$1 FOR UPDATE OF h").bind(id).fetch_optional(&mut *tx).await.map_err(db)?.ok_or((StatusCode::NOT_FOUND,"Agent not found.".into()))?;
    if row.get::<Option<Uuid>, _>("definition_revision_id") != r.expected_base_revision_id
        || row.get::<Option<Uuid>, _>("active") != r.expected_active_revision_id
    {
        return Err(conflict());
    }
    let changed=sqlx::query("UPDATE agent_drafts SET base_revision_id=$3,expected_active_revision_id=$4,generation=generation+1,updated_at=now(),updated_by=$5 WHERE agent_id=$1 AND generation=$2").bind(id).bind(r.expected_generation).bind(r.expected_base_revision_id).bind(r.expected_active_revision_id).bind(crate::registry_scope::actor_reference("agent-editor")).execute(&mut *tx).await.map_err(db)?;
    if changed.rows_affected() != 1 {
        return Err(conflict());
    }
    tx.commit().await.map_err(db)?;
    read(pool, id).await
}
