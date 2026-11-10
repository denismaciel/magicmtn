//! Copies the tracked working tree and repository metadata independently of attachment.
use crate::Api;
use anyhow::{ensure, Context, Result};
use flate2::write::GzEncoder;
use flate2::Compression;
use magicmtn_sdk::ExecResult;
use reqwest::Method;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use uuid::Uuid;

fn git(directory: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .output()
        .context("running Git for relay")?;
    ensure!(
        output.status.success(),
        "relay requires the current directory to be in a Git repository"
    );
    Ok(output.stdout)
}

fn repository_root(directory: &Path) -> Result<PathBuf> {
    ensure!(
        git(directory, &["rev-parse", "--is-inside-work-tree"])? == b"true\n",
        "relay requires the current directory to be in a Git repository"
    );
    let root = git(directory, &["rev-parse", "--show-toplevel"])?;
    let root = root
        .strip_suffix(b"\n")
        .context("invalid Git repository root")?;
    #[cfg(unix)]
    let root = {
        use std::os::unix::ffi::OsStrExt;
        Path::new(std::ffi::OsStr::from_bytes(root))
    };
    #[cfg(not(unix))]
    let root = Path::new(std::str::from_utf8(root)?);
    root.canonicalize().context("resolving Git repository root")
}

pub(super) fn start(api: &Api, id: Uuid, directory: &Path) -> Result<()> {
    let directory = repository_root(directory)?;
    let (log, log_path) = tempfile::Builder::new()
        .prefix("magic-relay-")
        .suffix(".log")
        .tempfile()?
        .keep()?;
    let mut command = transfer_command(api, id, &directory)?;
    command
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // The transfer survives terminal detach and the local Pi process exiting.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().context("starting background relay")?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    eprintln!("Copying your repo... Transfer log: {}", log_path.display());
    Ok(())
}

fn transfer_command(api: &Api, id: Uuid, directory: &Path) -> Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    if cfg!(feature = "development-server") {
        command.args(["--server-url", api.base.as_str()]);
    }
    command
        .args(["pi", "relay", &id.to_string()])
        .arg(directory);
    Ok(command)
}

fn archive(directory: &Path, destination: &Path) -> Result<()> {
    // Pi can run in any subdirectory. Keep paths relative to the repository root
    // so the working tree and its Git/Jujutsu metadata remain together.
    let root = repository_root(directory)?;
    let directory = root.as_path();
    let files = git(
        directory,
        &["ls-files", "--cached", "--stage", "-z", "--", "."],
    )?;
    let output = GzEncoder::new(fs::File::create(destination)?, Compression::fast());
    let mut archive = tar::Builder::new(output);
    archive.follow_symlinks(false);
    let mut seen = std::collections::HashSet::new();
    'files: for entry in files
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        // Gitlinks track a submodule revision, not its working-tree files.
        if entry.starts_with(b"160000 ") {
            continue;
        }
        let separator = entry
            .iter()
            .position(|byte| *byte == b'\t')
            .context("invalid Git index entry")?;
        let name = &entry[separator + 1..];
        #[cfg(unix)]
        let name = {
            use std::os::unix::ffi::OsStrExt;
            PathBuf::from(std::ffi::OsStr::from_bytes(name))
        };
        #[cfg(not(unix))]
        let name = PathBuf::from(std::str::from_utf8(name)?);
        if !seen.insert(name.clone()) {
            continue;
        }
        ensure!(
            name.components()
                .all(|part| matches!(part, std::path::Component::Normal(_))),
            "invalid tracked path"
        );
        // Never read through a directory replaced with a symlink: doing so could
        // upload files outside the repository that Git does not track.
        for parent in name
            .ancestors()
            .skip(1)
            .filter(|path| !path.as_os_str().is_empty())
        {
            let metadata = match fs::symlink_metadata(directory.join(parent)) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue 'files,
                Err(error) => return Err(error.into()),
            };
            ensure!(
                !metadata.file_type().is_symlink(),
                "tracked directory was replaced with a symlink: {}",
                parent.display()
            );
        }
        let source = directory.join(&name);
        let metadata = match fs::symlink_metadata(&source) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        // Skip a tracked file replaced locally with a directory.
        if metadata.is_dir() {
            continue;
        }
        ensure!(
            metadata.is_file() || metadata.file_type().is_symlink(),
            "unsupported tracked file: {}",
            name.display()
        );
        archive.append_path_with_name(&source, &name)?;
    }
    for name in [".git", ".jj"] {
        let source = directory.join(name);
        let metadata = match fs::symlink_metadata(&source) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if metadata.is_dir() {
            archive.append_dir_all(name, &source)?;
        } else {
            archive.append_path_with_name(&source, name)?;
        }
    }
    archive.into_inner()?.finish()?;
    Ok(())
}

