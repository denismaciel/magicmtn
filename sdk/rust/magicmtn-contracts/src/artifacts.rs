//! Frozen v1 definitions. Parsing establishes configuration integrity, not release permission.
use crate::{
    contracts::{canonical_bytes, digest},
    models::{Capabilities, ModelConfiguration},
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSnapshot {
    pub source: String,
    pub source_model_id: String,
    pub display_name: String,
    pub description: String,
    pub task_type: String,
    pub model_configuration: ModelConfiguration,
    pub connection_ref: Option<String>,
    pub capabilities: Capabilities,
    pub provenance: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelManifestEntry {
    pub model_id: Uuid,
    pub snapshot: ModelSnapshot,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptProvenance {
    pub kind: String,
    pub library_prompt_id: Option<Uuid>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactV1 {
    pub contract_version: u32,
    pub canonicalization_version: String,
    pub harness_id: Uuid,
    pub harness_revision_id: Uuid,
    pub workflow: String,
    pub runtime_version: String,
    pub system_prompt: String,
    pub prompt_provenance: PromptProvenance,
    pub model_manifest: Vec<ModelManifestEntry>,
    pub default_model_id: Uuid,
    pub model_configuration: ModelConfiguration,
    pub tool_manifest: Vec<Value>,
    pub runtime_configuration: Value,
    pub artifact_digest: String,
}
fn closed(v: &Value, keys: &str) -> Result<()> {
    let o = v.as_object().context("object required")?;
    let keys: Vec<_> = keys.split_whitespace().collect();
    ensure!(
        o.len() == keys.len() && keys.iter().all(|k| o.contains_key(*k)),
        "object fields differ"
    );
    Ok(())
}
fn text(v: &Value) -> Result<&str> {
    let s = v.as_str().context("string required")?;
    ensure!(!s.trim().is_empty(), "nonblank string required");
    Ok(s)
}
fn uuid(v: &Value) -> Result<Uuid> {
    let raw = v.as_str().context("UUID required")?;
    let id = Uuid::parse_str(raw)?;
    ensure!(
        !id.is_nil() && id.to_string() == raw,
        "canonical nonnil UUID required"
    );
    Ok(id)
}
fn nullable_text(v: &Value) -> Result<()> {
    if !v.is_null() {
        text(v)?;
    }
    Ok(())
}
fn nonsecret(v: &Value) -> Result<()> {
    match v {
        Value::Object(o) => {
            for (key, val) in o {
                let normalized: String = key
                    .to_ascii_lowercase()
                    .chars()
                    .filter(char::is_ascii_alphabetic)
                    .collect();
                ensure!(
                    ![
                        "token",
                        "accesstoken",
                        "apikey",
                        "password",
                        "secret",
                        "authorization",
                        "credentials",
                        "credential",
                        "headers",
                        "endpoint",
                        "url",
                        "baseurl"
                    ]
                    .contains(&normalized.as_str()),
                    "secret/transport parameter forbidden"
                );
                nonsecret(val)?;
            }
        }
        Value::Array(a) => {
            for val in a {
                nonsecret(val)?;
            }
        }
        Value::String(s) => ensure!(
            !s.contains("://"),
            "transport URLs forbidden in inference parameters"
        ),
        _ => (),
    };
    Ok(())
}
pub fn validate_snapshot(v: &Value) -> Result<()> {
    closed(v, "source source_model_id display_name description task_type model_configuration connection_ref capabilities provenance")?;
    for k in ["source", "source_model_id", "display_name", "task_type"] {
        text(&v[k])?;
    }
    ensure!(
        v["description"].is_string()
            && matches!(
                v["provenance"].as_str(),
                Some("registered" | "legacy_migration")
            ),
        "invalid snapshot provenance/description"
    );
    let m = &v["model_configuration"];
    closed(m, "model provider version parameters")?;
    text(&m["model"])?;
    nullable_text(&m["provider"])?;
    nullable_text(&m["version"])?;
    ensure!(m["parameters"].is_object(), "parameters object required");
    nonsecret(&m["parameters"])?;
    if !v["connection_ref"].is_null() {
        let c = text(&v["connection_ref"])?;
        let suffix = c
            .strip_prefix("connection:")
            .context("opaque connection reference required")?;
        ensure!(
            !suffix.is_empty()
                && suffix.len() <= 189
                && suffix
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b)),
            "invalid connection reference"
        );
    }
    let c = &v["capabilities"];
    closed(c, "completeness tasks tool_calls")?;
    ensure!(
        matches!(
            c["completeness"].as_str(),
            Some("unknown" | "partial" | "complete")
        ),
        "invalid capability completeness"
    );
    let tasks = c["tasks"].as_array().context("task array required")?;
    let mut seen = std::collections::BTreeSet::new();
    for task in tasks {
        ensure!(seen.insert(text(task)?), "duplicate task");
    }
    ensure!(
        c["tool_calls"].is_null() || c["tool_calls"].is_boolean(),
        "invalid tool capability"
    );
    ensure!(
        c["completeness"] != "unknown" || (tasks.is_empty() && c["tool_calls"].is_null()),
        "unknown capabilities cannot assert support"
    );
    canonical_bytes(v)?;
    Ok(())
}
/// Normalize set order only. Reject duplicate IDs/tasks rather than hiding them.
pub fn normalize(value: &Value) -> Result<Value> {
    let mut v = value.clone();
    ensure!(v.is_object(), "artifact object required");
    let entries = v["model_manifest"]
        .as_array_mut()
        .context("manifest array required")?;
    ensure!(!entries.is_empty(), "nonempty manifest required");
    let mut ids = std::collections::BTreeSet::new();
    for entry in entries.iter_mut() {
        closed(entry, "model_id snapshot")?;
        ensure!(ids.insert(uuid(&entry["model_id"])?), "duplicate model ID");
        validate_snapshot(&entry["snapshot"])?;
        entry["snapshot"]["capabilities"]["tasks"]
            .as_array_mut()
            .unwrap()
            .sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    }
    entries.sort_by(|a, b| a["model_id"].as_str().cmp(&b["model_id"].as_str()));
    if v.get("compute_manifest").is_some() || v["contract_version"] == 3 {
        crate::compute::validate_manifest(&v["compute_manifest"], &v["default_compute_id"])?;
        let entries = v["compute_manifest"].as_array_mut().unwrap();
        for e in entries.iter_mut() {
            e["snapshot"]["capabilities"]["operations"]
                .as_array_mut()
                .unwrap()
                .sort_by(|a, b| a.as_str().cmp(&b.as_str()));
        }
        entries.sort_by(|a, b| a["compute_id"].as_str().cmp(&b["compute_id"].as_str()));
    }
    Ok(v)
}
pub fn artifact_digest(v: &Value) -> Result<String> {
    let mut body = normalize(v)?;
    body.as_object_mut()
        .context("artifact object required")?
        .remove("artifact_digest");
    digest(&body)
}
pub fn configuration_digest(v: &Value) -> Result<String> {
    let normalized = normalize(v)?;
    let mut body = serde_json::Map::new();
    for k in [
        "workflow",
        "runtime_version",
        "system_prompt",
        "model_manifest",
        "default_model_id",
        "tool_manifest",
        "runtime_configuration",
    ] {
        body.insert(
            k.into(),
            normalized
                .get(k)
                .context("configuration field missing")?
                .clone(),
        );
    }
    if v.get("compute_manifest").is_some() || v["contract_version"] == 3 {
        for k in ["compute_manifest", "default_compute_id"] {
            body.insert(k.into(), normalized[k].clone());
        }
    }
    digest(&Value::Object(body))
}
pub fn seal(v: &Value) -> Result<Value> {
    let mut v = normalize(v)?;
    v["artifact_digest"] = json!(artifact_digest(&v)?);
    crate::contracts::validate_artifact(&v)?;
    Ok(v)
}
impl ArtifactV1 {
    pub fn parse(value: &Value) -> Result<Self> {
        closed(value, "contract_version canonicalization_version harness_id harness_revision_id workflow runtime_version system_prompt prompt_provenance model_manifest default_model_id model_configuration tool_manifest runtime_configuration artifact_digest")?;
        ensure!(
            matches!(value["contract_version"].as_u64(), Some(1 | 2))
                && value["canonicalization_version"] == "ck-json-v1",
            "unsupported artifact version"
        );
        for k in ["harness_id", "harness_revision_id", "default_model_id"] {
            uuid(&value[k])?;
        }
        text(&value["workflow"])?;
        let p = &value["prompt_provenance"];
        closed(p, "kind library_prompt_id")?;
        ensure!(
            matches!(
                p["kind"].as_str(),
                Some("inline" | "library" | "legacy_migration")
            ),
            "invalid prompt provenance"
        );
        if !p["library_prompt_id"].is_null() {
            uuid(&p["library_prompt_id"])?;
        }
        ensure!(
            p["kind"] != "inline" || p["library_prompt_id"].is_null(),
            "inline prompt cannot name library"
        );
        ensure!(
            p["kind"] != "library" || !p["library_prompt_id"].is_null(),
            "library prompt ID required"
        );
        let v = normalize(value)?;
        let selected = v["model_manifest"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["model_id"] == v["default_model_id"])
            .context("default outside manifest")?;
        ensure!(
            v["model_configuration"] == selected["snapshot"]["model_configuration"],
            "default projection differs"
        );
        ensure!(
            value["artifact_digest"].as_str() == Some(artifact_digest(value)?.as_str()),
            "artifact digest mismatch"
        );
        let prompt = text(&v["system_prompt"])?;
        let tools = v["tool_manifest"]
            .as_array()
            .context("tool manifest array required")?;
        let runtime = &v["runtime_configuration"];
        match v["runtime_version"].as_str() {
            Some("unconfigured") => ensure!(
                prompt.chars().count() <= 100000 && *runtime == json!({"state":"unconfigured"}),
                "invalid draft runtime"
            ),
            Some("interactive-v1") => {
                ensure!(
                    v["workflow"] == "interactive" && prompt.len() <= 100000,
                    "invalid interactive workflow/prompt"
                );
                if runtime["state"] == "unconfigured" {
                    closed(runtime, "state")?;
                } else {
                    closed(runtime, "state sandbox_image limits")?;
                    ensure!(
                        runtime["state"] == "configured"
                            && !text(&runtime["sandbox_image"])?.contains(char::is_whitespace),
                        "invalid sandbox runtime"
                    );
                    closed(&runtime["limits"], "max_model_calls max_tool_calls max_turn_seconds command_timeout_ms max_output_bytes max_context_messages")?;
                    for n in runtime["limits"].as_object().unwrap().values() {
                        ensure!(
                            n.as_u64()
                                .is_some_and(|n| n > 0 && n <= 9_007_199_254_740_991),
                            "invalid limit"
                        );
                    }
                    ensure!(tools.len() == 1, "run_command required");
                    closed(&tools[0], "type function")?;
                    ensure!(tools[0]["type"] == "function", "invalid function type");
                    closed(&tools[0]["function"], "name description parameters")?;
                    ensure!(
                        tools[0]["function"]["name"] == "run_command"
                            && tools[0]["function"]["parameters"].is_object(),
                        "unsupported tool"
                    );
                    text(&tools[0]["function"]["description"])?;
                    let snapshot = &selected["snapshot"];
                    ensure!(
                        snapshot["model_configuration"]["provider"]
                            .as_str()
                            .is_some_and(|p| !p.is_empty())
                            && !snapshot["connection_ref"].is_null()
                            && snapshot["capabilities"]["tasks"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .any(|t| t == "chat")
                            && snapshot["capabilities"]["tool_calls"] == true,
                        "incompatible default chat model"
                    );
                }
            }
            _ => anyhow::bail!("unknown runtime"),
        }
        canonical_bytes(&v)?;
        Ok(serde_json::from_value(v)?)
    }
    /// Frozen selection integrity only. Callers must also check current policy/status/readiness.
    pub fn select(
        &self,
        id: Uuid,
        configuration: &ModelConfiguration,
        task: &str,
        tools: bool,
    ) -> Result<&ModelSnapshot> {
        ensure!(
            self.runtime_version != "unconfigured"
                && self.runtime_configuration["state"] == "configured",
            "draft cannot execute"
        );
        let snapshot = &self
            .model_manifest
            .iter()
            .find(|e| e.model_id == id)
            .context("selected model outside manifest")?
            .snapshot;
        ensure!(
            &snapshot.model_configuration == configuration,
            "selected configuration differs from frozen snapshot"
        );
        ensure!(
            snapshot.capabilities.completeness != "unknown"
                && snapshot.capabilities.tasks.iter().any(|t| t == task)
                && (!tools || snapshot.capabilities.tool_calls == Some(true)),
            "unsupported selection capability"
        );
        ensure!(
            snapshot.model_configuration.provider.is_some() && snapshot.connection_ref.is_some(),
            "unknown serving configuration"
        );
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_rust_python_vectors() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/harness_models-vectors.json"
        ))
        .unwrap();
        for v in vectors["valid"].as_array().unwrap() {
            crate::contracts::validate_artifact(&v["artifact"]).unwrap();
            let a =
                ArtifactV1::parse(&v["artifact"]).unwrap_or_else(|e| panic!("{}: {e}", v["name"]));
            assert_eq!(
                configuration_digest(&v["artifact"]).unwrap(),
                v["configuration_digest"],
                "{}",
                v["name"]
            );
            for s in v["selections"].as_array().unwrap() {
                let id = uuid(&s["model_id"]).unwrap();
                let config = &a
                    .model_manifest
                    .iter()
                    .find(|e| e.model_id == id)
                    .unwrap()
                    .snapshot
                    .model_configuration;
                a.select(
                    id,
                    config,
                    s["task"].as_str().unwrap(),
                    s["requires_tools"].as_bool().unwrap(),
                )
                .unwrap();
                let mut wrong = config.clone();
                wrong.model = "other".into();
                assert!(a.select(id, &wrong, "chat", true).is_err());
            }
        }
        for v in vectors["invalid"].as_array().unwrap() {
            assert!(ArtifactV1::parse(&v["artifact"]).is_err(), "{}", v["name"]);
        }
    }
    #[test]
    fn interactive_instructions_use_a_shared_utf8_byte_limit() {
        let examples: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/harness-models-multimodel-artifacts.example.json"
        ))
        .unwrap();
        let mut a = examples[1].clone();
        a["system_prompt"] = Value::String("é".repeat(50_000));
        let a = seal(&a).unwrap();
        assert!(ArtifactV1::parse(&a).is_ok());
        let mut too_large = a.clone();
        too_large["system_prompt"] = Value::String("é".repeat(50_001));
        assert!(seal(&too_large).is_err());
    }
    #[test]
    fn membership_prompt_and_actual_selection() {
        let examples: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/harness-models-multimodel-artifacts.example.json"
        ))
        .unwrap();
        let a = &examples[1];
        let baseline = configuration_digest(a).unwrap();
        let mut changed = a.clone();
        changed["system_prompt"] = json!("Changed\r\n ");
        assert_ne!(baseline, configuration_digest(&changed).unwrap());
        changed = a.clone();
        changed["model_manifest"].as_array_mut().unwrap().remove(0);
        assert_ne!(baseline, configuration_digest(&changed).unwrap());
        let artifact = ArtifactV1::parse(a).unwrap();
        let tev = artifact
            .model_manifest
            .iter()
            .find(|e| e.model_id != artifact.default_model_id)
            .unwrap();
        assert!(artifact
            .select(
                tev.model_id,
                &tev.snapshot.model_configuration,
                "chat",
                true
            )
            .is_err());
        assert!(artifact
            .select(
                tev.model_id,
                &tev.snapshot.model_configuration,
                "typed_decision",
                false
            )
            .is_ok());
        assert!(artifact
            .select(
                Uuid::from_u128(999),
                &tev.snapshot.model_configuration,
                "typed_decision",
                false
            )
            .is_err());
        let draft = ArtifactV1::parse(&examples[0]).unwrap();
        assert!(draft
            .select(
                draft.default_model_id,
                &draft.model_configuration,
                "chat",
                true
            )
            .is_err());
    }
}
