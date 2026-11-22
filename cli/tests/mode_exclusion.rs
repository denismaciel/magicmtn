use std::fs::{self, File};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn magic(home: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_magic"));
    command
        .env("HOME", home)
        .env_remove("MAGICMTN_CONFIG_DIRECTORY")
        .env_remove("MAGICMTN_LOCAL_DIRECTORY")
        .env_remove("MAGICMTN_SERVER_URL")
        .env_remove("MAGICMTN_DEPLOYMENT_MODE")
        .env_remove("MAGICMTN_LOCAL_DATABASE_URL");
    command
}

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn running_server_blocks_login_and_stop_releases_locks() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".config/magicmtn/local");
    let credentials = home.path().join(".config/magicmtn/cli");
    fs::create_dir_all(&credentials).unwrap();
    use sha2::{Digest, Sha256};
    let developer = "http://127.0.0.1:0/";
    fs::write(
        credentials.join(format!("{:x}.json", Sha256::digest(developer.as_bytes()))),
        serde_json::json!({"server":developer,"token":"developer-session"}).to_string(),
    )
    .unwrap();
    let mut server = Server(
        magic(home.path())
            .args(["server", "start", "--bind", "127.0.0.1:0"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while !directory.join("connection.json").exists() {
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "server exited during startup"
        );
        assert!(Instant::now() < deadline, "server startup timed out");
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(magicmtn_local::is_running(&directory).unwrap());
    let account = magic(home.path()).args(["pi", "account"]).output().unwrap();
    assert!(
        account.status.success(),
        "{}",
        String::from_utf8_lossy(&account.stderr)
    );
    let account: serde_json::Value = serde_json::from_slice(&account.stdout).unwrap();
    assert_eq!(account["mode"], "local");
    assert_eq!(account["signed_in"], false);
    let list = magic(home.path())
        .args(["computer", "list"])
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let development = magic(home.path())
        .args(["--server-url", "http://127.0.0.1:0", "login"])
        .output()
        .unwrap();
    let error = String::from_utf8_lossy(&development.stderr);
    assert!(error.contains("authentication service"), "{error}");
    assert!(!error.contains("magic server stop"), "{error}");

    let login = magic(home.path()).arg("login").output().unwrap();
    assert!(!login.status.success());
    assert!(String::from_utf8_lossy(&login.stderr).contains("magic server stop"));
    let other_directory = tempfile::tempdir().unwrap();
    let overridden = magic(home.path())
        .env("MAGICMTN_LOCAL_DIRECTORY", other_directory.path())
        .arg("login")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&overridden.stderr).contains("magic server stop"));
    let stop = magic(home.path())
        .env("MAGICMTN_LOCAL_DIRECTORY", other_directory.path())
        .args(["server", "stop"])
        .output()
        .unwrap();
    assert!(
        stop.status.success(),
        "{}",
        String::from_utf8_lossy(&stop.stderr)
    );
    server.0.wait().unwrap();
    assert!(!magicmtn_local::is_running(&directory).unwrap());
    assert!(directory.join("connection.json").exists());
    let account = magic(home.path()).args(["pi", "account"]).output().unwrap();
    assert!(account.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&account.stdout).unwrap()["mode"],
        "local"
    );

    // Stale connection and lock files do not prevent the next login attempt.
    let login = magic(home.path())
        .args(["--server-url", "http://127.0.0.1:0", "login"])
        .output()
        .unwrap();
    let error = String::from_utf8_lossy(&login.stderr);
    assert!(error.contains("authentication service"), "{error}");
    assert!(!error.contains("magic server stop"));
}

#[test]
fn transition_lock_blocks_both_login_and_startup() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".config/magicmtn/cli");
    fs::create_dir_all(&directory).unwrap();
    let lock = File::create(directory.join("mode.lock")).unwrap();
    lock.lock().unwrap();
    for args in [vec!["login"], vec!["server", "start"]] {
        let output = magic(home.path()).args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("in progress"));
    }
}

#[tokio::test]
async fn cloud_authentication_blocks_local_start_before_initialization() {
    use axum::{routing::get, Json, Router};
    use serde_json::json;
    use sha2::{Digest, Sha256};
    let app = Router::new().route(
        "/auth/me",
        get(|| async { Json(json!({"display_name":"Person","email":"person@example.test"})) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".config/magicmtn/cli");
    fs::create_dir_all(&directory).unwrap();
    let digest = Sha256::digest(url.as_bytes());
    fs::write(
        directory.join(format!("{digest:x}.json")),
        json!({"server":url,"token":"valid"}).to_string(),
    )
    .unwrap();
    fs::write(
        directory.join("selection.json"),
        json!({"mode":"cloud", "server":url}).to_string(),
    )
    .unwrap();
    let path = home.path().to_owned();
    let output = tokio::task::spawn_blocking(move || {
        magic(&path).args(["server", "start"]).output().unwrap()
    })
    .await
    .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("logout"));
    assert!(!home.path().join(".config/magicmtn/local").exists());
    server.abort();
}

#[test]
fn failed_start_does_not_select_local_mode() {
    let home = tempfile::tempdir().unwrap();
    let output = magic(home.path())
        .args(["server", "start", "--bind", "0.0.0.0:0"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!home
        .path()
        .join(".config/magicmtn/cli/selection.json")
        .exists());
}
