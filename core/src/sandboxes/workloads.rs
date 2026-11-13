//! Bounded workload execution in managed sandboxes.
use super::*;
use crate::provider_connections::{self as connections, Binding};
use magicmtn_sdk::ProviderSandbox;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Execute {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    timeout_ms: u64,
    max_output_bytes: usize,
}
impl Execute {
    fn validate(&self) -> Result<(), (StatusCode, String)> {
        if self.command.is_empty()
            || self.command.len() > 256
            || self.command.starts_with('-')
            || self.command.contains('\0')
            || self
                .args
                .iter()
                .any(|arg| arg.contains('\0') || arg.len() > 2048)
            || self.command.len() + self.args.iter().map(String::len).sum::<usize>() > 128 * 1024
            || self.args.len() > 32
            || !(1..=30_000).contains(&self.timeout_ms)
            || !(1..=16 * 1024).contains(&self.max_output_bytes)
        {
            return Err(invalid(
                "invalid command or execution limits (maximum 30 seconds, 16 KiB output, 32 arguments of 2 KiB each)",
            ));
        }
        Ok(())
    }
}

pub(super) async fn execute(
    State(s): State<ServiceState>,
    Path(id): Path<String>,
    Json(body): Json<Execute>,
) -> Reply {
    body.validate()?;
    let saved = resolve_store(&s.store, &id).await?;
    if !matches!(
        saved["status"].as_str(),
        Some("running" | "starting" | "RUNNING" | "ready" | "PENDING" | "STARTING" | "pending")
    ) || saved["provider_id"]
        .as_str()
        .is_none_or(|id| id.starts_with("pending:"))
    {
        return Err((
            StatusCode::CONFLICT,
            "sandbox is not available for execution".into(),
        ));
    }
    if let Some(expires) = saved["expires_at"].as_str() {
        if chrono::DateTime::parse_from_rfc3339(expires).map_err(failed)? <= chrono::Utc::now() {
            return Err((StatusCode::CONFLICT, "sandbox has expired".into()));
        }
    }
    let binding: Binding =
        serde_json::from_value(saved["metadata"]["_magicmtn_connection"].clone())
            .map_err(failed)?;
    let sandbox = ProviderSandbox {
        provider_id: saved["provider_id"].as_str().unwrap().into(),
        status: saved["status"].as_str().map(str::to_owned),
        create_time_ms: saved["create_time_ms"].as_i64(),
        metadata: Some(saved["metadata"].clone()),
    };
    let result = connections::dispatch_store(
        &s.store,
        &s.registry,
        &binding,
        &format!("sandbox:{}", saved["id"].as_str().unwrap()),
        Operation::SandboxExecBounded {
            sandbox,
            command: body.command,
            args: body.args,
            timeout_ms: body.timeout_ms,
            max_output_bytes: body.max_output_bytes,
        },
        Duration::from_millis(body.timeout_ms + 30_000),
        false,
    )
    .await;
    match result {
        Ok(ProviderValue::Exec(result)) => Ok(Json(json!(result))),
        _ => {
            tracing::error!("sandbox workload execution outcome uncertain");
            Err((StatusCode::BAD_GATEWAY, "Execution outcome uncertain; inspect the sandbox before retrying a command that changes state".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn execution_is_bounded_before_dispatch() {
        let mut body = Execute {
            command: "sh".into(),
            args: vec!["-lc".into(), "echo hello".into()],
            timeout_ms: 30_000,
            max_output_bytes: 16_384,
        };
        assert!(body.validate().is_ok());
        body.timeout_ms += 1;
        assert!(body.validate().is_err());
        body.timeout_ms = 1000;
        body.max_output_bytes = 16_385;
        assert!(body.validate().is_err());
        body.max_output_bytes = 16_384;
        body.args = vec!["x".repeat(2048); 32];
        assert!(body.validate().is_ok());
        body.args.push("overflow".into());
        assert!(body.validate().is_err());
        body.args = vec!["x".repeat(2049)];
        assert!(body.validate().is_err());
        body.args = vec!["bad\0arg".into()];
        assert!(body.validate().is_err());
    }
}
