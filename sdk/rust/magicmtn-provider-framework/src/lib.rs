//! Shared persistent JSON RPC runner for independently packaged providers.
use anyhow::{anyhow, bail, Context, Result};
use magicmtn_sdk::plugin::{
    Capabilities, ErrorCode, Operation, Outcome, ProviderError, ProviderManifest, Request,
    Response, Value, MANIFEST_VERSION, PROTOCOL_VERSION,
};
use magicmtn_sdk::{
    BoxFuture, InferenceBackend, InferenceDeploymentBackend, SandboxBackend, SearchEngineBackend,
};
use std::{
    io::{BufRead, Read, Write},
    path::Path,
};
pub const MAX_MESSAGE_BYTES: u64 = 1024 * 1024;

/// Serve web search requests from a persistent provider process.
pub async fn run_search(
    provider: &str,
    version: &str,
    backend: impl SearchEngineBackend,
) -> Result<()> {
    let backend = &backend;
    serve(provider, version, Capabilities::search(), |operation| {
        Box::pin(async move {
            match operation {
                Operation::Search { request } => backend.search(&request).await.map(Value::Search),
                _ => Err(anyhow!("unsupported search operation")),
            }
        })
    })
    .await
}

async fn serve<'a>(
    provider: &str,
    version: &str,
    capabilities: Capabilities,
    mut dispatch: impl FnMut(Operation) -> BoxFuture<'a, Value>,
) -> Result<()> {
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    loop {
        let mut bytes = Vec::new();
        if input
            .by_ref()
            .take(MAX_MESSAGE_BYTES + 1)
            .read_until(b'\n', &mut bytes)?
            == 0
        {
            return Ok(());
        }
        if bytes.len() as u64 > MAX_MESSAGE_BYTES || !bytes.ends_with(b"\n") {
            bail!("invalid or oversized RPC frame");
        }
        let request: Request = serde_json::from_slice(&bytes).context("invalid provider RPC")?;
        if request.protocol_version != PROTOCOL_VERSION || request.provider != provider {
            bail!("provider protocol or identity mismatch");
        }
        let shutdown = matches!(request.operation, Operation::Shutdown);
        let unsupported = !capabilities.supports(&request.operation);
        let result = match request.operation {
            Operation::Describe => Ok(Value::Description(ProviderManifest {
                manifest_version: MANIFEST_VERSION,
                provider_version: version.into(),
                capabilities: capabilities.clone(),
            })),
            Operation::Shutdown => Ok(Value::Done),
            _ if unsupported => Err(anyhow!("unsupported operation")),
            operation => dispatch(operation).await,
        };
        let outcome = match result {
            Ok(value) => Outcome::Ok(value),
            Err(error) => Outcome::Error(
                error
                    .downcast_ref::<ProviderError>()
                    .cloned()
                    .unwrap_or(ProviderError {
                        code: if unsupported {
                            ErrorCode::UnsupportedOperation
                        } else {
                            ErrorCode::Internal
                        },
                        retryable: false,
                        message: if unsupported {
                            "operation is not supported"
                        } else {
                            "provider operation failed"
                        }
                        .into(),
                    }),
            ),
        };
        let response = Response {
            id: request.id,
            protocol_version: PROTOCOL_VERSION,
            provider: provider.into(),
            outcome,
        };
        let bytes = serde_json::to_vec(&response)?;
        if bytes.len() as u64 >= MAX_MESSAGE_BYTES {
            bail!("provider result exceeds message limit");
        }
        output.write_all(&bytes)?;
        output.write_all(b"\n")?;
        output.flush()?;
        if shutdown {
            return Ok(());
        }
    }
}
pub async fn run_sandbox(
    provider: &str,
    version: &str,
    capabilities: Capabilities,
    backend: impl SandboxBackend + Sync,
) -> Result<()> {
    let backend = &backend;
    serve(provider, version, capabilities, |operation| {
        Box::pin(async move {
            match operation {
                Operation::SandboxCreate {
                    template,
                    ttl_seconds,
                } => match ttl_seconds {
                    Some(0) => Err(anyhow!("sandbox TTL must be positive")),
                    Some(seconds) => backend.create_with_ttl(&template, seconds).await,
                    None => backend.create(&template).await,
                }
                .map(Value::Sandbox),
                Operation::SandboxStatus { sandbox } => {
                    backend.status(&sandbox).await.map(Value::SandboxStatus)
                }
                Operation::SandboxTerminate { sandbox } => {
                    backend.terminate(&sandbox).await.map(|()| Value::Done)
                }
                Operation::SandboxExec {
                    sandbox,
                    command,
                    args,
                } => backend
                    .exec(&sandbox, &command, &args)
                    .await
                    .map(Value::Text),
                Operation::SandboxExecBounded {
                    sandbox,
                    command,
                    args,
                    timeout_ms,
                    max_output_bytes,
                } => backend
                    .exec_bounded(&sandbox, &command, &args, timeout_ms, max_output_bytes)
                    .await
                    .map(Value::Exec),
                Operation::SandboxSsh { sandbox } => {
                    backend.ssh(&sandbox).await.map(|()| Value::Done)
                }
                Operation::SandboxScp {
                    sandbox,
                    local,
                    remote,
                    direction,
                } => backend
                    .scp(&sandbox, Path::new(&local), Path::new(&remote), direction)
                    .await
                    .map(|()| Value::Done),
                _ => Err(anyhow!("unsupported sandbox operation")),
            }
        })
    })
    .await
}
pub async fn run_inference(
    provider: &str,
    version: &str,
    capabilities: Capabilities,
    backend: impl InferenceBackend,
) -> Result<()> {
    let backend = &backend;
    serve(provider, version, capabilities, |operation| {
        Box::pin(async move {
            match operation {
                Operation::Infer { request } => backend.infer(&request).await.map(Value::Inference),
                _ => Err(anyhow!("unsupported inference operation")),
            }
        })
    })
    .await
}

