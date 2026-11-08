//! Remove one CLI installation and the local integration files it sets up.
use crate::{authentication, Api};
use anyhow::{Context, Result};
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

struct ShellEdit {
    path: PathBuf,
    remove_lines: Vec<String>,
    remove_if_empty: bool,
}

struct Installation {
    executable: PathBuf,
    completions: Vec<PathBuf>,
    shell_edits: Vec<ShellEdit>,
    pi_extension: PathBuf,
}

fn configured_directory(variable: &str, fallback: PathBuf) -> PathBuf {
    std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or(fallback)
}

fn executable_path() -> Result<PathBuf> {
    let executable = std::env::current_exe().context("locating the running CLI")?;
    let invoked = PathBuf::from(
        std::env::args_os()
            .next()
            .context("missing executable name")?,
    );
    let candidates = if invoked.components().count() > 1 || invoked.is_absolute() {
        vec![std::path::absolute(invoked)?]
    } else {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|directory| directory.join(&invoked))
            .collect()
    };
    // Removing a development symlink must leave the repository's binary intact.
    // Verify argv[0]/PATH candidates against the running executable first.
    for candidate in candidates {
        if candidate.canonicalize().ok().as_ref() == Some(&executable) {
            return std::path::absolute(candidate).context("resolving the CLI installation path");
        }
    }
    Ok(executable)
}

fn directory_will_be_empty(directory: &Path, executable: &Path) -> Result<bool> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error).context("checking whether the CLI directory is shared"),
    };
    for entry in entries {
        if entry?.path() != executable {
            return Ok(false);
        }
    }
    Ok(true)
}

impl Installation {
    fn new(
        home: &Path,
        executable: PathBuf,
        zsh: PathBuf,
        config: PathBuf,
        pi: PathBuf,
    ) -> Result<Self> {
        let completion_directory = home.join(".local/share/magic/completions");
        let bash_completion = completion_directory.join("magic.bash");
        let quote = |path: &Path| path.to_string_lossy().replace('\'', "'\\''");
        let mut remove_lines = vec![
            format!("source '{}'", quote(&bash_completion)),
            format!("fpath=('{}' $fpath)", quote(&completion_directory)),
        ];
        let mut fish_lines = Vec::new();
        let mut directories = vec![home.join(".local/bin")];
        if let Some(directory) = executable.parent() {
            directories.push(directory.to_owned());
        }
        directories.sort();
        directories.dedup();
        for directory in directories {
            // Retain PATH entries when another tool uses this directory.
            if directory_will_be_empty(&directory, &executable)? {
                remove_lines.push(format!("export PATH='{}':\"$PATH\"", quote(&directory)));
                let quoted = directory
                    .to_string_lossy()
                    .replace('\\', "\\\\")
                    .replace('\'', "\\'");
                fish_lines.push(format!("fish_add_path -- '{quoted}'"));
            }
        }
        let mut shell_edits: Vec<_> = [".bashrc", ".bash_profile", ".bash_login", ".profile"]
            .iter()
            .map(|name| ShellEdit {
                path: home.join(name),
                remove_lines: remove_lines.clone(),
                remove_if_empty: false,
            })
            .collect();
        for directory in [home.to_owned(), zsh] {
            let path = directory.join(".zshrc");
            if !shell_edits.iter().any(|edit| edit.path == path) {
                shell_edits.push(ShellEdit {
                    path,
                    remove_lines: remove_lines.clone(),
                    remove_if_empty: false,
                });
            }
        }
        shell_edits.push(ShellEdit {
            path: config.join("fish/conf.d/magic-path.fish"),
            remove_lines: fish_lines,
            remove_if_empty: true,
        });
        Ok(Self {
            executable,
            completions: vec![
                bash_completion,
                completion_directory.join("_magic"),
                config.join("fish/completions/magic.fish"),
            ],
            shell_edits,
            pi_extension: pi.join("extensions/magicmtn"),
        })
    }

    fn remove(&self) -> Result<()> {
        for edit in &self.shell_edits {
            edit.apply()
                .with_context(|| format!("cleaning shell configuration {}", edit.path.display()))?;
        }
        for path in &self.completions {
            remove_file_if_present(path)?;
        }
        match fs::symlink_metadata(&self.pi_extension) {
            Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&self.pi_extension)?,
            Ok(_) => fs::remove_file(&self.pi_extension)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error).context("removing the Pi extension"),
        }
        // Keep the CLI available for a retry if any earlier cleanup fails.
        remove_file_if_present(&self.executable)
    }
}

impl ShellEdit {
    fn apply(&self) -> Result<()> {
        let original = match fs::read_to_string(&self.path) {
            Ok(original) => original,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let updated: String = original
            .split_inclusive('\n')
            .filter(|line| {
                !self
                    .remove_lines
                    .iter()
                    .any(|remove| line.trim_end_matches(['\r', '\n']) == remove)
            })
            .collect();
        if updated == original {
            return Ok(());
        }
        if self.remove_if_empty && updated.trim().is_empty() {
            return remove_file_if_present(&self.path);
        }
        // Preserve dotfile symlinks and permissions, and replace content atomically.
        let target = self.path.canonicalize()?;
        let mut staged = tempfile::NamedTempFile::new_in(
            target
                .parent()
                .context("missing shell configuration directory")?,
        )?;
        staged
            .as_file()
            .set_permissions(fs::metadata(&target)?.permissions())?;
        staged.write_all(updated.as_bytes())?;
        staged.flush()?;
        staged.persist(&target)?;
        Ok(())
    }
}

fn remove_file_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("removing {}", path.display())),
    }
}

fn confirm(input: &mut impl BufRead, output: &mut impl Write) -> Result<bool> {
    write!(
        output,
        "Uninstall MagicMtn CLI and its shell/Pi integration? [y/N] "
    )?;
    output.flush()?;
    let mut answer = String::new();
    input.read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

pub(crate) async fn run(server: &str) -> Result<()> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);
    let installation = Installation::new(
        &home,
        executable_path()?,
        configured_directory("ZDOTDIR", home.clone()),
        configured_directory("XDG_CONFIG_HOME", home.join(".config")),
        configured_directory("PI_CODING_AGENT_DIR", home.join(".pi/agent")),
    )?;
    println!(
        "This will sign out, remove {}, clean up MagicMtn shell completions, and remove {}.",
        installation.executable.display(),
        installation.pi_extension.display()
    );
    println!("Shared PATH entries, other Pi extensions, and your sandboxes are kept.");
    let mut terminal = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .context("uninstall requires an interactive terminal for confirmation")?;
    if !confirm(
        &mut std::io::BufReader::new(terminal.try_clone()?),
        &mut terminal,
    )? {
        println!("Uninstall cancelled.");
        return Ok(());
    }
    let mut api = Api::new(server)?;
    api.token = authentication::Credentials::new()?.load(&api.base)?;
    authentication::logout(&api)
        .await
        .context("could not sign out; no installation files were removed")?;
    installation.remove()?;
    println!(
        "MagicMtn CLI uninstalled. Restart your shell and run /reload in Pi to finish cleanup."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_requires_an_explicit_yes() -> Result<()> {
        for (answer, accepted) in [
            ("y\n", true),
            (" YES \n", true),
            ("\n", false),
            ("", false),
            ("no\n", false),
        ] {
            assert_eq!(confirm(&mut answer.as_bytes(), &mut Vec::new())?, accepted);
        }
        Ok(())
    }
}
