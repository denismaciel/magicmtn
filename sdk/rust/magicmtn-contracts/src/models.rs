//! Closed, non-secret tenant inventory contracts. Serving readiness is not inferred.
use anyhow::{ensure, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfiguration {
    pub model: String,
    pub provider: Option<String>,
    pub version: Option<String>,
    pub parameters: Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub completeness: String,
    pub tasks: Vec<String>,
    pub tool_calls: Option<bool>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelFields {
    pub source: String,
    pub source_model_id: String,
    pub display_name: String,
    pub description: String,
    pub task_type: String,
    pub model_configuration: ModelConfiguration,
    pub connection_ref: Option<String>,
    pub capabilities: Capabilities,
    pub supported_catalog_id: Option<Uuid>,
    pub status: String,
}
// Flattening deny_unknown_fields is unsupported by serde. Wire requests are
// decoded as a closed object explicitly before decoding their flattened fields.
#[derive(Debug, Clone, Serialize)]
pub struct CreateModel {
    #[serde(flatten)]
    pub fields: ModelFields,
    pub idempotency_key: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct UpdateModel {
    #[serde(flatten)]
    pub fields: ModelFields,
    pub expected_generation: i64,
    pub idempotency_key: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantModel {
    pub id: Uuid,
    #[serde(flatten)]
    pub fields: ModelFields,
    pub generation: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResult {
    pub model: TenantModel,
    pub outcome: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ModelList {
    pub models: Vec<TenantModel>,
}

fn request(value: Value, update: bool) -> Result<(ModelFields, String, Option<i64>)> {
    let mut object = value
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("object required"))?;
    let key = object
        .remove("idempotency_key")
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or_else(|| anyhow::anyhow!("operation key required"))?;
    ensure!(
        !key.is_empty() && key.len() <= 200 && !key.chars().any(char::is_control),
        "invalid operation key"
    );
    let generation = if update {
        let n = object
            .remove("expected_generation")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow::anyhow!("generation required"))?;
        ensure!(n > 0 && n <= 9_007_199_254_740_991, "invalid generation");
        Some(n)
    } else {
        None
    };
    // Option fields are required on the wire even when their value is null.
    for key in ["connection_ref", "supported_catalog_id"] {
        ensure!(object.contains_key(key), "required field missing");
    }
    let config = object
        .get("model_configuration")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("configuration required"))?;
    ensure!(
        config.contains_key("provider") && config.contains_key("version"),
        "required configuration field missing"
    );
    ensure!(
        object
            .get("capabilities")
            .and_then(Value::as_object)
            .is_some_and(|c| c.contains_key("tool_calls")),
        "tool capability required"
    );
    if let Some(Value::String(id)) = object.get("supported_catalog_id") {
        ensure!(
            Uuid::parse_str(id)?.to_string() == *id,
            "canonical catalog UUID required"
        );
    }
    let mut fields: ModelFields = serde_json::from_value(Value::Object(object))?;
    fields.validate()?;
    fields.capabilities.tasks.sort();
    Ok((fields, key, generation))
}
impl CreateModel {
    pub fn parse(value: Value) -> Result<Self> {
        let (fields, idempotency_key, _) = request(value, false)?;
        Ok(Self {
            fields,
            idempotency_key,
        })
    }
}
impl UpdateModel {
    pub fn parse(value: Value) -> Result<Self> {
        let (fields, idempotency_key, n) = request(value, true)?;
        Ok(Self {
            fields,
            idempotency_key,
            expected_generation: n.unwrap(),
        })
    }
}
fn identifier(s: &str) -> bool {
    !s.trim().is_empty()
        && s.len() <= 512
        && !s.chars().any(char::is_control)
        && !s.contains("://")
        && !s.contains('@')
        && !s.contains('?')
        && !s.contains('#')
}
impl ModelFields {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            [
                &self.source,
                &self.source_model_id,
                &self.display_name,
                &self.task_type,
                &self.model_configuration.model
            ]
            .iter()
            .all(|v| identifier(v)),
            "invalid model identity"
        );
        ensure!(self.description.len() <= 8000, "description too long");
        ensure!(
            ["enabled", "disabled"].contains(&self.status.as_str()),
            "invalid status"
        );
        ensure!(
            self.supported_catalog_id.is_none_or(|id| !id.is_nil()),
            "invalid catalog identity"
        );
        if let Some(r) = &self.connection_ref {
            ensure!(
                r.starts_with("connection:")
                    && r.len() > 11
                    && r.len() <= 200
                    && r[11..]
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b)),
                "opaque connection reference required"
            );
        }
        if let Some(v) = &self.model_configuration.version {
            ensure!(identifier(v), "invalid version");
        }
        let c = &self.capabilities;
        ensure!(
            ["unknown", "partial", "complete"].contains(&c.completeness.as_str()),
            "invalid completeness"
        );
        ensure!(
            c.tasks.len() <= 64
                && c.tasks.iter().all(|t| identifier(t))
                && c.tasks
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    == c.tasks.len(),
            "invalid task set"
        );
        ensure!(
            c.completeness != "unknown" || (c.tasks.is_empty() && c.tool_calls.is_none()),
            "unknown capabilities cannot assert support"
        );
        let params = self
            .model_configuration
            .parameters
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("parameters object required"))?;
        // No recursive arbitrary objects: the closed scalar/stop-list allowlist
        // rejects nested credentials, transport headers and URL configuration.
        let allowed: &[&str] = match self.model_configuration.provider.as_deref() {
            None => &[],
            Some("ollama") => &[
                "temperature",
                "num_ctx",
                "num_predict",
                "top_k",
                "seed",
                "repeat_last_n",
                "stop",
            ],
            Some("openai") => &["temperature", "max_tokens", "seed", "stop"],
            Some(name) => {
                ensure!(
                    !name.is_empty()
                        && name.len() <= 64
                        && name.as_bytes()[0].is_ascii_lowercase()
                        && name
                            .bytes()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'),
                    "invalid provider name"
                );
                &["temperature", "max_tokens", "seed", "stop"]
            }
        };
        for (key, value) in params {
            ensure!(
                allowed.contains(&key.as_str()),
                "unsupported inference parameter"
            );
            if key == "stop" {
                ensure!(
                    value.as_array().is_some_and(|v| v.len() <= 16
                        && v.iter()
                            .all(|s| s.as_str().is_some_and(|s| s.len() <= 200 && !s.is_empty()))),
                    "invalid stop parameter"
                );
            } else {
                let n = value
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("integer parameter required"))?;
                ensure!(n.unsigned_abs() <= 9_007_199_254_740_991, "unsafe integer");
                if key == "temperature" {
                    ensure!((0..=2).contains(&n), "invalid temperature");
                }
                if ["num_ctx", "max_tokens", "top_k"].contains(&key.as_str()) {
                    ensure!(n > 0, "positive limit required");
                }
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn body() -> Value {
        serde_json::json!({"source":"ollama","source_model_id":"qwen3:4b","display_name":"Qwen","description":"","task_type":"chat","model_configuration":{"model":"qwen3:4b","provider":"ollama","version":null,"parameters":{"temperature":0}},"connection_ref":"connection:self-ollama","capabilities":{"completeness":"partial","tasks":["chat"],"tool_calls":true},"supported_catalog_id":null,"status":"enabled","idempotency_key":"create"})
    }
    #[test]
    fn closed_non_secret_requests() {
        let examples: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/harness-models-configuration-requests.example.json"
        ))
        .unwrap();
        assert!(CreateModel::parse(examples["inventory_create"].clone()).is_ok());
        assert!(CreateModel::parse(body()).is_ok());
        for (path, value) in [
            ("token", serde_json::json!("synthetic")),
            ("headers", serde_json::json!({})),
            ("temperature", serde_json::json!({"password":"synthetic"})),
            ("endpoint", serde_json::json!("https://example.invalid")),
        ] {
            let mut b = body();
            b["model_configuration"]["parameters"][path] = value;
            assert!(CreateModel::parse(b).is_err());
        }
        let mut b = body();
        b["credential"] = Value::Null;
        assert!(CreateModel::parse(b).is_err());
        let mut b = body();
        b["connection_ref"] = serde_json::json!("https://example.invalid");
        assert!(CreateModel::parse(b).is_err());
        let mut b = body();
        b["capabilities"]["endpoint"] = Value::Null;
        assert!(CreateModel::parse(b).is_err());
        let mut b = body();
        b.as_object_mut().unwrap().remove("connection_ref");
        assert!(CreateModel::parse(b).is_err());
        assert!(UpdateModel::parse(body()).is_err());
    }
}