/// Serve sandbox and inference operations from one provider process.
pub async fn run_sandbox_with_inference(
    provider: &str,
    version: &str,
    capabilities: Capabilities,
    backend: impl SandboxBackend + InferenceBackend + Sync,
) -> Result<()> {
    let backend = &backend;
    serve(provider, version, capabilities, |operation| {
        Box::pin(async move {
            match operation {
                Operation::SandboxCreate {
                    template,
                    ttl_seconds,
                } => match ttl_seconds {
                    Some(0) => Err(anyhow!("sandbox TTL must be positive")),
                    Some(seconds) => backend.create_with_ttl(&template, seconds).await,
                    None => backend.create(&template).await,
                }
                .map(Value::Sandbox),
                Operation::SandboxStatus { sandbox } => {
                    backend.status(&sandbox).await.map(Value::SandboxStatus)
                }
                Operation::SandboxTerminate { sandbox } => {
                    backend.terminate(&sandbox).await.map(|()| Value::Done)
                }
                Operation::SandboxExec {
                    sandbox,
                    command,
                    args,
                } => backend
                    .exec(&sandbox, &command, &args)
                    .await
                    .map(Value::Text),
                Operation::SandboxExecBounded {
                    sandbox,
                    command,
                    args,
                    timeout_ms,
                    max_output_bytes,
                } => backend
                    .exec_bounded(&sandbox, &command, &args, timeout_ms, max_output_bytes)
                    .await
                    .map(Value::Exec),
                Operation::SandboxSsh { sandbox } => {
                    backend.ssh(&sandbox).await.map(|()| Value::Done)
                }
                Operation::SandboxScp {
                    sandbox,
                    local,
                    remote,
                    direction,
                } => backend
                    .scp(&sandbox, Path::new(&local), Path::new(&remote), direction)
                    .await
                    .map(|()| Value::Done),
                Operation::Infer { request } => backend.infer(&request).await.map(Value::Inference),
                _ => Err(anyhow!("unsupported operation")),
            }
        })
    })
    .await
}
#[cfg(feature = "host")]
pub mod host;

/// Serve invocation and optional lifecycle capabilities from one provider.
/// Shutdown closes the RPC process only; remote deployments outlive it.
pub async fn run_inference_with_deployments(
    provider: &str,
    version: &str,
    capabilities: Capabilities,
    backend: impl InferenceBackend + InferenceDeploymentBackend,
) -> Result<()> {
    let backend = &backend;
    serve(provider, version, capabilities, |operation| {
        Box::pin(async move {
            match operation {
                Operation::Infer { request } => backend.infer(&request).await.map(Value::Inference),
                Operation::InferenceDeploymentCreate { spec } => {
                    backend.create(&spec).await.map(Value::InferenceDeployment)
                }
                Operation::InferenceDeploymentStart { resource } => backend
                    .start(&resource)
                    .await
                    .map(Value::InferenceDeployment),
                Operation::InferenceDeploymentStatus { resource } => backend
                    .status(&resource)
                    .await
                    .map(Value::InferenceDeployment),
                Operation::InferenceDeploymentStop { resource } => backend
                    .stop(&resource)
                    .await
                    .map(Value::InferenceDeployment),
                Operation::InferenceDeploymentDelete { resource } => {
                    backend.delete(&resource).await.map(|()| Value::Done)
                }
                _ => Err(anyhow!("unsupported inference operation")),
            }
        })
    })
    .await
}
