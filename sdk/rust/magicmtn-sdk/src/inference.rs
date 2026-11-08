//! Provider-neutral inference for local and hosted models.
//!
//! The core contract carries a task and structured input/output. Higher-level
//! task adapters can validate concrete types such as chat messages, embeddings,
//! classification scores, or probabilistic decisions. Provider implementations
//! own transport, credentials, and any local server lifecycle.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::BoxFuture;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfiguration {
    pub model: String,
    pub provider: Option<String>,
    pub version: Option<String>,
    /// Non-secret inference parameters, e.g. temperature and token budget.
    pub parameters: Value,
}

/// A model invocation. `task` names the operation the caller expects, such as
/// `chat`, `embedding`, or `classification`; providers must reject unsupported
/// model/task combinations. `input` follows that task's schema, not a provider's
/// wire format. Text chat follows the conventions in [`crate::chat`].
/// Model parameters are non-secret; credentials stay with the
/// provider implementation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InferenceRequest {
    pub model: ModelConfiguration,
    pub task: String,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InferenceResponse {
    /// The model that actually served the request, after alias resolution.
    pub model: String,
    /// Result in the requested task's schema, including scores or probabilities
    /// when the task defines them.
    pub output: Value,
}

/// Invoke a model through local or hosted inference compute.
///
/// Unsupported tasks, inputs, or parameters must return an error rather than
/// being silently ignored. Streaming is a separate future capability.
pub trait InferenceBackend: Send + Sync {
    fn infer<'a>(&'a self, request: &'a InferenceRequest) -> BoxFuture<'a, InferenceResponse>;
}
