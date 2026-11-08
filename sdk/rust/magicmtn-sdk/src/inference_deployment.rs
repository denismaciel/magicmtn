//! Optional management of inference serving resources, independent of invocation.
//!
//! Resource IDs are provider-owned and must be retained by the caller. Operations
//! request transitions; callers poll `status` for completion. A successful stop
//! request does not itself prove that capacity has been released. Resources may
//! be shared across sessions: ending a session or an RPC process never implies
//! stopping or deleting its inference resource.

use crate::{
    plugin::{ErrorCode, ProviderError},
    BoxFuture,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceDeploymentSpec {
    /// Caller-chosen name for identifying/reconciling an uncertain create.
    pub name: String,
    pub model: String,
    /// Provider-specific, non-secret serving configuration.
    pub configuration: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceDeploymentRef {
    pub provider_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InferenceDeploymentState {
    Creating,
    Starting,
    Ready,
    Stopping,
    Stopped,
    Deleting,
    Deleted,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceDeployment {
    pub resource: InferenceDeploymentRef,
    /// Base model being hosted, if known (e.g. absent after deletion).
    pub model: Option<String>,
    /// Value to use in `ModelConfiguration.model` to address this resource.
    pub inference_model: String,
    pub state: InferenceDeploymentState,
    /// None when the provider cannot establish the replica count.
    pub ready_replicas: Option<u32>,
}

fn unsupported<'a, T: Send + 'a>() -> BoxFuture<'a, T> {
    Box::pin(async {
        Err(ProviderError {
            code: ErrorCode::UnsupportedOperation,
            retryable: false,
            message: "inference deployment operation is not supported".into(),
        }
        .into())
    })
}

/// Optional serving-resource lifecycle. Advertise only implemented operations
/// in the provider manifest. Inference-only providers need not implement this
/// trait. Implementers may support a subset using the unsupported defaults.
pub trait InferenceDeploymentBackend: Send + Sync {
    /// Create a resource without starting serving capacity, where supported.
    /// Never blindly replay an uncertain create; reconcile using `spec.name`.
    fn create<'a>(
        &'a self,
        _spec: &'a InferenceDeploymentSpec,
    ) -> BoxFuture<'a, InferenceDeployment> {
        unsupported()
    }
    fn start<'a>(
        &'a self,
        _resource: &'a InferenceDeploymentRef,
    ) -> BoxFuture<'a, InferenceDeployment> {
        unsupported()
    }
    fn status<'a>(
        &'a self,
        _resource: &'a InferenceDeploymentRef,
    ) -> BoxFuture<'a, InferenceDeployment> {
        unsupported()
    }
    fn stop<'a>(
        &'a self,
        _resource: &'a InferenceDeploymentRef,
    ) -> BoxFuture<'a, InferenceDeployment> {
        unsupported()
    }
    /// Request deletion. Success acknowledges the request, not final teardown.
    fn delete<'a>(&'a self, _resource: &'a InferenceDeploymentRef) -> BoxFuture<'a, ()> {
        unsupported()
    }
}
