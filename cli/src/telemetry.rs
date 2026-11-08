use crate::{authentication::Credentials, Api};
use anyhow::Error;
use reqwest::Method;
use sentry::{ClientInitGuard, ClientOptions, Level};
use std::time::Duration;

// A Sentry DSN is a public ingestion address. The CLI cannot rely on a server
// environment variable because it runs on users' machines.
const CLIENTS_DSN: &str = "https://0cd93d5f67a490a71dfdb14744f85bc4@o4512304122429440.ingest.us.sentry.io/4512309266022400";

pub(crate) fn init() -> ClientInitGuard {
    sentry::init((
        CLIENTS_DSN,
        ClientOptions::new()
            .release(concat!("magic-cli@", env!("CARGO_PKG_VERSION")))
            .environment("production")
            .send_default_pii(false),
    ))
}

pub(crate) async fn report(client: &str, operation: &str, error: &Error, server_url: &str) {
    let message = error.to_string().to_ascii_lowercase();
    if operation == "report"
        || message.contains("cancelled")
        || message.contains("not signed in")
        || message.contains("select an available")
    {
        return;
    }
    // Error chains can contain server responses, paths, or provider input.
    // Report a stable failure category and operation without copying those values.
    let status = message
        .strip_prefix("server returned ")
        .and_then(|rest| rest.get(..3))
        .filter(|value| value.bytes().all(|byte| byte.is_ascii_digit()));
    let category = if status.is_some() {
        "server_response"
    } else if message.starts_with("terminal connection")
        || message.starts_with("terminal disconnected")
    {
        "terminal_connection"
    } else if error.downcast_ref::<reqwest::Error>().is_some() {
        "http_transport"
    } else if error.downcast_ref::<std::io::Error>().is_some() {
        "io"
    } else {
        "operation"
    };
    let identity = tokio::time::timeout(Duration::from_secs(2), async {
        let mut api = Api::new(server_url).ok()?;
        api.token = Some(Credentials::new().ok()?.load(&api.base).ok()??);
        let user = api.request(Method::GET, "auth/me", None).await.ok()?;
        let organization = api
            .request(Method::GET, "auth/organizations", None)
            .await
            .ok()
            .and_then(|organizations| organizations.as_array()?.first().cloned());
        Some((user, organization))
    })
    .await
    .ok()
    .flatten();
    sentry::with_scope(
        |scope| {
            scope.set_tag("client", client);
            scope.set_tag("operation", operation);
            scope.set_tag("failure_category", category);
            if let Some(status) = status {
                scope.set_tag("http_status", status);
            }
            if let Some((user, organization)) = identity {
                scope.set_user(Some(sentry::User {
                    id: user["user_id"].as_str().map(str::to_owned),
                    email: user["email"].as_str().map(str::to_owned),
                    ..Default::default()
                }));
                if let Some(id) = organization.as_ref().and_then(|value| value["id"].as_str()) {
                    scope.set_tag("organization_id", id);
                }
            }
        },
        || {
            sentry::capture_message(&format!("{client} {operation} failed"), Level::Error);
        },
    );
}
