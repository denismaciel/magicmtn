//! Versioned fixture catalog, independent of organization registrations.
use magicmtn_contracts::models::ModelFields;
use serde_json::{json, Value};

pub fn catalog() -> Value {
    serde_json::from_str(include_str!("../data/library.json")).expect("valid bundled Library")
}
pub fn default_model(provider: &str) -> anyhow::Result<ModelFields> {
    anyhow::ensure!(
        magicmtn_provider_framework::host::valid_name(provider),
        "invalid provider"
    );
    let catalog = catalog();
    let entry = &catalog["inference_providers"][provider];
    let default = &catalog["default_inference_model"];
    let model = entry["model"]
        .as_str()
        .unwrap_or(default["model"].as_str().unwrap());
    let display = entry["model_display_name"]
        .as_str()
        .unwrap_or(default["display_name"].as_str().unwrap());
    let fields: ModelFields = serde_json::from_value(json!({
        "source":provider,"source_model_id":model,"display_name":display,
        "description":"Default Library offering; provider discovery is deferred.","task_type":"chat",
        "model_configuration":{"model":model,"provider":provider,"version":null,"parameters":entry.get("parameters").cloned().unwrap_or(json!({"temperature":0}))},
        "connection_ref":format!("connection:self-{provider}"),
        "capabilities":{"completeness":"partial","tasks":["chat"],"tool_calls":true},
        "supported_catalog_id":null,"status":"enabled"
    }))?;
    fields.validate()?;
    Ok(fields)
}

/// Bundled offerings, with the provider's default first.
pub fn model_offerings(provider: &str) -> anyhow::Result<Vec<ModelFields>> {
    let default = default_model(provider)?;
    let catalog = catalog();
    let mut offerings = vec![default.clone()];
    if let Some(entries) = catalog["inference_providers"][provider]["additional_models"].as_array()
    {
        for entry in entries {
            let mut offering = default.clone();
            let model = entry["model"].as_str().expect("bundled model identifier");
            offering.source_model_id = model.into();
            offering.display_name = entry["display_name"]
                .as_str()
                .expect("bundled model name")
                .into();
            offering.model_configuration.model = model.into();
            if let Some(task) = entry["task_type"].as_str() {
                offering.task_type = task.into();
                offering.capabilities.tasks = vec![task.into()];
                offering.capabilities.tool_calls = Some(false);
                offering.model_configuration.parameters = json!({});
            }
            if let Some(status) = entry["status"].as_str() {
                offering.status = status.into();
            }
            if let Some(description) = entry["description"].as_str() {
                offering.description = description.into();
            }
            offering.validate()?;
            offerings.push(offering);
        }
    }
    Ok(offerings)
}

