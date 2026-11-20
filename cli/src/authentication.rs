use crate::Api;
use anyhow::{bail, ensure, Context, Result};
use reqwest::{Method, Url};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::PathBuf, process::Command, time::Duration};

pub(crate) struct Credentials {
    directory: PathBuf,
}

impl Credentials {
    pub(crate) fn new() -> Result<Self> {
        let home = std::env::var_os("HOME").context("HOME environment variable not set")?;
        Ok(Self {
            directory: PathBuf::from(home).join(".config/magicmtn/cli"),
        })
    }

    fn path(&self, server: &Url) -> PathBuf {
        let digest = Sha256::digest(server.as_str().as_bytes());
        self.directory.join(format!("{digest:x}.json"))
    }

    fn read(&self, server: &Url) -> Result<Option<Value>> {
        let bytes = match fs::read(self.path(server)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("reading saved CLI session"),
        };
        let value: Value = serde_json::from_slice(&bytes).context("invalid saved CLI session")?;
        ensure!(
            value["server"] == server.as_str(),
            "saved session server mismatch"
        );
        required_string(&value, "token")?;
        Ok(Some(value))
    }

    pub(crate) fn load(&self, server: &Url) -> Result<Option<String>> {
        Ok(self
            .read(server)?
            .map(|value| value["token"].as_str().unwrap().to_owned()))
    }

    /// Check the consumer's Cloud session without contacting developer servers.
    pub(crate) fn ensure_signed_out(&self, server: &Url) -> Result<()> {
        let token = self.load(server)?;
        let logout = if server.as_str() == "https://api.magicmtn.ai/" {
            "magic logout".to_owned()
        } else {
            format!("magic --server-url {server} logout")
        };
        ensure!(
            token.is_none(),
            "signed in to MagicMtn Cloud at {server}; run `{logout}` before `magic server start`"
        );
        Ok(())
    }

    fn save(&self, server: &Url, token: &str) -> Result<()> {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&self.directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        serde_json::to_writer(
            &mut file,
            &json!({"server": server.as_str(), "token": token}),
        )?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(self.path(server))
            .context("saving CLI session")?;
        Ok(())
    }

    fn remove(&self, server: &Url) -> Result<()> {
        match fs::remove_file(self.path(server)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("removing saved CLI session"),
        }
    }
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .with_context(|| format!("invalid authentication response: missing {key}"))
}

