//! OpenCode workspace commands and installation of the OpenCode V2 plugin.
use crate::{authentication, configuration, pi, Api};
use anyhow::{ensure, Context, Result};
use clap::Subcommand;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
};
use uuid::Uuid;

#[derive(Subcommand)]
pub enum Commands {
    #[command(hide = true)]
    Account,
    #[command(hide = true)]
    Inference {
        #[arg(long)]
        model: Uuid,
    },
    /// List OpenCode workspaces
    List,
    /// Start OpenCode in a new managed workspace
    Create {
        #[arg(long, conflicts_with = "connection")]
        model: Option<Uuid>,
        #[arg(long)]
        connection: Option<Uuid>,
        #[arg(long)]
        idempotency_key: Option<Uuid>,
    },
    /// Connect to the OpenCode terminal
    Attach { id: Uuid },
    /// Read the managed session status
    Status { id: Uuid },
    /// Stop OpenCode and keep the sandbox
    Stop { id: Uuid },
    /// Restart OpenCode in the same session
    Restart { id: Uuid },
    /// Delete the workspace and sandbox
    Terminate { id: Uuid },
    /// Connect a provider with model and api_key JSON supplied on stdin
    Connect,
    /// List the account's connected inference profiles
    Connections,
    /// Revoke a connected inference profile
    Disconnect { id: Uuid },
    #[command(hide = true)]
    Catalog,
    #[command(hide = true)]
    Defaults {
        #[arg(
            long,
            conflicts_with = "connection",
            required_unless_present = "connection"
        )]
        model: Option<Uuid>,
        #[arg(long)]
        connection: Option<Uuid>,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum DefaultInference {
    Model(Uuid),
    Connection(Uuid),
}

pub(crate) fn plugin_directory(home: &std::path::Path) -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|root| !root.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
        .join("opencode/plugins/magicmtn")
}

pub(crate) fn install_extension() -> Result<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);
    let directory = plugin_directory(&home);
    fs::create_dir_all(&directory)?;
    write_file(
        &directory,
        "config.js",
        &format!(
            "export const executable = {};\n",
            serde_json::to_string(&std::env::current_exe()?)?
        ),
    )?;
    for (name, source) in [
        (
            "index.js",
            include_str!("../../interfaces/opencode/index.js"),
        ),
        ("tui.js", include_str!("../../interfaces/opencode/tui.js")),
        (
            "magic.js",
            include_str!("../../interfaces/opencode/magic.js"),
        ),
        (
            "package.json",
            include_str!("../../interfaces/opencode/package.json"),
        ),
    ] {
        write_file(&directory, name, source)?;
    }
    Ok(directory)
}

fn write_file(directory: &std::path::Path, name: &str, source: &str) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(source.as_bytes())?;
    file.persist(directory.join(name))?;
    Ok(())
}

