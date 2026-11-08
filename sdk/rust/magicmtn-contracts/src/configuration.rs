//! Closed revision authoring requests. Nullable keys are explicit in wire bodies.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activation {
    Draft,
    IfAuthorized,
    Required,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRequest {
    pub name: String,
    pub description: Option<String>,
    pub workflow: String,
    pub model_ids: Vec<Uuid>,
    #[serde(default)]
    pub compute_ids: Vec<Uuid>,
    #[serde(default)]
    pub default_compute_id: Option<Uuid>,
    pub default_model_id: Uuid,
    pub system_prompt: String,
    pub library_prompt_id: Option<Uuid>,
    pub runtime_version: String,
    pub tool_manifest: Vec<Value>,
    pub runtime_configuration: Value,
    pub activation: Activation,
    pub expected_active_revision_id: Option<Uuid>,
    pub idempotency_key: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSelection {
    pub model_ids: Vec<Uuid>,
    pub default_model_id: Uuid,
    pub refresh_snapshots: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputeSelection {
    pub compute_ids: Vec<Uuid>,
    pub default_compute_id: Uuid,
    pub refresh_snapshots: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptChange {
    pub system_prompt: String,
    pub library_prompt_id: Option<Uuid>,
    pub expected_library_content_digest: Option<String>,
    pub update_library: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeChange {
    pub runtime_version: String,
    pub tool_manifest: Vec<Value>,
    pub runtime_configuration: Value,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Changes {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compute_selection: Option<ComputeSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<ModelSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<PromptChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeChange>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveRequest {
    pub expected_base_revision_id: Option<Uuid>,
    pub expected_active_revision_id: Option<Uuid>,
    pub changes: Changes,
    pub activation: Activation,
    pub idempotency_key: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationRequest {
    pub expected_base_revision_id: Option<Uuid>,
    pub expected_active_revision_id: Option<Uuid>,
    pub revision_id: Uuid,
    pub idempotency_key: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataRequest {
    pub name: String,
    pub description: Option<String>,
    pub expected_metadata_generation: i64,
    pub idempotency_key: String,
}
fn canonical_ids(v: &Value) -> Result<()> {
    if let Some(object) = v.as_object() {
        for (key, value) in object {
            if key.ends_with("_id") && !value.is_null() {
                let text = value
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("invalid UUID"))?;
                let id = Uuid::parse_str(text)?;
                ensure!(!id.is_nil() && id.to_string() == text, "noncanonical UUID");
            } else if key == "model_ids" || key == "compute_ids" {
                for value in value
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("invalid model set"))?
                {
                    let text = value
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("invalid UUID"))?;
                    let id = Uuid::parse_str(text)?;
                    ensure!(!id.is_nil() && id.to_string() == text, "noncanonical UUID");
                }
            } else if key == "changes"
                || key == "model_selection"
                || key == "compute_selection"
                || key == "prompt"
            {
                canonical_ids(value)?;
            }
        }
    }
    Ok(())
}
fn present(v: &Value, keys: &[&str]) -> Result<()> {
    ensure!(
        v.is_object() && keys.iter().all(|k| v.get(k).is_some()),
        "required keys missing"
    );
    Ok(())
}
pub fn parse_create(v: Value) -> Result<CreateRequest> {
    present(
        &v,
        &[
            "description",
            "library_prompt_id",
            "expected_active_revision_id",
        ],
    )?;
    canonical_ids(&v)?;
    Ok(serde_json::from_value(v)?)
}
pub fn parse_save(v: Value) -> Result<SaveRequest> {
    present(
        &v,
        &["expected_base_revision_id", "expected_active_revision_id"],
    )?;
    if let Some(p) = v.pointer("/changes/prompt") {
        present(p, &["library_prompt_id", "expected_library_content_digest"])?;
    }
    canonical_ids(&v)?;
    Ok(serde_json::from_value(v)?)
}
pub fn parse_activation(v: Value) -> Result<ActivationRequest> {
    present(
        &v,
        &["expected_base_revision_id", "expected_active_revision_id"],
    )?;
    canonical_ids(&v)?;
    Ok(serde_json::from_value(v)?)
}
pub fn parse_metadata(v: Value) -> Result<MetadataRequest> {
    present(&v, &["description"])?;
    canonical_ids(&v)?;
    Ok(serde_json::from_value(v)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn closed_requests_require_explicit_nullable_keys_and_canonical_ids() {
        let body = json!({"expected_base_revision_id":null,"expected_active_revision_id":null,"changes":{},"activation":"draft","idempotency_key":"retry"});
        assert!(parse_save(body.clone()).is_ok());
        let mut missing = body.clone();
        missing
            .as_object_mut()
            .unwrap()
            .remove("expected_active_revision_id");
        assert!(parse_save(missing).is_err());
        let mut unknown = body.clone();
        unknown["actor"] = json!("local-dashboard");
        assert!(parse_save(unknown).is_err());
        let mut invalid = body.clone();
        invalid["expected_base_revision_id"] = json!("ABCD0000-0000-4000-8000-000000000001");
        assert!(parse_save(invalid).is_err());
        let mut nil = body.clone();
        nil["expected_base_revision_id"] = json!(Uuid::nil());
        assert!(parse_save(nil).is_err());
        let mut prompt = body;
        prompt["changes"] = json!({"prompt":{"system_prompt":"exact\r\n", "library_prompt_id":null,"update_library":false}});
        assert!(parse_save(prompt.clone()).is_err());
        prompt["changes"]["prompt"]["expected_library_content_digest"] = Value::Null;
        assert_eq!(
            parse_save(prompt)
                .unwrap()
                .changes
                .prompt
                .unwrap()
                .system_prompt,
            "exact\r\n"
        );
    }
}
