use anyhow::Error;
use sentry::{ClientInitGuard, ClientOptions, Level};

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

pub(crate) fn report(client: &str, operation: &str, error: &Error) {
    if !crate::configuration::error_reporting().unwrap_or(false) {
        return;
    }
    let message = error.to_string().to_ascii_lowercase();
    // Remote exits are queued by the authenticated server relay.
    if message.starts_with("terminal disconnected: status ")
        || message.starts_with("remote terminal exited with status ")
        || operation == "report"
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
    sentry::with_scope(
        |scope| {
            scope.set_tag("client", client);
            scope.set_tag("operation", operation);
            scope.set_tag("failure_category", category);
            if let Some(status) = status {
                scope.set_tag("http_status", status);
            }
        },
        || {
            sentry::capture_message(&format!("{client} {operation} failed"), Level::Error);
        },
    );
}
