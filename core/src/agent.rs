//! Agent identity, immutable versions, publication, and execution attribution.
//!
//! An agent is independent of any model or sandbox. A version describes its
//! implementation and execution defaults; a run records actual resource use.
//! Model routing and compute allocation belong to execution, not agent identity.
//! Legacy `harness_*` persistence and wire fields are retained for compatibility.
//!

pub mod authoring;
pub mod definition;
pub mod execution;
pub mod runtime;

pub use magicmtn_contracts::artifacts::{ModelManifestEntry, PromptProvenance};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

/// Editable local tool catalog. A revision uses its frozen tool_manifest, not this live set.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ToolSet {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
}

/// A single tool that can be made available to an agent.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Tool {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    /// JSON schema for the tool's inputs and capabilities.
    pub schema: Value,
    pub category: Option<String>,
    pub implementation_version: Option<String>,
    pub configuration: Value,
}

/// Junction between a tool set and its tools.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ToolSetTool {
    pub tool_set_id: i64,
    pub tool_id: i64,
}

/// A stable identity with independent ownership and evolution. Identical
/// configurations do not collapse independently managed agents into one ID.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Agent {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub workflow: Option<String>,
    pub definition_revision_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

/// Frozen definition. Only lifecycle status/timestamps may change in Postgres.
/// A parent can belong to another agent when creating an independently managed fork.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AgentVersion {
    pub id: Uuid,
    pub harness_id: Uuid,
    pub name: String,
    pub model: String,
    pub model_configuration: Value,
    /// NULL marks a legacy revision without registered models, never an empty set.
    pub model_manifest: Option<sqlx::types::Json<Vec<ModelManifestEntry>>>,
    pub default_model_id: Option<Uuid>,
    pub system_prompt: Option<String>,
    pub prompt_provenance: Option<sqlx::types::Json<PromptProvenance>>,
    /// Legacy snapshots were captured at migration time, not at historical execution time.
    pub provenance: String,
    /// Local catalog provenance only; never resolve historical tools through this ID.
    pub tool_set_id: Option<i64>,
    pub tool_manifest: sqlx::types::Json<Vec<Value>>,
    pub status: String,
    pub parent_revision_id: Option<Uuid>,
    pub surfaces: Value,
    pub created_at: DateTime<Utc>,
    pub activated_at: Option<DateTime<Utc>>,
    pub retired_at: Option<DateTime<Utc>>,
}

/// Mutable rollout pointer. Persistent behavioral overrides require a new revision.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AgentDeployment {
    pub id: Uuid,
    pub harness_id: Uuid,
    pub revision_id: Uuid,
    pub name: String,
    pub assignment_ref: Option<String>,
    pub created_at: DateTime<Utc>,
}
