//! Session model selection is separate from the frozen revision's default projection.
use crate::auth::{AuthPolicy, Principal};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use uuid::Uuid;

pub fn selection(artifact: &Value, requested: Option<Uuid>) -> Result<Option<Value>> {
    crate::contracts::validate_artifact(artifact)?;
    if artifact.get("model_manifest").is_none() {
        ensure!(
            requested.is_none(),
            "legacy revision has no registration identities"
        );
        return Ok(None);
    }
    let id = requested.unwrap_or(serde_json::from_value(
        artifact["default_model_id"].clone(),
    )?);
    let entry = artifact["model_manifest"]
        .as_array()
        .context("model manifest missing")?
        .iter()
        .find(|entry| entry["model_id"] == id.to_string())
        .context("model outside pinned revision")?;
    compatible(&entry["snapshot"])?;
    Ok(Some(entry.clone()))
}

/// The interactive runner currently implements local Ollama chat with run_command.
pub fn compatible(snapshot: &Value) -> Result<()> {
    let c = &snapshot["capabilities"];
    let m = &snapshot["model_configuration"];
    ensure!(
        c["completeness"] != "unknown"
            && c["tasks"]
                .as_array()
                .is_some_and(|tasks| tasks.iter().any(|t| t == "chat"))
            && c["tool_calls"] == true,
        "model cannot run chat tools"
    );
    ensure!(
        m["provider"]
            .as_str()
            .is_some_and(magicmtn_provider_framework::host::valid_name),
        "invalid provider name"
    );
    if m["provider"] != "ollama" {
        return Ok(());
    }
    ensure!(
        m["version"].is_null() && m["model"].as_str().is_some_and(|m| !m.ends_with(":cloud")),
        "unsupported model version/transport"
    );
    ensure!(
        m["parameters"].is_null()
            || m["parameters"] == json!({})
            || m["parameters"] == json!({"temperature":0}),
        "unsupported provider parameters"
    );
    Ok(())
}

/// Older sessions derive their selection from their immutable artifact.
pub fn pinned(
    artifact: &Value,
    id: Option<Uuid>,
    snapshot: Option<Value>,
) -> Result<Option<Value>> {
    let entry = selection(artifact, id)?;
    ensure!(
        id.is_some() == snapshot.is_some(),
        "partial session model pin"
    );
    if let Some(snapshot) = snapshot {
        ensure!(
            entry.as_ref().is_some_and(|e| e["snapshot"] == snapshot),
            "session snapshot differs"
        );
    }
    Ok(entry)
}

pub async fn check_current<'e, E: sqlx::Executor<'e, Database = sqlx::Postgres>>(
    executor: E,
    policy: &AuthPolicy,
    principal: &Principal,
    creator: Option<&str>,
    entry: Option<&Value>,
) -> Result<()> {
    if let Some(entry) = entry {
        let id: Uuid = serde_json::from_value(entry["model_id"].clone())?;
        principal.require_model(id)?;
        if let Some(creator) = creator {
            ensure!(
                policy.permits_model_ref(creator, id)?,
                "creator model permission revoked"
            );
        }
        let enabled: bool =
            sqlx::query_scalar("SELECT m.status='enabled' AND (m.organization_id IS NULL OR EXISTS(SELECT 1 FROM organization_providers p WHERE p.organization_id=m.organization_id AND p.provider=m.model_configuration->>'provider' AND p.status='enabled')) FROM tenant_models m WHERE m.id=$1")
                .bind(id)
                .fetch_optional(executor)
                .await?
                .unwrap_or(false);
        ensure!(enabled, "model disabled");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn artifact() -> Value {
        serde_json::from_str::<Value>(include_str!(
            "../../tests/fixtures/harness-models-multimodel-artifacts.example.json"
        ))
        .unwrap()[1]
            .clone()
    }
    #[test]
    fn frozen_membership_capability_and_provider_are_independent() {
        let a = artifact();
        let chosen = selection(&a, None).unwrap().unwrap();
        assert_eq!(pinned(&a, None, None).unwrap(), Some(chosen.clone()));
        assert!(selection(&a, Some(Uuid::new_v4())).is_err());
        let decision = a["model_manifest"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["snapshot"]["task_type"] == "typed_decision")
            .unwrap();
        assert!(selection(
            &a,
            Some(serde_json::from_value(decision["model_id"].clone()).unwrap())
        )
        .is_err());
        let id = serde_json::from_value(chosen["model_id"].clone()).unwrap();
        let mut tampered = chosen["snapshot"].clone();
        tampered["description"] = json!("changed");
        assert!(pinned(&a, Some(id), Some(tampered)).is_err());
        let mut snapshot = chosen["snapshot"].clone();
        snapshot["model_configuration"]["provider"] = json!("unimplemented");
        assert!(compatible(&snapshot).is_ok());
        snapshot["model_configuration"]["provider"] = json!("ollama");
        snapshot["model_configuration"]["model"] = json!("another-local-model");
        assert!(compatible(&snapshot).is_ok());
    }
}
