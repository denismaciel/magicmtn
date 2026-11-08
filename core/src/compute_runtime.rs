//! Frozen selection and current scope; no executable path comes from inventory.
use crate::auth::{AuthPolicy, Principal};
use anyhow::{ensure, Result};
use magicmtn_contracts::compute::ActualCompute;
use serde_json::{json, Value};
use uuid::Uuid;

pub fn selection(a: &Value, requested: Option<Uuid>) -> Result<Option<Value>> {
    if a.get("compute_manifest").is_none() {
        ensure!(
            requested.is_none(),
            "legacy revisions do not expose Compute selection"
        );
        return Ok(None);
    }
    crate::contracts::validate_artifact(a)?;
    let id = requested.unwrap_or(serde_json::from_value(a["default_compute_id"].clone())?);
    let e = a["compute_manifest"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["compute_id"] == id.to_string())
        .ok_or_else(|| anyhow::anyhow!("compute outside pinned revision"))?;
    let c = &e["snapshot"]["capabilities"];
    ensure!(
        c["completeness"] != "unknown"
            && ["create", "exec_bounded", "terminate"]
                .iter()
                .all(|op| c["operations"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|s| s == op))),
        "incompatible compute capabilities"
    );
    Ok(Some(e.clone()))
}
pub fn check_runner(
    policy: &AuthPolicy,
    p: &Principal,
    id: Option<Uuid>,
    snapshot: Option<&Value>,
    creator: Option<&str>,
) -> Result<()> {
    if let Some(id) = id {
        let snapshot = snapshot.ok_or_else(|| anyhow::anyhow!("compute pin missing"))?;
        p.require_compute(id)?;
        p.require_binding(
            snapshot["binding_ref"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("binding missing"))?,
        )?;
        if let Some(creator) = creator {
            ensure!(
                policy.permits_compute_ref(creator, id)?,
                "creator compute permission revoked"
            );
        }
    }
    Ok(())
}
pub fn actual(
    id: Uuid,
    snapshot: &Value,
    runner: &str,
    sandbox: Option<Uuid>,
    version: Option<String>,
    image: Option<String>,
) -> Result<Value> {
    let observed = sandbox.is_some() && version.is_some();
    let a = ActualCompute {
        compute_id: id,
        backend: snapshot["backend"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("backend required"))?
            .into(),
        binding_ref: snapshot["binding_ref"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("binding required"))?
            .into(),
        configuration: serde_json::from_value(snapshot["configuration"].clone())?,
        backend_version: version,
        image_identity: image.clone(),
        sandbox_id: sandbox,
        runner_ref: runner.into(),
        completeness: if observed && image.is_some() {
            "complete"
        } else {
            "partial"
        }
        .into(),
        provenance: if observed { "observed" } else { "selected" }.into(),
    };
    a.validate()?;
    Ok(json!(a))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn artifact() -> Value {
        serde_json::from_str::<Value>(include_str!("../../tests/fixtures/compute-vectors.json"))
            .unwrap()["valid"][0]["artifact"]
            .clone()
    }
    #[test]
    fn frozen_selection_and_legacy_are_explicit() {
        let a = artifact();
        let default = selection(&a, None).unwrap().unwrap();
        assert_eq!(default["compute_id"], a["default_compute_id"]);
        let second =
            serde_json::from_value(a["compute_manifest"][1]["compute_id"].clone()).unwrap();
        assert_eq!(
            selection(&a, Some(second)).unwrap().unwrap(),
            a["compute_manifest"][1]
        );
        assert!(selection(&a, Some(Uuid::new_v4())).is_err());
        assert_eq!(
            selection(&json!({"contract_version":2}), None).unwrap(),
            None
        );
        assert!(selection(&json!({"contract_version":2}), Some(second)).is_err());
    }
    #[test]
    fn observed_provenance_is_not_a_configured_tag() {
        let a = artifact();
        let e = &a["compute_manifest"][0];
        let id = serde_json::from_value(e["compute_id"].clone()).unwrap();
        let partial = actual(id, &e["snapshot"], "runner:test", None, None, None).unwrap();
        assert_eq!(partial["completeness"], "partial");
        assert_eq!(partial["provenance"], "selected");
        let observed = actual(
            id,
            &e["snapshot"],
            "runner:test",
            Some(Uuid::new_v4()),
            Some("0.1.0".into()),
            None,
        )
        .unwrap();
        assert_eq!(observed["provenance"], "observed");
        assert_eq!(observed["completeness"], "partial");
        let complete = actual(
            id,
            &e["snapshot"],
            "runner:test",
            Some(Uuid::new_v4()),
            Some("0.1.0".into()),
            Some(format!("sha256:{}", "a".repeat(64))),
        )
        .unwrap();
        assert_eq!(complete["completeness"], "complete");
        assert!(actual(
            id,
            &e["snapshot"],
            "runner:test",
            Some(Uuid::new_v4()),
            Some("0.1.0".into()),
            Some("alpine:latest".into())
        )
        .is_err());
    }
    #[test]
    fn binding_and_current_creator_scope_are_separate() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("policy.json");
        let a = artifact();
        let entry = &a["compute_manifest"][0];
        let id: Uuid = serde_json::from_value(entry["compute_id"].clone()).unwrap();
        let binding = entry["snapshot"]["binding_ref"].as_str().unwrap();
        let mut p:Principal=serde_json::from_value(json!({"principal_ref":"runner","token_sha256":"unused","actions":["compute_use",format!("compute_use:{id}"),format!("compute_binding:{binding}")],"harness_ids":[]})).unwrap();
        let write = |p: &Principal| {
            std::fs::write(
                &path,
                json!({"contract_version":1,"principals":[p]}).to_string(),
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        };
        write(&p);
        let policy = AuthPolicy::new(&path).unwrap();
        assert!(check_runner(
            &policy,
            &p,
            Some(id),
            Some(&entry["snapshot"]),
            Some("runner")
        )
        .is_ok());
        p.actions.retain(|v| !v.starts_with("compute_binding:"));
        assert!(check_runner(&policy, &p, Some(id), Some(&entry["snapshot"]), None).is_err());
        p.actions.push(format!("compute_binding:{binding}"));
        let trusted = p.clone();
        p.actions.retain(|v| v != "compute_use");
        write(&p);
        assert!(check_runner(
            &policy,
            &trusted,
            Some(id),
            Some(&entry["snapshot"]),
            Some("runner")
        )
        .is_err());
    }
}
