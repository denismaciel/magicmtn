use anyhow::{ensure, Result};
use magicmtn_sdk::plugin::Capabilities;
use magicmtn_sdk::{BoxFuture, InferenceBackend, InferenceRequest, InferenceResponse};
use serde_json::json;

struct Echo;
impl InferenceBackend for Echo {
    fn infer<'a>(&'a self, request: &'a InferenceRequest) -> BoxFuture<'a, InferenceResponse> {
        Box::pin(async move {
            ensure!(request.task == "echo", "unsupported task");
            Ok(InferenceResponse {
                model: request.model.model.clone(),
                output: json!({"value":request.input,"pid":std::process::id()}),
            })
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    magicmtn_provider_framework::run_inference(
        "minimal",
        env!("CARGO_PKG_VERSION"),
        Capabilities::inference(vec!["echo".into()]),
        Echo,
    )
    .await
}
