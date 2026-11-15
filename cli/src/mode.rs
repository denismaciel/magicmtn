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

pub(crate) fn server(directory: &Path) -> Result<File> {
    let mut lock = open("local-server.lock")?;
    lock.try_lock()
        .context("a local MagicMtn server is running; run `magic server stop` first")?;
    lock.set_len(0)?;
    serde_json::to_writer(&mut lock, directory)?;
    lock.flush()?;
    Ok(lock)
}

/// Read the active server's directory only while its lease is held.
pub(crate) fn running_directory() -> Result<Option<PathBuf>> {
    let lease = open("local-server.lock")?;
    match lease.try_lock() {
        Ok(()) => Ok(None),
        Err(std::fs::TryLockError::WouldBlock) => Ok(Some(
            serde_json::from_reader(lease).context("reading running local server directory")?,
        )),
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
