//! User preferences are deliberately separate from server-scoped runner registration.
use crate::Api;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct Config {
    local_sandbox: bool,
}
pub(crate) fn directory() -> Result<PathBuf> {
    Ok(
        PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?)
            .join(".config/magicmtn"),
    )
}
fn read(path: &Path) -> Result<Config> {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).with_context(|| {
            format!(
                "invalid TOML in {} (use local-sandbox = true)",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}
fn provider() -> Result<&'static str> {
    match std::env::consts::OS {
        "macos" => Ok("apple-container"),
        "linux" => Ok("firecracker"),
        _ => anyhow::bail!("local sandboxes require macOS or Linux"),
    }
}
pub(crate) fn creation_body(api: &Api, mut body: Value) -> Result<Value> {
    let root = directory()?;
    if read(&root.join("config"))?.local_sandbox {
        provider()?;
        let bindings: Value = serde_json::from_slice(
            &fs::read(root.join("local-connections.json"))
                .context("saved local provider connections are unavailable")?,
        )?;
        let id: Uuid = serde_json::from_value(bindings[api.base.as_str()].clone())
            .context("no local runner connection registered for this server")?;
        body["local_connection_id"] = json!(id);
    }
    Ok(body)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_is_opt_in_and_strict() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        assert!(!read(&path).unwrap().local_sandbox);
        for (text, expected) in [
            ("", false),
            ("local-sandbox = false", false),
            ("local-sandbox = true", true),
        ] {
            fs::write(&path, text).unwrap();
            assert_eq!(read(&path).unwrap().local_sandbox, expected);
        }
        for text in [
            "local-sandbox: true",
            "local-sandbox = 'true'",
            "local_sandbox = true",
        ] {
            fs::write(&path, text).unwrap();
            assert!(read(&path).is_err());
        }
    }
}

pub(crate) fn write_catalog(
    registry: &magicmtn_provider_framework::host::Registry,
    root: &Path,
) -> Result<()> {
    let releases: Vec<_> = registry.list()?.into_iter().map(|installed| {
        let executable = root.join(&installed.package.name).join(&installed.package.version).join("provider");
        json!({"package":installed.package,"manifest":installed.manifest,"active":installed.active,"executable":executable})
    }).collect();
    let catalog = root.join("catalog.json");
    let mut file = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer_pretty(&mut file, &releases)?;
    file.persist(&catalog)?;
    Ok(())
}

/// Keep install/remove operations separate from running provider processes.
pub(crate) fn lock_provider_store(root: &Path) -> Result<fs::File> {
    fs::create_dir_all(root)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(".lock"))?;
    lock.try_lock().context("provider store is in use; stop the local runner before installing or uninstalling providers")?;
    Ok(lock)
}
