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

pub(super) fn start(api: &Api, id: Uuid, directory: &Path) -> Result<()> {
    ensure!(
        git(directory, &["rev-parse", "--is-inside-work-tree"])? == b"true\n",
        "relay requires the current directory to be in a Git repository"
    );
    let directory = directory.canonicalize()?;
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
    eprintln!(
        "Copying tracked local files and repository metadata in the background. Transfer log: {}",
        log_path.display()
    );
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
    ensure!(
        git(directory, &["rev-parse", "--is-inside-work-tree"])? == b"true\n",
        "relay requires the current directory to be in a Git repository"
    );
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

async fn remote(api: &Api, id: Uuid, action: &str, name: &str) -> Result<()> {
    let body = crate::execution::request(
        include_str!("relay.py"),
        &[id.to_string(), action.to_owned(), name.to_owned()],
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
    // Workspace IDs are sandbox IDs, as used by the extension's sandbox picker.
    let name = format!(".magic-relay-{}.tar.gz", Uuid::new_v4());
    remote(api, id, "begin", &name).await?;
    let result = async {
        let temporary = tempfile::tempdir()?;
        let path = temporary.path().join(&name);
        archive(directory, &path)?;
        // Share the transport used by `magic sandbox scp`.
        crate::sandbox_scp::run(api, &id.to_string(), &path).await?;
        remote(api, id, "extract", &name).await
    }
    .await;
    if result.is_err() {
        // Retain a visible failure in Pi instead of silently clearing the notice.
        let _ = remote(api, id, "fail", &name).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::Read;

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
    fn archive_copies_working_files_and_only_tracked_paths_in_the_current_directory() -> Result<()>
    {
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
        let mut contents = std::collections::BTreeMap::new();
        for entry in tar::Archive::new(GzDecoder::new(fs::File::open(output.path())?)).entries()? {
            let mut entry = entry?;
            let name = entry.path()?.into_owned();
            let mut value = String::new();
            entry.read_to_string(&mut value)?;
            contents.insert(name, value);
        }
        assert_eq!(
            contents,
            [
                (PathBuf::from("modified"), "unstaged change".into()),
                (PathBuf::from("staged"), "working copy after staging".into()),
                (PathBuf::from("line\nbreak 雪"), "odd filename".into()),
            ]
            .into()
        );
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
        fs::write(directory.path().join("tracked"), "modified")?;
        fs::create_dir_all(directory.path().join(".jj/repo/store"))?;
        fs::create_dir_all(directory.path().join(".jj/working_copy"))?;
        let state = [0, 255, 1, 128];
        fs::write(directory.path().join(".jj/repo/store/state"), state)?;
        fs::write(directory.path().join("untracked"), "excluded")?;
        let output = tempfile::NamedTempFile::new()?;
        archive(directory.path(), output.path())?;
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