async fn remote(api: &Api, id: Uuid, action: &str, name: &str, repository: &str) -> Result<()> {
    let body = crate::execution::request(
        include_str!("relay.py"),
        &[
            id.to_string(),
            action.to_owned(),
            name.to_owned(),
            repository.to_owned(),
        ],
        30_000,
    )?;
    let result: ExecResult = serde_json::from_value(
        api.request(Method::POST, &format!("v1/sandboxes/{id}/exec"), Some(body))
            .await?,
    )?;
    ensure!(
        !result.timed_out && !result.truncated && result.exit_code == Some(0),
        "relay {action} failed: {}",
        result.stderr
    );
    Ok(())
}

pub(super) async fn run(api: &Api, id: Uuid, directory: &Path) -> Result<()> {
    let root = repository_root(directory)?;
    let repository = root
        .file_name()
        .and_then(|name| name.to_str())
        .context("repository directory name must be valid UTF-8")?;
    // Workspace IDs are sandbox IDs, as used by the extension's sandbox picker.
    let name = format!(".magic-relay-{}.tar.gz", Uuid::new_v4());
    remote(api, id, "begin", &name, repository).await?;
    let result = async {
        let temporary = tempfile::tempdir()?;
        let path = temporary.path().join(&name);
        archive(&root, &path)?;
        // Share the transport used by `magic sandbox scp`.
        crate::sandbox_scp::run(api, &id.to_string(), &path).await?;
        remote(api, id, "extract", &name, repository).await
    }
    .await;
    if result.is_err() {
        // Retain a visible failure in Pi instead of silently clearing the notice.
        let _ = remote(api, id, "fail", &name, repository).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;

    #[test]
    fn background_command_is_accepted_by_this_cli_build() -> Result<()> {
        use clap::Parser;
        let server = if cfg!(feature = "development-server") {
            "http://127.0.0.1:4321/"
        } else {
            "https://api.magicmtn.ai/"
        };
        let api = Api::new(server)?;
        let id = Uuid::new_v4();
        let directory = Path::new("/working tree");
        let command = transfer_command(&api, id, directory)?;
        let parsed = crate::Cli::try_parse_from(
            std::iter::once(std::ffi::OsStr::new("magic")).chain(command.get_args()),
        )?;
        assert_eq!(
            parsed.server_url().trim_end_matches('/'),
            server.trim_end_matches('/')
        );
        assert!(matches!(parsed.command, Some(crate::Commands::Pi {
            command: super::super::Commands::Relay { id: parsed_id, directory: parsed_directory }
        }) if parsed_id == id && parsed_directory == directory));
        Ok(())
    }

    #[test]
    fn archive_copies_all_tracked_working_files_from_a_subdirectory() -> Result<()> {
        let directory = tempfile::tempdir()?;
        git(directory.path(), &["init", "-q"])?;
        fs::create_dir_all(directory.path().join("sub/deleted"))?;
        for (name, data) in [
            ("outside", "outside"),
            ("sub/modified", "original"),
            ("sub/staged", "staged"),
            ("sub/deleted/file", "deleted"),
            ("sub/line\nbreak 雪", "odd filename"),
        ] {
            fs::write(directory.path().join(name), data)?;
        }
        git(directory.path(), &["add", "."])?;
        fs::write(directory.path().join("sub/modified"), "unstaged change")?;
        fs::write(
            directory.path().join("sub/staged"),
            "working copy after staging",
        )?;
        fs::write(directory.path().join("sub/untracked"), "must not copy")?;
        fs::remove_dir_all(directory.path().join("sub/deleted"))?;
        git(
            directory.path(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                "160000,1111111111111111111111111111111111111111,sub/module",
            ],
        )?;
        fs::create_dir(directory.path().join("sub/module"))?;
        fs::write(
            directory.path().join("sub/module/file"),
            "submodule contents",
        )?;
        let output = tempfile::NamedTempFile::new()?;
        archive(&directory.path().join("sub"), output.path())?;
        let extracted = tempfile::tempdir()?;
        tar::Archive::new(GzDecoder::new(fs::File::open(output.path())?))
            .unpack(extracted.path())?;
        for (name, contents) in [
            ("outside", "outside"),
            ("sub/modified", "unstaged change"),
            ("sub/staged", "working copy after staging"),
            ("sub/line\nbreak 雪", "odd filename"),
        ] {
            assert_eq!(fs::read_to_string(extracted.path().join(name))?, contents);
        }
        assert!(extracted.path().join(".git/index").is_file());
        for name in ["sub/deleted", "sub/untracked", "sub/module"] {
            assert!(!extracted.path().join(name).exists());
        }
        Ok(())
    }

    #[test]
    fn archive_preserves_repository_metadata() -> Result<()> {
        let directory = tempfile::tempdir()?;
        git(directory.path(), &["init", "-q"])?;
        fs::write(directory.path().join("tracked"), "committed")?;
        git(directory.path(), &["add", "."])?;
        git(
            directory.path(),
            &[
                "-c",
                "user.name=Relay Test",
                "-c",
                "user.email=relay@example.com",
                "commit",
                "-qm",
                "initial",
            ],
        )?;
        git(directory.path(), &["checkout", "-qb", "relay-branch"])?;
        fs::write(directory.path().join("tracked"), "staged")?;
        git(directory.path(), &["add", "tracked"])?;
        fs::write(directory.path().join("tracked"), "modified")?;
        fs::create_dir_all(directory.path().join(".jj/repo/store"))?;
        fs::create_dir_all(directory.path().join(".jj/working_copy"))?;
        let state = [0, 255, 1, 128];
        fs::write(directory.path().join(".jj/repo/store/state"), state)?;
        fs::write(directory.path().join("untracked"), "excluded")?;
        let output = tempfile::NamedTempFile::new()?;
        fs::create_dir(directory.path().join("sub"))?;
        archive(&directory.path().join("sub"), output.path())?;
        let extracted = tempfile::tempdir()?;
        tar::Archive::new(GzDecoder::new(fs::File::open(output.path())?))
            .unpack(extracted.path())?;
        assert_eq!(
            git(extracted.path(), &["show", "HEAD:tracked"])?,
            b"committed"
        );
        assert_eq!(
            git(extracted.path(), &["diff", "--", "tracked"])?,
            git(directory.path(), &["diff", "--", "tracked"])?
        );
        assert_eq!(
            git(extracted.path(), &["diff", "--cached"])?,
            git(directory.path(), &["diff", "--cached"])?
        );
        assert_eq!(
            git(extracted.path(), &["branch", "--show-current"])?,
            b"relay-branch\n"
        );
        assert_eq!(
            fs::read(extracted.path().join(".jj/repo/store/state"))?,
            state
        );
        assert!(extracted.path().join(".jj/working_copy").is_dir());
        assert!(!extracted.path().join("untracked").exists());
        Ok(())
    }

    #[test]
    fn archive_requires_a_git_working_tree() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let output = tempfile::NamedTempFile::new()?;
        assert!(archive(directory.path(), output.path()).is_err());
        git(directory.path(), &["init", "--bare", "-q"])?;
        assert!(archive(directory.path(), output.path()).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn archive_preserves_file_modes_and_symlinks_without_reading_their_targets() -> Result<()> {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let directory = tempfile::tempdir()?;
        git(directory.path(), &["init", "-q"])?;
        fs::write(directory.path().join("executable"), "#!/bin/sh\n")?;
        fs::set_permissions(
            directory.path().join("executable"),
            fs::Permissions::from_mode(0o755),
        )?;
        symlink("executable", directory.path().join("link"))?;
        git(directory.path(), &["add", "."])?;
        let output = tempfile::NamedTempFile::new()?;
        archive(directory.path(), output.path())?;
        let extracted = tempfile::tempdir()?;
        tar::Archive::new(GzDecoder::new(fs::File::open(output.path())?))
            .unpack(extracted.path())?;
        assert_eq!(
            fs::read_link(extracted.path().join("link"))?,
            Path::new("executable")
        );
        assert_eq!(
            fs::metadata(extracted.path().join("executable"))?
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn archive_rejects_tracked_directories_replaced_with_symlinks() -> Result<()> {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        git(directory.path(), &["init", "-q"])?;
        fs::create_dir(directory.path().join("tracked"))?;
        fs::write(directory.path().join("tracked/secret"), "original")?;
        git(directory.path(), &["add", "."])?;
        fs::remove_dir_all(directory.path().join("tracked"))?;
        fs::write(outside.path().join("secret"), "untracked secret")?;
        symlink(outside.path(), directory.path().join("tracked"))?;
        let output = tempfile::NamedTempFile::new()?;
        assert!(archive(directory.path(), output.path()).is_err());
        Ok(())
    }
}
