//! Closed non-secret Compute inventory and frozen snapshots.
use anyhow::{ensure, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub completeness: String,
    pub operations: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputeFields {
    pub display_name: String,
    pub description: String,
    pub backend: String,
    pub binding_ref: String,
    pub configuration: Configuration,
    pub capabilities: Capabilities,
    pub status: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct CreateCompute {
    #[serde(flatten)]
    pub fields: ComputeFields,
    pub idempotency_key: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct UpdateCompute {
    #[serde(flatten)]
    pub fields: ComputeFields,
    pub expected_generation: i64,
    pub idempotency_key: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Readiness {
    pub state: String,
    pub checked_at: Option<DateTime<Utc>>,
}
impl Readiness {
    pub fn unknown() -> Self {
        Self {
            state: "unknown".into(),
            checked_at: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantCompute {
    pub id: Uuid,
    #[serde(default = "Readiness::unknown")]
    pub readiness: Readiness,
    #[serde(flatten)]
    pub fields: ComputeFields,
    pub generation: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeResult {
    pub compute: TenantCompute,
    pub outcome: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ComputeList {
    pub compute: Vec<TenantCompute>,
}
impl ComputeFields {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.display_name.trim().is_empty()
                && self.display_name.len() <= 512
                && !self.display_name.chars().any(char::is_control),
            "invalid display name"
        );
        ensure!(
            self.description.len() <= 8000
                && !self.backend.is_empty()
                && self.backend.len() <= 64
                && self.backend.as_bytes()[0].is_ascii_lowercase()
                && self
                    .backend
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
                && ["enabled", "disabled"].contains(&self.status.as_str()),
            "invalid compute fields"
        );
        ensure!(
            self.binding_ref.starts_with("binding:")
                && self.binding_ref.len() > 8
                && self.binding_ref.len() <= 200
                && self.binding_ref[8..]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b)),
            "invalid trusted binding reference"
        );
        if let Some(image) = &self.configuration.image {
            ensure!(
                !image.is_empty()
                    && image.len() <= 512
                    && image
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"/_.:-@".contains(&b))
                    && !image.contains("://"),
                "invalid OCI image"
            );
        }
        let c = &self.capabilities;
        ensure!(
            ["unknown", "partial", "complete"].contains(&c.completeness.as_str())
                && c.operations
                    .iter()
                    .all(|s| ["create", "exec_bounded", "terminate"].contains(&s.as_str()))
                && c.operations
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    == c.operations.len(),
            "invalid capabilities"
        );
        ensure!(
            c.completeness != "unknown" || c.operations.is_empty(),
            "unknown capabilities cannot assert support"
        );
        Ok(())
    }
}
fn request(v: Value, update: bool) -> Result<(ComputeFields, String, Option<i64>)> {
    let mut o = v
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("object required"))?;
    let key = o
        .remove("idempotency_key")
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or_else(|| anyhow::anyhow!("key required"))?;
    ensure!(
        !key.is_empty() && key.len() <= 200 && !key.chars().any(char::is_control),
        "invalid key"
    );
    let generation = if update {
        let n = o
            .remove("expected_generation")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow::anyhow!("generation required"))?;
        ensure!(n > 0 && n <= 9_007_199_254_740_991, "invalid generation");
        Some(n)
    } else {
        None
    };
    let mut f: ComputeFields = serde_json::from_value(Value::Object(o))?;
    f.validate()?;
    ensure!(
        f.configuration.image.is_none(),
        "images belong to harness runtime configuration"
    );
    f.capabilities.operations.sort();
    Ok((f, key, generation))
}
impl CreateCompute {
    pub fn parse(v: Value) -> Result<Self> {
        let (fields, idempotency_key, _) = request(v, false)?;
        Ok(Self {
            fields,
            idempotency_key,
        })
    }
}
impl UpdateCompute {
    pub fn parse(v: Value) -> Result<Self> {
        let (fields, idempotency_key, n) = request(v, true)?;
        Ok(Self {
            fields,
            idempotency_key,
            expected_generation: n.unwrap(),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_inventory_is_a_backend_and_rejects_image_variants() {
        let mut body = serde_json::json!({"display_name":"Apple Container","description":"","backend":"apple-container","binding_ref":"binding:test","configuration":{},"capabilities":{"completeness":"unknown","operations":[]},"status":"enabled","idempotency_key":"backend"});
        assert!(CreateCompute::parse(body.clone()).is_ok());
        body["configuration"] = serde_json::json!({"image":"alpine:3.24.1"});
        assert!(CreateCompute::parse(body.clone()).is_err());
        body["configuration"] = serde_json::json!({});
        body["display_name"] = serde_json::json!("Apple Container Workspace");
        assert!(CreateCompute::parse(body).is_ok());
    }
    #[test]
    fn rejects_secrets_paths_unknown_and_invalid_capabilities() {
        let e: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/compute-examples.json"
        ))
        .unwrap();
        let good = e["registration_create"].clone();
        assert!(CreateCompute::parse(good.clone()).is_ok());
        for (k, v) in [
            ("backend", serde_json::json!("../mock")),
            ("binding_ref", serde_json::json!("/tmp/provider")),
            ("status", serde_json::json!("ready")),
            ("credentials", Value::Null),
        ] {
            let mut b = good.clone();
            b[k] = v;
            assert!(CreateCompute::parse(b).is_err());
        }
        let mut b = good.clone();
        b["configuration"]["mounts"] = serde_json::json!([]);
        assert!(CreateCompute::parse(b).is_err());
        let mut b = good;
        b["capabilities"]["operations"] = serde_json::json!(["create", "create"]);
        assert!(CreateCompute::parse(b).is_err());
    }
}

/// Registration snapshot excludes mutable enabled state and update generation.
pub fn validate_manifest(v: &Value, default: &Value) -> Result<()> {
    let entries = v
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("compute manifest required"))?;
    ensure!(!entries.is_empty(), "compute required");
    let mut ids = std::collections::BTreeSet::new();
    for e in entries {
        ensure!(
            e.as_object().is_some_and(|o| o.len() == 2
                && o.contains_key("compute_id")
                && o.contains_key("snapshot")),
            "closed entry required"
        );
        let raw = e["compute_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("UUID required"))?;
        let id = Uuid::parse_str(raw)?;
        ensure!(
            !id.is_nil() && id.to_string() == raw && ids.insert(id),
            "unique canonical compute IDs required"
        );
        let mut snapshot = e["snapshot"].clone();
        ensure!(
            snapshot.get("status").is_none(),
            "snapshot cannot include mutable status"
        );
        snapshot["status"] = serde_json::json!("enabled");
        let f: ComputeFields = serde_json::from_value(snapshot)?;
        f.validate()?;
    }
    let raw = default
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("default compute required"))?;
    let id = Uuid::parse_str(raw)?;
    ensure!(
        id.to_string() == raw && ids.contains(&id),
        "default compute outside manifest"
    );
    Ok(())
}
/// Frozen v1 model and compute membership; older local artifacts remain readable.
pub fn validate_artifact(v: &Value) -> Result<()> {
    ensure!(
        matches!(v["contract_version"].as_u64(), Some(1 | 3)),
        "unsupported artifact version"
    );
    validate_manifest(&v["compute_manifest"], &v["default_compute_id"])?;
    let mut base = v.clone();
    let o = base
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("artifact required"))?;
    o.remove("compute_manifest");
    o.remove("default_compute_id");
    base["contract_version"] = serde_json::json!(if v["contract_version"] == 3 { 2 } else { 1 });
    base["artifact_digest"] = serde_json::json!(crate::artifacts::artifact_digest(&base)?);
    crate::artifacts::ArtifactV1::parse(&base)?;
    ensure!(
        v["artifact_digest"].as_str() == Some(crate::artifacts::artifact_digest(v)?.as_str()),
        "artifact digest mismatch"
    );
    if v["runtime_version"] == "interactive-v1"
        && v["runtime_configuration"]["state"] == "configured"
    {
        let e = v["compute_manifest"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["compute_id"] == v["default_compute_id"])
            .unwrap();
        ensure!(
            e["snapshot"]["configuration"].get("image").is_none()
                || v["runtime_configuration"]["sandbox_image"]
                    == e["snapshot"]["configuration"]["image"],
            "conflicting image projection"
        );
    }
    Ok(())
}
#[cfg(test)]
mod vectors {
    #[test]
    fn shared_python_vectors() {
        let v: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/compute-vectors.json"
        ))
        .unwrap();
        for e in v["valid"].as_array().unwrap() {
            super::validate_artifact(&e["artifact"]).unwrap();
            crate::contracts::validate_artifact(&e["artifact"]).unwrap();
            assert_eq!(
                crate::artifacts::configuration_digest(&e["artifact"]).unwrap(),
                e["configuration_digest"]
            );
        }
        let mut future = v["valid"][0]["artifact"].clone();
        future["contract_version"] = serde_json::json!(4);
        future["artifact_digest"] =
            serde_json::json!(crate::artifacts::artifact_digest(&future).unwrap());
        assert!(crate::contracts::validate_artifact(&future).is_err());
        // New image-free backend membership retains the harness's runtime image.
        let mut backend = v["valid"][0]["artifact"].clone();
        let member = backend["compute_manifest"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["compute_id"] == backend["default_compute_id"])
            .unwrap()
            .clone();
        backend["compute_manifest"] = serde_json::json!([member]);
        backend["compute_manifest"][0]["snapshot"]["configuration"] = serde_json::json!({});
        backend["artifact_digest"] =
            serde_json::json!(crate::artifacts::artifact_digest(&backend).unwrap());
        super::validate_artifact(&backend).unwrap();
        backend["compute_manifest"] = serde_json::json!([backend["compute_manifest"][0].clone(), {"compute_id":"00000000-0000-4000-8000-000000000099","snapshot":backend["compute_manifest"][0]["snapshot"].clone()}]);
        backend["compute_manifest"][1]["snapshot"]["binding_ref"] =
            serde_json::json!("binding:second-account");
        assert!(super::validate_manifest(
            &backend["compute_manifest"],
            &backend["default_compute_id"]
        )
        .is_ok());
        for e in v["invalid"].as_array().unwrap() {
            assert!(
                super::validate_artifact(&e["artifact"]).is_err(),
                "{}",
                e["name"]
            );
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActualCompute {
    pub compute_id: Uuid,
    pub backend: String,
    pub binding_ref: String,
    pub configuration: Configuration,
    pub backend_version: Option<String>,
    pub image_identity: Option<String>,
    pub sandbox_id: Option<Uuid>,
    pub runner_ref: String,
    pub completeness: String,
    pub provenance: String,
}
impl ActualCompute {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.compute_id.is_nil() && self.sandbox_id.is_none_or(|id| !id.is_nil()),
            "invalid compute identity"
        );
        ComputeFields {
            display_name: "Actual compute".into(),
            description: String::new(),
            backend: self.backend.clone(),
            binding_ref: self.binding_ref.clone(),
            configuration: self.configuration.clone(),
            capabilities: Capabilities {
                completeness: "unknown".into(),
                operations: vec![],
            },
            status: "enabled".into(),
        }
        .validate()?;
        ensure!(
            !self.runner_ref.trim().is_empty()
                && self.runner_ref.len() <= 200
                && !self.runner_ref.chars().any(char::is_control),
            "invalid runner reference"
        );
        ensure!(
            ["unknown", "partial", "complete"].contains(&self.completeness.as_str())
                && ["selected", "observed"].contains(&self.provenance.as_str()),
            "invalid compute provenance"
        );
        if let Some(v) = &self.backend_version {
            ensure!(
                !v.is_empty()
                    && v.len() <= 200
                    && v.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-+".contains(&b)),
                "invalid observed version"
            );
        }
        if let Some(v) = &self.image_identity {
            ensure!(
                v.starts_with("sha256:")
                    && v.len() == 71
                    && v[7..]
                        .bytes()
                        .all(|b| b.is_ascii_digit() || b"abcdef".contains(&b)),
                "invalid observed image digest"
            );
        }
        ensure!(
            self.provenance != "observed"
                || (self.backend_version.is_some() && self.sandbox_id.is_some()),
            "observed backend/sandbox required"
        );
        ensure!(
            self.completeness != "complete"
                || (self.provenance == "observed" && self.image_identity.is_some()),
            "complete image identity required"
        );
        Ok(())
    }
}
