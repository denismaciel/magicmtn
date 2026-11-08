//! Reloadable, deny-by-default service policies. Token material is never returned.
use crate::contracts::CapabilityScope;
use anyhow::{ensure, Context, Result};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use uuid::Uuid;
#[derive(Clone)]
pub struct AuthPolicy {
    path: Option<PathBuf>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Principal {
    pub principal_ref: String,
    pub token_sha256: String,
    pub actions: Vec<String>,
    pub harness_ids: Vec<Uuid>,
    #[serde(default)]
    pub model_ids: Vec<Uuid>,
    #[serde(default)]
    pub profiles: Vec<CapabilityScope>,
    #[serde(default)]
    pub capture_principals: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    contract_version: u32,
    principals: Vec<Principal>,
}
impl AuthPolicy {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let policy = Self {
            path: Some(path.into()),
        };
        policy.read()?;
        Ok(policy)
    }
    /// Shared workspace resource policy. The standalone server separately
    /// enforces account authentication before these service handlers run.
    pub fn local() -> Self {
        Self { path: None }
    }
    pub fn is_local(&self) -> bool {
        self.path.is_none()
    }
    fn read(&self) -> Result<Policy> {
        let path = self
            .path
            .as_ref()
            .context("local policy has no credential file")?;
        let metadata = std::fs::metadata(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                metadata.permissions().mode() & 0o077 == 0,
                "policy file must be private (0600)"
            );
        }
        let policy: Policy = serde_json::from_slice(&std::fs::read(path)?)?;
        ensure!(policy.contract_version == 1, "unsupported policy version");
        ensure!(
            policy
                .principals
                .iter()
                .all(|p| p.principal_ref != "local:magicmtn"),
            "reserved local actor"
        );
        Ok(policy)
    }
    pub fn authenticate(&self, headers: &HeaderMap) -> Result<Principal> {
        if self.is_local() {
            let mut p = Principal::local();
            p.principal_ref = crate::registry_scope::actor_reference("local:magicmtn");
            return Ok(p);
        }
        let token = headers
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .context("bearer token required")?;
        ensure!(token.len() >= 32, "invalid bearer credential");
        let hash = hex::encode(Sha256::digest(token.as_bytes()));
        let matches: Vec<_> = self
            .read()?
            .principals
            .into_iter()
            .filter(|p| constant_eq(hash.as_bytes(), p.token_sha256.as_bytes()))
            .collect();
        ensure!(matches.len() == 1, "invalid or revoked credential");
        Ok(matches[0].clone())
    }
    pub fn permits_profile_ref(&self, reference: &str, scope: &CapabilityScope) -> Result<bool> {
        if self.is_local() {
            return Ok(true);
        }
        Ok(self
            .read()?
            .principals
            .into_iter()
            .any(|p| p.principal_ref == reference && p.permits_profile(scope)))
    }
    pub fn permits_model_ref(&self, reference: &str, id: Uuid) -> Result<bool> {
        if self.is_local() {
            return Ok(true);
        }
        Ok(self
            .read()?
            .principals
            .into_iter()
            .any(|p| p.principal_ref == reference && p.require_model(id).is_ok()))
    }
    pub fn permits_compute_ref(&self, reference: &str, id: Uuid) -> Result<bool> {
        if self.is_local() {
            return Ok(true);
        }
        Ok(self
            .read()?
            .principals
            .into_iter()
            .any(|p| p.principal_ref == reference && p.require_compute(id).is_ok()))
    }
    pub fn has_compute_runner(&self, h: Uuid, id: Uuid, binding: &str) -> Result<bool> {
        if self.is_local() {
            return Ok(true);
        }
        Ok(self.read()?.principals.into_iter().any(|p| {
            p.require("chat_run", h).is_ok()
                && p.require_compute(id).is_ok()
                && p.require_binding(binding).is_ok()
        }))
    }
    /// Recheck a queued or running user's current resource permission without
    /// carrying their bearer material into host-runner work.
    pub fn permits_ref(&self, principal_ref: &str, action: &str, harness: Uuid) -> Result<bool> {
        if self.is_local() {
            return Ok(
                principal_ref == "local:magicmtn" || principal_ref.starts_with("organization:")
            );
        }
        Ok(self
            .read()?
            .principals
            .into_iter()
            .any(|p| p.principal_ref == principal_ref && p.require(action, harness).is_ok()))
    }
}
fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |v, (a, b)| v | (a ^ b)) == 0
}
impl Principal {
    pub fn local() -> Self {
        Self {
            principal_ref: "local:magicmtn".into(),
            token_sha256: String::new(),
            actions: vec!["chat_run".into(), "harness_create".into()],
            harness_ids: vec![],
            model_ids: vec![],
            profiles: vec![],
            capture_principals: vec![],
        }
    }
    fn is_local(&self) -> bool {
        self.principal_ref == "local:magicmtn"
            || self.principal_ref == crate::registry_scope::actor_reference("local:magicmtn")
    }

