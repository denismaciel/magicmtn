//! Install a canonical agent or a local Python agent using the CLI account session.
use crate::Api;
use anyhow::{bail, ensure, Context, Result};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{json, Value};
use std::{
    io::{self, Write},
    path::Path,
    path::PathBuf,
    process::Command,
};

#[derive(Subcommand)]
pub enum Commands {
    /// Install a canonical agent (such as pi) or a local agent directory
    Install { agent: String },
}

pub async fn run(api: &Api, command: Commands) -> Result<()> {
    if api.local && matches!(&command, Commands::Install { agent } if agent == "pi") {
        let directory = crate::pi::install_extension()?;
        println!(
            "Installed /magic in {}. Start Pi or run /reload.",
            directory.display()
        );
        return Ok(());
    }
    ensure!(api.token.is_some(), "sign in first with `magic login`");
    match command {
        Commands::Install { agent } if agent == "pi" => {
            // The server owns canonical agent identity and its workspace installation.
            let installed = api
                .request(
                    Method::POST,
                    "v1/agents/install",
                    Some(json!({"canonical":"pi"})),
                )
                .await?;
            let directory = crate::pi::install_extension()?;
            println!(
                "Installed {} in your workspace and /magic in {}. Start pi or run /reload.",
                installed["name"].as_str().unwrap_or("pi"),
                directory.display()
            );
        }
        Commands::Install { agent } => {
            let root = PathBuf::from(&agent);
            let definition = local_definition(&root)?;
            let name = definition["name"]
                .as_str()
                .context("agent definition missing name")?
                .to_owned();
            let directory = api.request(Method::GET, "v1/code-agents", None).await?;
            let agents = directory["agents"]
                .as_array()
                .context("agent directory response missing agents")?;
            let existing = agents.iter().find(|item| {
                item["name"]
                    .as_str()
                    .is_some_and(|current| current.to_lowercase() == name.to_lowercase())
            });
            let expected_id = if let Some(existing) = existing {
                ensure!(
                    existing["sdk"] != "pi",
                    "Pi is a canonical agent; choose another name"
                );
                let id = existing["id"]
                    .as_str()
                    .context("existing agent missing ID")?;
                println!("An agent named {name:?} already exists in this workspace. Continuing will overwrite the existing agent.");
                Some(id.to_owned())
            } else {
                println!("A new agent named {name:?} will be created in this workspace.");
                None
            };
            print!("Continue? [y/N] ");
            io::stdout().flush()?;
            let mut answer = String::new();
            io::stdin().read_line(&mut answer)?;
            if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                println!("Installation cancelled.");
                return Ok(());
            }
            let installed = api
                .request(
                    Method::POST,
                    "v1/agents/install",
                    Some(json!({"definition": definition, "expected_agent_id": expected_id, "create_only": expected_id.is_none()})),
                )
                .await?;
            let id = installed["id"]
                .as_str()
                .context("install response missing agent ID")?;
            let version = installed["version"]
                .as_str()
                .context("install response missing version")?;
            println!(
                "Installed agent {} (version {}) in your workspace. View it at /agents/{}.",
                name, version, id
            );
        }
    }
    Ok(())
}

const DEFINITION_SCRIPT: &str = r#"
import json
import sys
from magicmtn.agents import installation_payload

print(json.dumps(installation_payload(sys.argv[1]), ensure_ascii=False))
"#;

fn local_definition(root: &Path) -> Result<Value> {
    let root = root.canonicalize().context("finding agent directory")?;
    ensure!(root.is_dir(), "agent source must be a directory");
    let python = if root.join(".venv/bin/python").is_file() {
        root.join(".venv/bin/python")
    } else {
        PathBuf::from("python3")
    };
    let output = Command::new(python)
        .arg("-c")
        .arg(DEFINITION_SCRIPT)
        .arg(&root)
        .current_dir(&root)
        .output()
        .context("loading agent definition with Python")?;
    if !output.status.success() {
        bail!(
            "could not load agent definition: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("agent definition did not produce valid JSON")
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    #[test]
    fn install_accepts_pi_and_a_local_path() {
        assert!(crate::Cli::try_parse_from(["magic", "agents", "install", "pi"]).is_ok());
        assert!(crate::Cli::try_parse_from(["magic", "agents", "install", "./my-agent"]).is_ok());
    }
}