/// Resolve the organization's chosen implementation, never a different
/// organization's latest installation. Legacy registrations reconcile on CLI add.
pub async fn installed_provider<'e, E: sqlx::Executor<'e, Database = sqlx::Postgres>>(
    executor: E,
    registry: &magicmtn_provider_framework::host::Registry,
    provider: &str,
    organization: Option<uuid::Uuid>,
) -> anyhow::Result<magicmtn_provider_framework::host::Installed> {
    let Some(org) = organization else {
        return registry.active(provider);
    };
    let version: Option<String> = sqlx::query_scalar("SELECT version FROM organization_providers WHERE organization_id=$1 AND provider=$2 AND status='enabled'")
        .bind(org).bind(provider).fetch_optional(executor).await?;
    let version =
        version.ok_or_else(|| anyhow::anyhow!("provider outside organization Registry"))?;
    anyhow::ensure!(
        !version.is_empty(),
        "re-add legacy provider through the CLI to select its installed version"
    );
    registry.installed(provider, &version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bedrock_models_use_us_profiles_without_sampling_overrides() {
        let models = model_offerings("aws").unwrap();
        assert_eq!(models.len(), 5);
        assert_eq!(
            models
                .iter()
                .map(|m| m.source_model_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "us.openai.gpt-6.1-sol",
                "us.openai.gpt-6.1-terra",
                "us.openai.gpt-6-astra",
                "us.openai.gpt-6-sol",
                "us.openai.gpt-6-luna"
            ]
        );
        for model in models {
            assert_eq!(model.model_configuration.parameters, json!({}));
            assert_eq!(model.model_configuration.provider.as_deref(), Some("aws"));
        }
    }

    #[test]
    fn fireworks_offerings_have_distinct_inference_identities() {
        let offerings = model_offerings("fireworks").unwrap();
        assert_eq!(offerings.len(), 11);
        assert_eq!(offerings[0], default_model("fireworks").unwrap());
        for (offering, display, model) in [
            (
                &offerings[1],
                "Kimi K3",
                "accounts/fireworks/models/kimi-k3",
            ),
            (
                &offerings[2],
                "Inkling",
                "accounts/fireworks/models/inkling",
            ),
            (
                &offerings[3],
                "Gemma 4 31B IT",
                "accounts/fireworks/models/gemma-4-31b-it",
            ),
            (
                &offerings[4],
                "DeepSeek V4.1 Flash",
                "accounts/fireworks/models/deepseek-v4p1-flash",
            ),
            (
                &offerings[5],
                "Qwen 3.8 Max",
                "accounts/fireworks/models/qwen3p8-max",
            ),
            (
                &offerings[6],
                "Muse Spark Open",
                "accounts/fireworks/models/muse-spark-open",
            ),
            (
                &offerings[7],
                "Minimax M3",
                "accounts/fireworks/models/minimax-m3",
            ),
            (
                &offerings[8],
                "Gemma 4 31B IT NVFP4",
                "accounts/fireworks/models/gemma-4-31b-it-nvfp4",
            ),
            (
                &offerings[9],
                "Kimi K3.1 Pro",
                "accounts/fireworks/models/kimi-k3p1-pro",
            ),
            (
                &offerings[10],
                "MiniMax M3.1 Flash",
                "accounts/fireworks/models/minimax-m3p1-flash",
            ),
        ] {
            assert_eq!(offering.display_name, display);
            assert_eq!(offering.source, "fireworks");
            assert_eq!(offering.source_model_id, model);
            assert_eq!(offering.model_configuration.model, model);
            assert_eq!(
                offering.model_configuration.provider.as_deref(),
                Some("fireworks")
            );
            assert_eq!(
                offering.connection_ref.as_deref(),
                Some("connection:self-fireworks")
            );
        }
        let identities: std::collections::HashSet<_> =
            offerings.iter().map(|o| &o.source_model_id).collect();
        assert_eq!(identities.len(), offerings.len());
        assert_eq!(offerings[3].status, "disabled");
        assert_eq!(offerings[8].status, "disabled");
        assert_eq!(offerings[9].status, "enabled");
        assert_eq!(offerings[9].task_type, "chat");
        assert_eq!(offerings[9].capabilities.tool_calls, Some(true));
        assert_eq!(offerings[10].status, "enabled");
        assert_eq!(offerings[10].task_type, "chat");
        assert_eq!(offerings[10].capabilities.tool_calls, Some(true));
    }

    #[test]
    fn other_providers_keep_their_default_offering() {
        for provider in ["ollama", "custom-provider"] {
            assert_eq!(
                model_offerings(provider).unwrap(),
                vec![default_model(provider).unwrap()]
            );
        }
        let baseten = model_offerings("baseten").unwrap();
        assert_eq!(baseten.len(), 4);
        assert_eq!(baseten[0], default_model("baseten").unwrap());
        for (offering, name, task) in [
            (&baseten[1], "Qwen3 TTS 12Hz", "text_to_speech"),
            (&baseten[2], "NVIDIA Nemotron 3.5 ASR", "transcription"),
            (&baseten[3], "Qwen Image", "image_generation"),
        ] {
            assert_eq!(offering.display_name, name);
            assert_eq!(offering.task_type, task);
            assert_eq!(offering.capabilities.tasks, vec![task]);
            assert_eq!(offering.capabilities.tool_calls, Some(false));
            assert_eq!(offering.status, "disabled");
        }
        assert!(model_offerings("invalid/provider").is_err());
    }
}
