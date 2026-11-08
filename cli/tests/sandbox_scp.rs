#![cfg(feature = "development-server")]

use axum::extract::{Path as RoutePath, State};
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const SANDBOX: &str = "11111111-1111-4111-8111-111111111111";

#[test]
fn destination_respects_image_workdir_and_workspace_fallback() {
    let output = Command::new("python3")
        .args([
            "-c",
            r#"
import sys
from pathlib import Path
from unittest.mock import patch
namespace = {"__name__": "sandbox_scp_test"}
exec(sys.argv[1], namespace)
for cwd, workspace_exists, expected in [
    ("/workspace", True, "/workspace"),
    ("/app", True, "/app"),
    ("/", True, "/workspace"),
    ("/home/agent", True, "/workspace"),
    ("/home/agent", False, "/home/agent"),
    ("/", False, "/"),
]:
    with patch.object(Path, "cwd", return_value=Path(cwd)), \
         patch.object(Path, "home", return_value=Path("/home/agent")), \
         patch.object(Path, "is_dir", return_value=workspace_exists):
        assert namespace["default_directory"]() == Path(expected)
"#,
            include_str!("../src/sandbox_scp.py"),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[derive(Clone)]
struct Remote {
    directory: PathBuf,
    requests: Arc<AtomicUsize>,
    fail_transfer: bool,
}

fn authenticate(headers: &HeaderMap) {
    assert_eq!(headers["authorization"], "Bearer upload-session");
}

async fn show(headers: HeaderMap) -> Json<Value> {
    authenticate(&headers);
    Json(json!({"id":SANDBOX,"status":"running"}))
}

async fn execute(
    State(remote): State<Remote>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    authenticate(&headers);
    assert_eq!(id, SANDBOX);
    assert_eq!(body["command"], "python3");
    let args: Vec<String> = serde_json::from_value(body["args"].clone()).unwrap();
    assert!(args.len() <= 32);
    assert!(args.iter().all(|arg| arg.len() <= 2048));
    let request = remote.requests.fetch_add(1, Ordering::SeqCst);
    if remote.fail_transfer && request == 2 {
        return Json(json!({
            "stdout":"", "stderr":"simulated full disk", "exit_code":1,
            "timed_out":false,"truncated":false,"elapsed_ms":0,
        }));
    }
    let output = tokio::task::spawn_blocking(move || {
        Command::new("python3")
            .args(args)
            .current_dir(remote.directory)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    Json(json!({
        "stdout":String::from_utf8(output.stdout).unwrap(),
        "stderr":String::from_utf8(output.stderr).unwrap(),
        "exit_code":output.status.code(),
        "timed_out":false,"truncated":false,"elapsed_ms":0,
    }))
}

async fn copy(contents: &[u8], fail_transfer: bool) -> anyhow::Result<()> {
    let guest = tempfile::tempdir()?;
    let local = tempfile::tempdir()?;
    let home = tempfile::tempdir()?;
    let name = "report 雪 ' ; $(touch injected).bin";
    let source = local.path().join(name);
    fs::write(&source, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&source, fs::Permissions::from_mode(0o640))?;
    }
    let destination = guest.path().join(name);
    fs::write(&destination, b"previous contents")?;
    let requests = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/v1/sandboxes/:id", get(show))
        .route("/v1/sandboxes/:id/exec", axum::routing::post(execute))
        .with_state(Remote {
            directory: guest.path().into(),
            requests: requests.clone(),
            fail_transfer,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}/", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let credentials = home.path().join(".config/magicmtn/cli");
    fs::create_dir_all(&credentials)?;
    fs::write(
        credentials.join(format!("{:x}.json", Sha256::digest(base.as_bytes()))),
        json!({"server":base,"token":"upload-session"}).to_string(),
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_magic"))
        .args(["--server-url", &base, "sandbox", "scp", "11111111"])
        .arg(&source)
        .env("HOME", home.path())
        .env_remove("MAGICMTN_CA_CERT")
        .output()?;
    server.abort();
    if fail_transfer {
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("simulated full disk"));
        assert_eq!(fs::read(&destination)?, b"previous contents");
        assert_eq!(requests.load(Ordering::SeqCst), 4);
    } else {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(&destination)?, contents);
        assert!(String::from_utf8_lossy(&output.stdout).contains(destination.to_str().unwrap()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&destination)?.permissions().mode() & 0o777,
                0o640
            );
        }
    }
    assert_eq!(fs::read_dir(guest.path())?.count(), 1);
    assert!(!guest.path().join("injected").exists());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn copies_binary_and_empty_files_to_the_image_working_directory() -> anyhow::Result<()> {
    let contents: Vec<u8> = (0..70_000).map(|index| (index % 256) as u8).collect();
    copy(&contents, false).await?;
    copy(&[], false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_upload_preserves_the_destination_and_removes_partial_data() -> anyhow::Result<()> {
    copy(&vec![42; 70_000], true).await
}