async fn auth_request(
    api: &Api,
    path: &str,
    origin: Option<&str>,
    body: Option<Value>,
) -> Result<Value> {
    let method = if body.is_some() {
        Method::POST
    } else {
        Method::GET
    };
    let mut request = api
        .client
        .request(method, api.base.join(path)?)
        .timeout(Duration::from_secs(15));
    if let Some(origin) = origin {
        request = request.header("Origin", origin);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .context("could not reach MagicMtn authentication service")?;
    ensure!(
        response.status().is_success(),
        "authentication failed ({}); run `magic login` to try again",
        response.status()
    );
    response
        .json()
        .await
        .context("invalid authentication response")
}

async fn start(api: &Api, client: &str) -> Result<(String, Value, Url)> {
    let config = auth_request(api, "auth/config", None, None).await?;
    let origin = required_string(&config, "origin")?.to_owned();
    let origin_url = Url::parse(&origin)?;
    ensure!(
        matches!(origin_url.scheme(), "http" | "https"),
        "invalid authentication origin"
    );
    let handoff = auth_request(api, "auth/desktop/start", Some(&origin), Some(json!({}))).await?;
    required_string(&handoff, "id")?;
    required_string(&handoff, "secret")?;
    let mut url = Url::parse(required_string(&handoff, "url")?)?;
    ensure!(
        url.origin() == origin_url.origin()
            && url.username().is_empty()
            && url.password().is_none(),
        "invalid browser sign-in URL"
    );
    url.query_pairs_mut().append_pair("client", client);
    Ok((origin, handoff, url))
}

async fn wait_for_approval(
    api: &Api,
    origin: &str,
    handoff: &Value,
    timeout: Duration,
) -> Result<String> {
    let body = json!({
        "id": required_string(handoff, "id")?,
        "secret": required_string(handoff, "secret")?,
        "account_only": true,
    });
    tokio::time::timeout(timeout, async {
        loop {
            let result =
                auth_request(api, "auth/desktop/poll", Some(origin), Some(body.clone())).await?;
            match result["approved"].as_bool() {
                Some(true) => return Ok(required_string(handoff, "secret")?.to_owned()),
                Some(false) => tokio::time::sleep(Duration::from_secs(1)).await,
                None => bail!("invalid authentication approval response"),
            }
        }
    })
    .await
    .context("sign-in timed out; run `magic login` to try again")?
}

pub(crate) fn open_browser(url: &Url) -> Result<()> {
    let mut command;
    #[cfg(target_os = "macos")]
    {
        command = Command::new("open");
    }
    #[cfg(target_os = "windows")]
    {
        command = Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        command = Command::new("xdg-open");
    }
    let status = command
        .arg(url.as_str())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    ensure!(status.success(), "browser launcher failed");
    Ok(())
}

pub(crate) async fn login(api: &Api, client: &str) -> Result<()> {
    let development = crate::mode::development_server(&api.base);
    let _mode = if development {
        None
    } else {
        Some(crate::mode::login()?)
    };
    let (origin, handoff, url) = start(api, client).await?;
    println!("Sign in and approve CLI access in your browser:\n{url}");
    if open_browser(&url).is_err() {
        eprintln!("Could not open a browser automatically. Open the URL above to continue.");
    }
    let token = wait_for_approval(api, &origin, &handoff, Duration::from_secs(300)).await?;
    Credentials::new()?.save(&api.base, &token)?;
    if !development {
        crate::mode::select(&crate::mode::Selection::Cloud {
            server: api.base.to_string(),
        })?;
    }
    println!(
        "Signed in to MagicMtn.{}",
        if client == "pi" {
            " Returning to pi."
        } else {
            ""
        }
    );
    Ok(())
}

pub(crate) async fn logout(api: &Api) -> Result<()> {
    if api.token.is_some() {
        let config = auth_request(api, "auth/config", None, None).await?;
        let response = api
            .authenticated(api.client.post(api.base.join("auth/logout")?))
            .header("Origin", required_string(&config, "origin")?)
            .timeout(Duration::from_secs(15))
            .json(&json!({}))
            .send()
            .await
            .context("could not revoke CLI session; saved session retained")?;
        ensure!(
            response.status().is_success(),
            "could not revoke CLI session ({}); saved session retained",
            response.status()
        );
    }
    Credentials::new()?.remove(&api.base)?;
    println!("Signed out of MagicMtn.");
    Ok(())
}

pub(crate) async fn account(api: &Api) -> Result<Value> {
    if api.local {
        return Ok(json!({"mode":"local", "signed_in":false, "server":api.base.as_str()}));
    }
    if api.token.is_none() {
        return Ok(json!({"signed_in":false}));
    }
    let response = api
        .authenticated(api.client.get(api.base.join("auth/me")?))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .context("could not check MagicMtn sign-in")?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Ok(json!({"signed_in":false}));
    }
    ensure!(
        response.status().is_success(),
        "could not check MagicMtn sign-in ({})",
        response.status()
    );
    let user: Value = response.json().await.context("invalid account response")?;
    Ok(
        json!({"signed_in":true,"user":{"display_name":required_string(&user,"display_name")?,"email":required_string(&user,"email")?}}),
    )
}

async fn account_organization(api: &Api) -> Result<Option<Value>> {
    let organizations = api.request(Method::GET, "auth/organizations", None).await?;
    let organizations = organizations
        .as_array()
        .context("invalid organizations response")?;
    // The server returns zero or one organization for this account.
    Ok(organizations.first().cloned())
}

