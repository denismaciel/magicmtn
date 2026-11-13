use std::process::{Command, Output};

fn magic(args: &[&str], server: Option<&str>) -> Output {
    let home = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_magic"));
    command
        .args(args)
        .env("HOME", home.path())
        .env_remove("MAGICMTN_SERVER_URL")
        .env_remove("MAGICMTN_CA_CERT")
        .env_remove("MAGICMTN_CONFIG_DIRECTORY")
        .env_remove("MAGICMTN_LOCAL_DIRECTORY")
        .env("MAGICMTN_ORGANIZATION_ID", "not-a-uuid");
    if let Some(server) = server {
        command.env("MAGICMTN_SERVER_URL", server);
    }
    command.output().unwrap()
}

#[test]
fn server_override_is_available_in_every_build() {
    let output = magic(&["--server-url", "http://127.0.0.1:8080", "whoami"], None);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not signed in"));
}

#[test]
fn explicit_server_takes_precedence_over_environment() {
    let overridden = magic(&["whoami"], Some("not-a-url"));
    assert!(String::from_utf8_lossy(&overridden.stderr).contains("relative URL"));
    let explicit = magic(
        &["--server-url", "http://127.0.0.1:8080", "whoami"],
        Some("not-a-url"),
    );
    assert!(String::from_utf8_lossy(&explicit.stderr).contains("not signed in"));
}

#[test]
fn local_selection_overrides_the_server_environment() {
    let output = magic(&["--local", "sandbox", "list"], Some("not-a-url"));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("magic server start"));
}

#[test]
fn help_and_completions_expose_server_selection() {
    for args in [vec!["--help"], vec!["completion", "bash", "--print"]] {
        let output = magic(&args, None);
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(!text.contains("--organization"));
        assert!(text.contains("--server-url"));
        assert!(text.contains("--local"));
    }
}

#[test]
fn organization_override_is_rejected() {
    let output = magic(
        &[
            "--organization",
            "11111111-1111-4111-8111-111111111111",
            "whoami",
        ],
        None,
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unexpected argument '--organization'")
    );
}

#[test]
fn sandbox_help_remains_available_without_sign_in() {
    for args in [
        vec!["help"],
        vec!["help", "sandbox"],
        vec!["sandbox", "--help"],
    ] {
        let output = magic(&args, None);
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout)
            .contains("Manage standalone and session sandboxes"));
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn agent_install_requires_cli_sign_in() {
    for target in ["pi", "./my-agent"] {
        let output = magic(&["agents", "install", target], None);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("sign in first with `magic login`"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
