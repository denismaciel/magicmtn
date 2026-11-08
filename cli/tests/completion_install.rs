use std::{fs, process::Command};

#[test]
fn automatic_completion_installs_for_each_supported_shell_and_is_repeatable() {
    for shell in ["bash", "zsh", "fish"] {
        let home = tempfile::tempdir().unwrap();
        if shell == "zsh" {
            fs::write(
                home.path().join(".zshrc"),
                "autoload -Uz compinit\ncompinit\n",
            )
            .unwrap();
        }
        let run = || {
            let output = Command::new(env!("CARGO_BIN_EXE_magic"))
                .args(["completion", "--yes"])
                .env("HOME", home.path())
                .env("SHELL", format!("/usr/bin/{shell}"))
                .env_remove("XDG_CONFIG_HOME")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{shell}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run();
        run();
        let path = match shell {
            "bash" => home
                .path()
                .join(".local/share/magic/completions/magic.bash"),
            "zsh" => home.path().join(".local/share/magic/completions/_magic"),
            "fish" => home.path().join(".config/fish/completions/magic.fish"),
            _ => unreachable!(),
        };
        assert!(!fs::read(&path).unwrap().is_empty());
        match shell {
            "bash" => {
                let rc = fs::read_to_string(home.path().join(if cfg!(target_os = "macos") {
                    ".bash_profile"
                } else {
                    ".bashrc"
                }))
                .unwrap();
                assert_eq!(rc.matches("source '").count(), 1);
            }
            "zsh" => {
                let rc = fs::read_to_string(home.path().join(".zshrc")).unwrap();
                assert!(rc.lines().next().unwrap().starts_with("fpath=("));
                assert_eq!(rc.matches("fpath=(").count(), 1);
                assert!(rc.contains("autoload -Uz compinit\ncompinit"));
            }
            "fish" => {}
            _ => unreachable!(),
        }
    }
}