pub(crate) async fn run(api: &Api, command: Commands) -> Result<()> {
    ensure!(
        !api.local || matches!(command, Commands::Account),
        "Use /magic in OpenCode to manage local sandboxes"
    );
    let path = "v1/opencode/workspaces";
    let value = match command {
        Commands::Account => authentication::account(api).await?,
        Commands::Inference { model } => {
            api.request(
                Method::POST,
                "v1/opencode/inference",
                Some(json!({"model_id":model})),
            )
            .await?
        }
        Commands::List => api.request(Method::GET, path, None).await?,
        Commands::Create {
            model,
            connection,
            idempotency_key,
        } => {
            let key = idempotency_key.unwrap_or_else(Uuid::new_v4);
            let (model, connection) = if model.is_none() && connection.is_none() {
                let catalog = catalog(api).await?;
                let defaults = &catalog["defaults"];
                let connection: Option<Uuid> =
                    serde_json::from_value(defaults["connection_id"].clone())?;
                let model = if connection.is_some() {
                    None
                } else {
                    Some(
                        serde_json::from_value(defaults["model_id"].clone())
                            .context("Select an available workspace model with /magic default")?,
                    )
                };
                (model, connection)
            } else {
                (model, connection)
            };
            eprintln!(
                "Creating workspace {key}. Inspect it with magic opencode list if interrupted."
            );
            api.request(
                Method::POST,
                path,
                Some(configuration::creation_body(json!({
                    "model_id": model, "connection_id": connection, "idempotency_key": key,
                }))?),
            )
            .await?
        }
        Commands::Attach { id } => {
            return pi::terminal(
                api,
                &format!("{path}/{id}/terminal"),
                &format!("workspace {id}"),
            )
            .await;
        }
        Commands::Status { id } => {
            api.request(Method::GET, &format!("{path}/{id}/session"), None)
                .await?
        }
        Commands::Stop { id } => workspace_action(api, path, id, "stop").await?,
        Commands::Restart { id } => workspace_action(api, path, id, "restart").await?,
        Commands::Terminate { id } => workspace_action(api, path, id, "terminate").await?,
        Commands::Connect => {
            let mut input = String::new();
            std::io::stdin().take(32769).read_to_string(&mut input)?;
            ensure!(input.len() <= 32768, "connection input too large");
            let value: Value =
                serde_json::from_str(&input).context("expected model and api_key JSON on stdin")?;
            api.request(
                Method::POST,
                "v1/gateway-inference-connections",
                Some(value),
            )
            .await?
        }
        Commands::Connections => connections(api).await?,
        Commands::Disconnect { id } => {
            api.request(
                Method::POST,
                &format!("v1/gateway-inference-connections/{id}/revoke"),
                Some(json!({})),
            )
            .await?
        }
        Commands::Catalog => catalog(api).await?,
        Commands::Defaults { model, connection } => {
            let catalog = catalog(api).await?;
            let default = if let Some(model) = model {
                ensure!(
                    available(&catalog, &json!(model)),
                    "Select an available MagicMtn model"
                );
                DefaultInference::Model(model)
            } else {
                let connection = connection.context("Select a gateway connection")?;
                let available = connections(api).await?;
                ensure!(
                    available["connections"]
                        .as_array()
                        .is_some_and(|items| items.iter().any(|item| item["id"]
                            == json!(connection)
                            && item["revoked"] == false)),
                    "Select an active gateway connection"
                );
                DefaultInference::Connection(connection)
            };
            let path = pi::defaults_path(api, &catalog)?.with_extension("opencode.json");
            let mut file = tempfile::NamedTempFile::new_in(
                path.parent().context("preferences directory missing")?,
            )?;
            serde_json::to_writer(&mut file, &default)?;
            file.as_file().sync_all()?;
            file.persist(path)?;
            json!({"saved":true})
        }
    };
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}

async fn workspace_action(api: &Api, path: &str, id: Uuid, action: &str) -> Result<Value> {
    api.request(
        Method::POST,
        &format!("{path}/{id}/{action}"),
        Some(json!({})),
    )
    .await
}

async fn connections(api: &Api) -> Result<Value> {
    api.request(Method::GET, "v1/gateway-inference-connections", None)
        .await
}

fn available(catalog: &Value, model: &Value) -> bool {
    catalog["models"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| &item["id"] == model))
}

async fn catalog(api: &Api) -> Result<Value> {
    let mut catalog = api
        .request(Method::GET, "v1/opencode/catalog", None)
        .await?;
    let path = pi::defaults_path(api, &catalog)?.with_extension("opencode.json");
    match fs::read(path) {
        Ok(bytes) => {
            match serde_json::from_slice(&bytes).context("invalid saved OpenCode inference")? {
                DefaultInference::Model(model) if available(&catalog, &json!(model)) => {
                    catalog["defaults"]["model_id"] = json!(model);
                }
                DefaultInference::Connection(connection) => {
                    catalog["defaults"]["connection_id"] = json!(connection);
                }
                DefaultInference::Model(_) => {}
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("reading OpenCode defaults"),
    }
    Ok(catalog)
}
