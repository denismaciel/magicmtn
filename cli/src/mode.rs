//! Serialize authentication transitions and retain a lease for the local server.
use anyhow::{bail, Context, Result};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

fn open(name: &str) -> Result<File> {
    // Match the credential store, independent of local/config directory overrides.
    let home = std::env::var_os("HOME").context("HOME environment variable not set")?;
    let directory = std::path::PathBuf::from(home).join(".config/magicmtn/cli");
    fs::create_dir_all(&directory)?;
    fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(name))
        .context("opening MagicMtn mode lock")
}

pub(crate) fn transition() -> Result<File> {
    let lock = open("mode.lock")?;
    lock.try_lock().context(
        "another MagicMtn login or server startup is in progress; retry after it finishes",
    )?;
    Ok(lock)
}

pub(crate) fn server() -> Result<File> {
    let lock = open("local-server.lock")?;
    lock.try_lock()
        .context("a local MagicMtn server is running; run `magic server stop` first")?;
    lock.set_len(0)?;
    Ok(lock)
}

pub(crate) fn ready(lock: &mut File, directory: &Path) -> Result<()> {
    serde_json::to_writer(&mut *lock, directory)?;
    lock.flush()?;
    Ok(())
}

/// Read the active server's directory only while its lease is held.
pub(crate) fn running_directory() -> Result<Option<PathBuf>> {
    let lease = open("local-server.lock")?;
    match lease.try_lock() {
        Ok(()) => Ok(None),
        Err(std::fs::TryLockError::WouldBlock) => {
            let contents = std::io::read_to_string(lease)?;
            if contents.is_empty() {
                return Ok(None);
            }
            Ok(Some(
                serde_json::from_str(&contents)
                    .context("reading running local server directory")?,
            ))
        }
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

pub(crate) fn login() -> Result<File> {
    let transition = transition()?;
    let lease = open("local-server.lock")?;
    match lease.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            bail!("local MagicMtn server is running; run `magic server stop` before `magic login`");
        }
        Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
    }
    // Also recognize servers started before the global lease was introduced.
    if magicmtn_local::is_running(&magicmtn_local::directory()?)? {
        bail!("local MagicMtn server is running; run `magic server stop` before `magic login`");
    }
    Ok(transition)
}

/// The consumer's selected server. Developer overrides do not change this state.
#[derive(Clone, serde::Deserialize, serde::Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(crate) enum Selection {
    Cloud { server: String },
    Local { directory: PathBuf },
}

fn selection_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME environment variable not set")?;
    Ok(PathBuf::from(home).join(".config/magicmtn/cli/selection.json"))
}

pub(crate) fn selected() -> Result<Option<Selection>> {
    match fs::read(selection_path()?) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).context("reading selected MagicMtn mode")?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let selection = running_directory()?.map(|directory| Selection::Local { directory });
            if let Some(selection) = &selection {
                select(selection)?;
            }
            Ok(selection)
        }
        Err(error) => Err(error).context("reading selected MagicMtn mode"),
    }
}

pub(crate) fn select(selection: &Selection) -> Result<()> {
    let path = selection_path()?;
    let directory = path.parent().context("mode directory missing")?;
    fs::create_dir_all(directory)?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(&mut file, selection)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path)
        .context("saving selected MagicMtn mode")?;
    Ok(())
}

pub(crate) fn default_server() -> &'static str {
    if cfg!(feature = "development-server") {
        "http://127.0.0.1:8080"
    } else {
        "https://api.magicmtn.ai"
    }
}

pub(crate) fn cloud_server() -> Result<String> {
    Ok(match selected()? {
        Some(Selection::Cloud { server }) => server,
        _ => "https://api.magicmtn.ai".to_owned(),
    })
}

pub(crate) fn development_server(server: &reqwest::Url) -> bool {
    server.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_matches(&['[', ']'][..])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    })
}