pub(crate) async fn whoami(api: &Api) -> Result<String> {
    ensure!(
        api.token.is_some(),
        "not signed in; run `magic login` for this server"
    );
    let user = api.request(Method::GET, "auth/me", None).await?;
    let email = required_string(&user, "email")?;
    let mut output = format!("Email: {email}");
    if let Some(organization) = account_organization(api).await? {
        output.push_str(&format!(
            "\nOrganization: {}",
            required_string(&organization, "display_name")?
        ));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::State,
        http::HeaderMap,
        routing::{get, post},
        Json, Router,
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn account_checks_the_server_and_treats_expired_sessions_as_signed_out() -> Result<()> {
        let app = Router::new().route("/auth/me", get(|headers: HeaderMap| async move {
            if headers.get("authorization").and_then(|h| h.to_str().ok()) == Some("Bearer valid") {
                (axum::http::StatusCode::OK, Json(json!({"display_name":"Rodrigo","email":"rodrigo@example.test","id":"private-session-id"})))
            } else {
                (axum::http::StatusCode::UNAUTHORIZED, Json(json!({})))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let mut api = Api::new(&format!("http://{}", listener.local_addr()?))?;
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert_eq!(account(&api).await?, json!({"signed_in":false}));
        api.token = Some("expired".into());
        assert_eq!(account(&api).await?, json!({"signed_in":false}));
        api.token = Some("valid".into());
        assert_eq!(
            account(&api).await?,
            json!({"signed_in":true,"user":{"display_name":"Rodrigo","email":"rodrigo@example.test"}})
        );
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn local_start_checks_selected_cloud_session_without_network_requests() -> Result<()> {
        let app = Router::new().route(
            "/auth/me",
            get(|headers: HeaderMap| async move {
                if headers["authorization"] == "Bearer valid" {
                    (
                        axum::http::StatusCode::OK,
                        Json(json!({"display_name":"Person","email":"person@example.test"})),
                    )
                } else {
                    (axum::http::StatusCode::UNAUTHORIZED, Json(json!({})))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = Url::parse(&format!("http://{}/", listener.local_addr()?))?;
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let directory = tempfile::tempdir()?;
        let credentials = Credentials {
            directory: directory.path().join("cli"),
        };
        credentials.ensure_signed_out(&url)?;
        fs::create_dir_all(&credentials.directory)?;
        let defaults_path = credentials
            .directory
            .join(format!("defaults-{}.json", "a".repeat(64)));
        let defaults = br#"{"inference_source":"magicmtn","model_id":"model"}"#;
        fs::write(&defaults_path, defaults)?;
        fs::write(credentials.directory.join("other.json"), b"invalid JSON")?;
        credentials.ensure_signed_out(&url)?;
        assert_eq!(fs::read(&defaults_path)?, defaults);
        credentials.save(&url, "valid")?;
        let error = credentials.ensure_signed_out(&url).unwrap_err().to_string();
        assert!(error.contains("logout"));
        credentials.save(&url, "expired")?;
        assert!(credentials.ensure_signed_out(&url).is_err());
        credentials.remove(&url)?;
        credentials.save(&Url::parse("http://127.0.0.1:1/")?, "developer-session")?;
        server.abort();
        credentials.ensure_signed_out(&url)?;
        fs::write(credentials.path(&url), b"{}")?;
        assert!(credentials.ensure_signed_out(&url).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn successful_cloud_login_selects_cloud_only_after_approval() -> Result<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let base = format!("http://cloud.example.test:{}", address.port());
        let app = Router::new()
            .route("/auth/config", get(|State(base): State<String>| async move { Json(json!({"origin":base})) }))
            .route("/auth/desktop/start", post(|State(base): State<String>| async move { Json(json!({"id":"request", "secret":"approved-session", "url":format!("{base}/auth/desktop?request=request")})) }))
            .route("/auth/desktop/poll", post(|| async { Json(json!({"approved":true})) }))
            .with_state(base.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let home = tempfile::tempdir()?;
        let directory = home.path().join(".config/magicmtn/cli");
        fs::create_dir_all(&directory)?;
        fs::write(
            directory.join("selection.json"),
            json!({"mode":"local", "directory":home.path().join("local")}).to_string(),
        )?;
        let bin = home.path().join("bin");
        fs::create_dir_all(&bin)?;
        for name in ["open", "xdg-open"] {
            let browser = bin.join(name);
            fs::write(&browser, "#!/bin/sh\nexit 0\n")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(browser, fs::Permissions::from_mode(0o700))?;
            }
        }
        let path = home.path().to_owned();
        let endpoint = base.clone();
        let output = tokio::task::spawn_blocking(move || {
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "authentication::tests::login_in_isolated_process",
                    "--nocapture",
                ])
                .env("HOME", path)
                .env("MAGICMTN_TEST_LOGIN_ENDPOINT", endpoint)
                .env("MAGICMTN_TEST_LOGIN_ADDRESS", address.to_string())
                .env(
                    "PATH",
                    std::env::join_paths([
                        bin.as_path(),
                        std::path::Path::new("/usr/bin"),
                        std::path::Path::new("/bin"),
                    ])
                    .unwrap(),
                )
                .output()
                .unwrap()
        })
        .await?;
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let selected: Value = serde_json::from_slice(&fs::read(directory.join("selection.json"))?)?;
        assert_eq!(selected["mode"], "cloud");
        assert_eq!(selected["server"], format!("{base}/"));
        server.abort();
        Ok(())
    }

    // A subprocess isolates HOME and the real mode/credential files while using
    // the production login flow and a client with a local DNS fixture.
    #[tokio::test]
    async fn login_in_isolated_process() -> Result<()> {
        let Ok(base) = std::env::var("MAGICMTN_TEST_LOGIN_ENDPOINT") else {
            return Ok(());
        };
        let address: std::net::SocketAddr =
            std::env::var("MAGICMTN_TEST_LOGIN_ADDRESS")?.parse()?;
        let mut api = Api::new(&base)?;
        api.client = reqwest::Client::builder()
            .no_proxy()
            .resolve("cloud.example.test", address)
            .build()?;
        let developer = Api::new(&format!("http://{address}"))?;
        let running = crate::mode::server()?;
        login(&developer, "cli").await?;
        assert!(matches!(
            crate::mode::selected()?,
            Some(crate::mode::Selection::Local { .. })
        ));
        drop(running);
        let mut failure = Api::new(&format!("{base}/failure"))?;
        failure.client = api.client.clone();
        assert!(login(&failure, "cli").await.is_err());
        assert!(matches!(
            crate::mode::selected()?,
            Some(crate::mode::Selection::Local { .. })
        ));
        login(&api, "cli").await?;
        assert_eq!(
            Credentials::new()?.load(&api.base)?.as_deref(),
            Some("approved-session")
        );
        Ok(())
    }

    #[tokio::test]
    async fn whoami_reports_account_and_optional_organization() -> Result<()> {
        let app = Router::new()
            .route(
                "/auth/me",
                get(|headers: HeaderMap| async move {
                    if headers["authorization"] == "Bearer expired" {
                        (axum::http::StatusCode::UNAUTHORIZED, Json(json!({})))
                    } else {
                        (
                            axum::http::StatusCode::OK,
                            Json(json!({"email":"person@example.test"})),
                        )
                    }
                }),
            )
            .route(
                "/auth/organizations",
                get(|headers: HeaderMap| async move {
                    Json(if headers["authorization"] == "Bearer solo" {
                        json!([])
                    } else {
                        json!([{"display_name":"Acme"}])
                    })
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let mut api = Api::new(&format!("http://{}", listener.local_addr()?))?;
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert!(whoami(&api)
            .await
            .unwrap_err()
            .to_string()
            .contains("magic login"));
        api.token = Some("solo".into());
        assert_eq!(whoami(&api).await?, "Email: person@example.test");
        api.token = Some("member".into());
        assert_eq!(
            whoami(&api).await?,
            "Email: person@example.test\nOrganization: Acme"
        );
        api.token = Some("expired".into());
        assert!(whoami(&api)
            .await
            .unwrap_err()
            .to_string()
            .contains("magic login"));
        server.abort();
        Ok(())
    }

    #[test]
    fn credentials_are_private_atomic_and_server_scoped() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let credentials = Credentials {
            directory: directory.path().join("cli"),
        };
        let server = Url::parse("http://127.0.0.1:8080/")?;
        let other = Url::parse("http://127.0.0.1:8081/")?;
        assert_eq!(credentials.load(&server)?, None);
        fs::create_dir_all(&credentials.directory)?;
        fs::write(
            credentials.path(&server),
            serde_json::to_vec(&json!({
                "server": server.as_str(),
                "token": "first",
                "plan_access": {"plan": "ascent", "access_active": true}
            }))?,
        )?;
        assert_eq!(credentials.load(&server)?.as_deref(), Some("first"));
        credentials.save(&server, "replacement")?;
        assert_eq!(
            credentials.read(&server)?,
            Some(json!({"server": server.as_str(), "token": "replacement"}))
        );
        assert_eq!(credentials.load(&server)?.as_deref(), Some("replacement"));
        assert_eq!(credentials.load(&other)?, None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&credentials.directory)?.permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(credentials.path(&server))?
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        credentials.remove(&server)?;
        credentials.remove(&server)?;
        assert_eq!(credentials.load(&server)?, None);
        Ok(())
    }

    #[tokio::test]
    async fn browser_handoff_keeps_api_and_dashboard_origins_separate() -> Result<()> {
        let polls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/auth/config", get(|| async { Json(json!({"origin": "https://control.magicmtn.ai"})) }))
            .route("/auth/desktop/start", post(|headers: HeaderMap| async move {
                assert_eq!(headers["host"], "api.magicmtn.ai");
                assert_eq!(headers["origin"], "https://control.magicmtn.ai");
                assert!(!headers.contains_key("authorization"));
                Json(json!({"id": "request-id", "secret": "session-secret", "url": "https://control.magicmtn.ai/auth/desktop?request=request-id"}))
            }))
            .route("/auth/desktop/poll", post(|State(polls): State<Arc<AtomicUsize>>, headers: HeaderMap, Json(body): Json<Value>| async move {
                assert_eq!(headers["host"], "api.magicmtn.ai");
                assert_eq!(headers["origin"], "https://control.magicmtn.ai");
                assert_eq!(body, json!({"id": "request-id", "secret": "session-secret", "account_only": true}));
                Json(json!({"approved": polls.fetch_add(1, Ordering::SeqCst) > 0}))
            }))
            .route("/v1/providers", get(|headers: HeaderMap| async move {
                assert_eq!(headers["authorization"], "Bearer session-secret");
                Json(json!([]))
            }).post(|headers: HeaderMap| async move {
                assert_eq!(headers["authorization"], "Bearer session-secret");
                Json(json!({"ok": true}))
            }))
            .route("/private", get(|| async { axum::http::StatusCode::UNAUTHORIZED }))
            .with_state(polls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let mut api = Api::new(&format!("http://{}", listener.local_addr()?))?;
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        // Reach a local fixture with the production API Host header. Browser
        // approval must still open the distinct dashboard origin from discovery.
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::HOST, "api.magicmtn.ai".parse()?);
        api.client = reqwest::Client::builder()
            .default_headers(headers)
            .build()?;
        let (origin, handoff, url) = start(&api, "cli").await?;
        assert_eq!(
            url.as_str(),
            "https://control.magicmtn.ai/auth/desktop?request=request-id&client=cli"
        );
        let (_, _, pi_url) = start(&api, "pi").await?;
        assert_eq!(
            pi_url
                .query_pairs()
                .find(|(key, _)| key == "client")
                .unwrap()
                .1,
            "pi"
        );
        assert!(
            wait_for_approval(&api, &origin, &handoff, Duration::from_millis(20))
                .await
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        polls.store(0, Ordering::SeqCst);
        let token = wait_for_approval(&api, &origin, &handoff, Duration::from_secs(3)).await?;
        assert_eq!(polls.load(Ordering::SeqCst), 2);
        let directory = tempfile::tempdir()?;
        let credentials = Credentials {
            directory: directory.path().join("cli"),
        };
        credentials.save(&api.base, &token)?;
        api.token = credentials.load(&api.base)?;
        assert_eq!(
            api.request(Method::GET, "v1/providers", None).await?,
            json!([])
        );
        let upload = api
            .authenticated(api.client.post(api.base.join("v1/providers")?))
            .body("package")
            .send()
            .await?;
        assert!(upload.status().is_success());
        assert!(api
            .request(Method::GET, "private", None)
            .await
            .unwrap_err()
            .to_string()
            .contains("magic login"));
        server.abort();
        Ok(())
    }
}
