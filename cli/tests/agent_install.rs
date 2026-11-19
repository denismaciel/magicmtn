#![cfg(feature = "development-server")]

use axum::{
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

fn save_login(home: &std::path::Path, base: &str) {
    let directory = home.join(".config/magicmtn/cli");
    fs::create_dir_all(&directory).unwrap();
    let digest = Sha256::digest(format!("{base}/").as_bytes());
    fs::write(
        directory.join(format!("{digest:x}.json")),
        json!({"server":format!("{base}/"),"token":"cli-test-session"}).to_string(),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_installs_local_definition_and_canonical_agents_with_saved_session(
) -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let project = root.path().join("error-investigator");
    fs::create_dir_all(project.join("langchain_core/language_models"))?;
    fs::write(project.join("langchain_core/__init__.py"), "")?;
    fs::write(
        project.join("langchain_core/language_models/__init__.py"),
        "",
    )?;
    fs::write(
        project.join("langchain_core/language_models/fake_chat_models.py"),
        "class FakeListChatModel:\n    def __init__(self, responses): self.responses = responses\n",
    )?;
    fs::write(
        project.join("agent.py"),
        "import magicmtn.agents as magicmtn\n"
            .to_owned()
            + "@magicmtn.agent(sdk='langchain', default_model='kimi-k3', mcps=[magicmtn.MCP('grafana')])\n"
            + "def agent(ctx):\n"
            + "    class Graph:\n"
            + "        name = 'Error Investigator'\n"
            + "        def invoke(self, value): return value\n"
            + "    return Graph()\n",
    )?;
    fs::write(
        project.join("pyproject.toml"),
        "[project]\nname='error-investigator'\n",
    )?;
    let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
    let installs = Arc::new(AtomicUsize::new(0));
    let listed = installs.clone();
    let posted = installs.clone();
    let app = Router::new().route(
        "/v1/code-agents",
        get(move |headers: HeaderMap| {
            let listed = listed.clone();
            async move {
                assert_eq!(headers["authorization"], "Bearer cli-test-session");
                let agents = if listed.load(Ordering::SeqCst) == 0 {
                    json!([])
                } else {
                    json!([{"id":"11111111-1111-4111-8111-111111111111","name":"error investigator","sdk":"langchain"}])
                };
                Json(json!({"agents":agents}))
            }
        }),
    ).route(
        "/v1/agents/install",
        post(move |headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer cli-test-session");
            if let Some(definition) = body.get("definition") {
                assert_eq!(definition["name"], "Error Investigator");
                assert_eq!(definition["mcps"][0]["provider"], "grafana");
                assert_eq!(definition["source_files"]["pyproject.toml"], "[project]\nname='error-investigator'\n");
                let number = posted.fetch_add(1, Ordering::SeqCst);
                if number == 0 {
                    assert_eq!(body["create_only"], true);
                    assert!(body["expected_agent_id"].is_null());
                } else {
                    assert_eq!(body["create_only"], false);
                    assert_eq!(body["expected_agent_id"], "11111111-1111-4111-8111-111111111111");
                }
                (StatusCode::OK, Json(json!({"id":"11111111-1111-4111-8111-111111111111","version":definition["version"]})))
            } else {
                let name = match body["canonical"].as_str() {
                    Some("pi") => "Pi",
                    Some("opencode") => "OpenCode",
                    other => panic!("unexpected canonical agent {other:?}"),
                };
                (StatusCode::OK, Json(json!({"id":"22222222-2222-4222-8222-222222222222","name":name})))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = receiver.await;
            })
            .await
    });
    let base = format!("http://{address}");
    let home = root.path().join("home");
    fs::create_dir_all(&home)?;
    save_login(&home, &base);
    let install = |target: &str, answer: Option<&str>| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_magic"))
            .args(["--server-url", &base, "agents", "install", target])
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", root.path().join("xdg"))
            .env("PI_CODING_AGENT_DIR", root.path().join("pi-home"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(answer) = answer {
            child.stdin.take().unwrap().write_all(answer.as_bytes())?;
        }
        child.wait_with_output()
    };
    let code = install(project.to_str().unwrap(), Some("yes\n"))?;
    assert!(
        code.status.success(),
        "{}",
        String::from_utf8_lossy(&code.stderr)
    );
    assert!(String::from_utf8_lossy(&code.stdout).contains("A new agent named"));
    let declined = install(project.to_str().unwrap(), Some("\n"))?;
    assert!(declined.status.success());
    assert!(String::from_utf8_lossy(&declined.stdout).contains("will overwrite the existing agent"));
    assert!(String::from_utf8_lossy(&declined.stdout).contains("Installation cancelled"));
    assert_eq!(installs.load(Ordering::SeqCst), 1);
    let replaced = install(project.to_str().unwrap(), Some("y\n"))?;
    assert!(
        replaced.status.success(),
        "{}",
        String::from_utf8_lossy(&replaced.stderr)
    );
    assert_eq!(installs.load(Ordering::SeqCst), 2);
    let pi = install("pi", None)?;
    assert!(
        pi.status.success(),
        "{}",
        String::from_utf8_lossy(&pi.stderr)
    );
    assert!(root
        .path()
        .join("pi-home/extensions/magicmtn/index.js")
        .is_file());
    let opencode = install("opencode", None)?;
    assert!(
        opencode.status.success(),
        "{}",
        String::from_utf8_lossy(&opencode.stderr)
    );
    let plugin = root.path().join("xdg/opencode/plugins/magicmtn");
    for file in [
        "index.js",
        "package.json",
        "tui.js",
        "magic.js",
        "config.js",
    ] {
        assert!(plugin.join(file).is_file(), "missing {file}");
    }
    let legacy = Command::new(env!("CARGO_BIN_EXE_magic"))
        .args(["--server-url", &base, "pi", "install"])
        .env("HOME", &home)
        .env("PI_CODING_AGENT_DIR", root.path().join("pi-home"))
        .output()?;
    assert!(
        legacy.status.success(),
        "{}",
        String::from_utf8_lossy(&legacy.stderr)
    );
    let _ = sender.send(());
    server.await??;
    Ok(())
}
