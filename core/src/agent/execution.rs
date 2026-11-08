//! Resolved execution context is separate from the immutable agent version.
//! All identity/connection/resource strings are opaque references, never tokens,
//! passwords, API keys, or authorization grants. Unknown access stays unknown.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{types::Json, FromRow, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolDefinition {
    /// Stable integration-defined key, not a local BIGINT catalog ID.
    pub key: String,
    pub name: String,
    /// None means implementation version was not observed, not "latest".
    pub version: Option<String>,
    pub schema: Value,
    pub configuration: Value,
}

pub use magicmtn_sdk::inference::ModelConfiguration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum Completeness {
    Unknown,
    Partial,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionBinding {
    pub tool_key: String,
    /// Actual downstream actor; may differ from the initiating employee.
    pub principal_ref: Option<String>,
    pub connection_ref: Option<String>,
    pub allowed_actions: Vec<String>,
    pub resource_scope_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationContext {
    pub completeness: Completeness,
    pub bindings: Vec<PermissionBinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessDecision {
    Allowed,
    Denied,
    Unknown,
}

/// Observed decision on a tool operation, not a replacement for access control.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationDecision {
    pub tool_key: String,
    pub action: String,
    pub resource_ref: Option<String>,
    pub principal_ref: Option<String>,
    pub decision: AccessDecision,
    pub reason_code: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Execution {
    pub id: Uuid,
    pub harness_id: Uuid,
    pub harness_revision_id: Uuid,
    pub deployment_id: Option<Uuid>,
    pub integration: String,
    pub external_trace_id: String,
    pub initiator_ref: Option<String>,
    pub workflow: String,
    pub started_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// Append another snapshot when tools, model, or permissions change during a run.
/// A span references its snapshot explicitly, including for out-of-order delivery.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ExecutionContext {
    #[serde(default)]
    pub compute_context: Option<Json<magicmtn_contracts::compute::ActualCompute>>,
    pub id: Uuid,
    pub execution_id: Uuid,
    pub sequence: i32,
    pub effective_at: DateTime<Utc>,
    pub model: Json<ModelConfiguration>,
    pub available_tools: Json<Vec<ToolDefinition>>,
    pub tools_completeness: Completeness,
    pub authorization_context: Json<AuthorizationContext>,
    pub environment_refs: Json<Vec<String>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionIdentity {
    pub harness_id: Uuid,
    pub harness_revision_id: Uuid,
    pub execution_id: Uuid,
    pub execution_context_id: Uuid,
}

impl ExecutionIdentity {
    /// Referential validation only. Authentication and authorization are separate
    /// requirements; knowing these UUIDs never grants access to an execution.
    pub async fn matches_registered_execution(
        &self,
        pool: &PgPool,
        external_trace_id: &str,
        model: &str,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM executions e
                JOIN execution_contexts c ON c.execution_id = e.id
                WHERE e.id = $1 AND e.harness_id = $2 AND e.harness_revision_id = $3
                  AND c.id = $4 AND e.external_trace_id = $5
                  AND ($6 = 'unknown' OR c.model->>'model' = $6)
            )",
        )
        .bind(self.execution_id)
        .bind(self.harness_id)
        .bind(self.harness_revision_id)
        .bind(self.execution_context_id)
        .bind(external_trace_id)
        .bind(model)
        .fetch_one(pool)
        .await
    }
}

/// Register execution attribution and its first context atomically. This is a
/// data-plane library operation, not a public unauthenticated registration API.
pub async fn register_execution(
    pool: &PgPool,
    execution: &Execution,
    initial_context: &ExecutionContext,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        initial_context.execution_id == execution.id,
        "context execution mismatch"
    );
    anyhow::ensure!(
        initial_context.sequence == 0,
        "initial context must have sequence zero"
    );
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO executions (id, harness_id, harness_revision_id, deployment_id,
            integration, external_trace_id, initiator_ref, workflow, started_at, created_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(execution.id)
    .bind(execution.harness_id)
    .bind(execution.harness_revision_id)
    .bind(execution.deployment_id)
    .bind(&execution.integration)
    .bind(&execution.external_trace_id)
    .bind(&execution.initiator_ref)
    .bind(&execution.workflow)
    .bind(execution.started_at)
    .bind(execution.created_at)
    .execute(&mut *tx)
    .await?;
    insert_context(&mut tx, initial_context).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn append_context(pool: &PgPool, context: &ExecutionContext) -> anyhow::Result<()> {
    anyhow::ensure!(
        context.sequence > 0,
        "additional context must have positive sequence"
    );
    let mut tx = pool.begin().await?;
    insert_context(&mut tx, context).await?;
    tx.commit().await?;
    Ok(())
}

async fn insert_context(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    context: &ExecutionContext,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !context.model.model.trim().is_empty(),
        "model must be identified"
    );
    if let Some(c) = &context.compute_context {
        c.validate()?;
    }
    let mut keys = std::collections::HashSet::new();
    for tool in context.available_tools.iter() {
        anyhow::ensure!(
            !tool.key.is_empty() && keys.insert(&tool.key),
            "tool keys must be nonempty and unique"
        );
    }
    sqlx::query(
        "INSERT INTO execution_contexts (id, execution_id, sequence, effective_at, model,
            available_tools, tools_completeness, authorization_context, environment_refs, created_at,compute_context)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(context.id)
    .bind(context.execution_id)
    .bind(context.sequence)
    .bind(context.effective_at)
    .bind(&context.model)
    .bind(&context.available_tools)
    .bind(context.tools_completeness)
    .bind(&context.authorization_context)
    .bind(&context.environment_refs)
    .bind(context.created_at)
    .bind(&context.compute_context)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
