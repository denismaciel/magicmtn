//! Attach a terminal through the exact provider release selected by the local server.
use crate::Api;
use anyhow::{ensure, Context, Result};
use magicmtn_provider_framework::host::{Pin, ProcessEnvironment, Registry};
use magicmtn_sdk::{
    plugin::{Operation, Value},
    ProviderSandbox,
};
use reqwest::Method;
use serde::Deserialize;
use std::{collections::BTreeMap, io::IsTerminal, time::Duration};

#[derive(Deserialize)]
struct Connection {
    pin: Pin,
    configuration: BTreeMap<String, String>,
    sandbox: ProviderSandbox,
}

pub(crate) async fn run(api: &Api, id: &str) -> Result<()> {
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "this command requires an interactive terminal"
    );
    let connection: Connection = serde_json::from_value(
        api.request(Method::POST, &format!("v1/sandboxes/{id}/shell"), None)
            .await?,
    )?;
    let root = crate::configuration::directory()?.join("providers");
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .open(root.join(".lock"))?;
    lock.try_lock_shared()
        .context("provider store is being modified")?;
    let registry = Registry::from_catalog(&root.join("catalog.json"))?;
    let mut values = connection.configuration;
    values.insert(
        "PATH".into(),
        "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".into(),
    );
    for key in [
        "HOME",
        "TERM",
        "COLORTERM",
        "LANG",
        "MAGICMTN_MXC_STATE_DIR",
    ] {
        if let Ok(value) = std::env::var(key) {
            values.insert(key.into(), value);
        }
    }
    eprintln!("Opening local sandbox shell; exit returns to your Mac.");
    let result = registry
        .call_configured(
            &connection.pin,
            &format!("local-shell:{id}"),
            Operation::SandboxSsh {
                sandbox: connection.sandbox,
            },
            Duration::from_secs(24 * 60 * 60),
            Some(&ProcessEnvironment { values }),
        )
        .await;
    registry.shutdown().await;
    ensure!(
        matches!(result?, Value::Done),
        "invalid local shell response"
    );
    Ok(())
}
