//! Canonical frozen configuration integrity and capability scopes.
use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn canonical_bytes(value: &Value) -> Result<Vec<u8>> {
    fn check(v: &Value) -> Result<()> {
        match v {
            Value::Number(n) => ensure!(
                n.as_i64()
                    .is_some_and(|x| x.unsigned_abs() <= 9_007_199_254_740_991),
                "canonical numbers must be safe integers"
            ),
            Value::Array(a) => {
                for v in a {
                    check(v)?;
                }
            }
            Value::Object(o) => {
                for (k, v) in o {
                    ensure!(k.is_ascii(), "canonical keys must be ASCII");
                    check(v)?;
                }
            }
            _ => (),
        }
        Ok(())
    }
    check(value)?;
    Ok(serde_json::to_vec(value)?)
}
pub fn digest(value: &Value) -> Result<String> {
    Ok(hex::encode(Sha256::digest(canonical_bytes(value)?)))
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityScope {
    pub principal_ref: String,
    pub profile_id: String,
    pub completeness: String,
    pub allowed_tools: Vec<String>,
    pub resource_scope_refs: Vec<String>,
}
pub fn validate_artifact(value: &Value) -> Result<()> {
    match value["contract_version"].as_u64() {
        Some(1) if value.get("compute_manifest").is_some() => {
            crate::compute::validate_artifact(value)
        }
        Some(3) => crate::compute::validate_artifact(value),
        Some(1 | 2) => crate::artifacts::ArtifactV1::parse(value).map(|_| ()),
        _ => bail!("unsupported artifact version"),
    }
}
