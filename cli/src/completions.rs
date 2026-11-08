use clap::{Command, ValueEnum};
use clap_complete::Shell;
use std::path::{Path, PathBuf};

pub(crate) fn fish_directory(home: &Path) -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    config.join("fish/completions")
}

pub(crate) fn generate(shell: Shell, command: &mut Command) -> Vec<u8> {
    let mut script = Vec::new();
    if shell == Shell::Fish {
        // Sourcing an updated file must replace the shell's loaded definitions.
        script.extend_from_slice(b"complete -c magic -e\n");
    }
    clap_complete::generate(shell, command, "magic", &mut script);
    if shell == Shell::Fish {
        // clap_complete's static Fish generator omits positional enum values,
        // and its root help guard excludes `help` itself, hiding all candidates.
        // Add these missing contexts using the CLI's own command/value metadata.
        script.extend_from_slice(
            br#"
function __fish_magic_at_path
    set -l expected $argv[1]
    set -l tokens (commandline -opc)
    set -e tokens[1]
    argparse (__fish_magic_global_optspecs) print -- $tokens 2>/dev/null
    or return 1
    set -q argv[1]; or return 1
    test (string join / -- $argv) = "$expected"
end
"#,
        );
        for value in crate::CompletionShell::value_variants() {
            let value = value.to_possible_value().expect("visible shell");
            script.extend_from_slice(
                format!(
                    "complete -c magic -n '__fish_magic_at_path completion' -f -a '{}'\n",
                    value.get_name()
                )
                .as_bytes(),
            );
        }
        for subcommand in command.get_subcommands().filter(|cmd| !cmd.is_hide_set()) {
            let help = subcommand
                .get_about()
                .map(ToString::to_string)
                .unwrap_or_default();
            let help = help
                .replace('\\', "\\\\")
                .replace('\'', "\\'")
                .replace('\n', " ");
            script.extend_from_slice(
                format!(
                    "complete -c magic -n '__fish_magic_at_path help' -f -a '{}' -d '{}'\n",
                    subcommand.get_name(),
                    help
                )
                .as_bytes(),
            );
        }
    }
    script
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use std::{io::Write, process::Command as Process};

    #[test]
    fn fish_completes_commands_help_and_values_after_reload() -> anyhow::Result<()> {
        if Process::new("fish").arg("--version").output().is_err() {
            return Ok(()); // Fish is optional on developer and CI machines.
        }
        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(&generate(Shell::Fish, &mut crate::Cli::command()))?;
        for (line, expected) in [
            (
                "magic ",
                vec![
                    "update",
                    "whoami",
                    "pi",
                    "agents",
                    "provider",
                    "sandbox",
                    "completion",
                ],
            ),
            (
                "magic help ",
                vec![
                    "update",
                    "whoami",
                    "pi",
                    "agents",
                    "provider",
                    "sandbox",
                    "completion",
                ],
            ),
            ("magic whoami ", vec!["plan"]),
            ("magic help whoami ", vec!["plan"]),
            ("magic completion ", vec!["bash", "fish", "zsh"]),
            ("magic completion --print ", vec!["bash", "fish", "zsh"]),
            ("magic pi ", vec!["install", "defaults", "attach", "create"]),
            ("magic agents ", vec!["install"]),
            (
                "magic help pi ",
                vec!["install", "defaults", "attach", "create"],
            ),
            ("magic sandbox show --format ", vec!["human", "json"]),
        ] {
            let output = Process::new("fish").args([
                "--no-config", "-c",
                "complete -c magic -f -a obsolete; source $argv[1]; source $argv[1]; complete -C $argv[2]",
                "--",
            ]).arg(file.path()).arg(line).output()?;
            assert!(output.status.success(), "{line}: {:?}", output);
            assert!(
                output.stderr.is_empty(),
                "{line}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8(output.stdout)?;
            let candidates: Vec<_> = text
                .lines()
                .map(|line| line.split('\t').next().unwrap())
                .collect();
            assert!(
                !candidates.contains(&"obsolete"),
                "stale completion survived reload"
            );
            if line == "magic " || line == "magic help " {
                assert!(
                    !candidates.contains(&"plan"),
                    "plan must be nested under whoami"
                );
            }
            for value in expected {
                assert!(
                    candidates.contains(&value),
                    "{line}: missing {value}: {text}"
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[test]
fn bash_completion_generates() {
    use clap::CommandFactory;
    assert!(!generate(Shell::Bash, &mut crate::Cli::command()).is_empty());
}