    /// Inventory permissions are explicit tenant-local actions, independent of harness scope.
    pub fn require_inventory(&self, action: &str) -> Result<()> {
        if self.is_local() {
            return Ok(());
        }
        ensure!(
            [
                "models_read",
                "models_manage",
                "compute_read",
                "compute_manage",
                "images_read"
            ]
            .contains(&action)
                && self.actions.iter().any(|a| a == action),
            "inventory action denied"
        );
        Ok(())
    }
    pub fn require_compute(&self, id: Uuid) -> Result<()> {
        if self.is_local() {
            return Ok(());
        }
        ensure!(
            !id.is_nil()
                && self.actions.iter().any(|a| a == "compute_use")
                && self.actions.contains(&format!("compute_use:{id}")),
            "compute use denied"
        );
        Ok(())
    }
    pub fn require_binding(&self, binding: &str) -> Result<()> {
        if self.is_local() {
            return Ok(());
        }
        ensure!(
            self.actions.contains(&format!("compute_binding:{binding}")),
            "runner binding denied"
        );
        Ok(())
    }
    pub fn require_model(&self, model: Uuid) -> Result<()> {
        if self.is_local() {
            return Ok(());
        }
        ensure!(
            !model.is_nil()
                && self.actions.iter().any(|a| a == "model_use")
                && self.model_ids.contains(&model),
            "model use denied"
        );
        Ok(())
    }
    pub fn require(&self, action: &str, harness: Uuid) -> Result<()> {
        if self.is_local() {
            return Ok(());
        }
        ensure!(
            self.actions.iter().any(|a| a == action) && self.harness_ids.contains(&harness),
            "action/resource denied"
        );
        Ok(())
    }
    pub fn permits_profile(&self, scope: &CapabilityScope) -> bool {
        self.profiles.iter().any(|p| {
            p.principal_ref == scope.principal_ref
                && p.profile_id == scope.profile_id
                && p.completeness == scope.completeness
                && p.allowed_tools == scope.allowed_tools
                && p.resource_scope_refs == scope.resource_scope_refs
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_membership_shortcut() {
        let p = Principal {
            principal_ref: "p".into(),
            token_sha256: "".into(),
            actions: vec!["evaluate".into()],
            harness_ids: vec![Uuid::from_u128(1)],
            model_ids: vec![],
            profiles: vec![],
            capture_principals: vec![],
        };
        assert!(p.require("evaluate", Uuid::from_u128(1)).is_ok());
        assert!(p.require("release", Uuid::from_u128(1)).is_err());
        assert!(p.require("evaluate", Uuid::from_u128(2)).is_err());
    }
}
#[cfg(test)]
mod policy_tests {
    use super::*;

    #[test]
    fn current_scope_is_rechecked_without_a_bearer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        let harness = Uuid::from_u128(1);
        let mut principal = Principal {
            principal_ref: "user".into(),
            token_sha256: "unused".into(),
            actions: vec!["chat_send".into()],
            harness_ids: vec![harness],
            model_ids: vec![],
            profiles: vec![],
            capture_principals: vec![],
        };
        let write = |principal: &Principal| {
            let body = serde_json::json!({"contract_version":1,"principals":[principal]});
            std::fs::write(&path, serde_json::to_vec(&body).unwrap()).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        };
        write(&principal);
        let policy = AuthPolicy::new(&path).unwrap();
        assert!(policy.permits_ref("user", "chat_send", harness).unwrap());
        principal.actions.clear();
        write(&principal);
        assert!(!policy.permits_ref("user", "chat_send", harness).unwrap());
    }
}
