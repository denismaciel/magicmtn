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

    /// Check every saved remote session, including explicit server overrides.
    pub(crate) async fn ensure_signed_out(&self) -> Result<()> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).context("reading saved CLI sessions"),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let saved: Value =
                serde_json::from_slice(&fs::read(&path)?).context("invalid saved CLI session")?;
            let mut api = Api::new(required_string(&saved, "server")?)?;
            api.token = self.load(&api.base)?;
            let logout = if api.base.as_str() == "https://api.magicmtn.ai/" {
                "magic logout".to_owned()
            } else {
                format!("magic --server-url {} logout", api.base)
            };
            ensure!(
                account(&api).await?["signed_in"] != true,
                "signed in to MagicMtn Cloud at {}; run `{logout}` before `magic server start`",
                api.base
            );
        }
        Ok(())
    }

    #[cfg(test)]
    fn plan_access(&self, server: &Url) -> Result<Value> {
        self.read(server)?
            .and_then(|value| {
                value
                    .get("plan_access")
                    .filter(|plan| plan.is_object())
                    .cloned()
            })
            .context(
                "saved account plan is missing; run `magic login` to refresh your account information",
            )
    }

    fn save(&self, server: &Url, token: &str, plan_access: Option<&Value>) -> Result<()> {
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
            &json!({"server": server.as_str(), "token": token, "plan_access": plan_access}),
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
) -> Result<(String, Value)> {
    let body = json!({"id": required_string(handoff, "id")?, "secret": required_string(handoff, "secret")?});
    tokio::time::timeout(timeout, async {
        loop {
            let result =
                auth_request(api, "auth/desktop/poll", Some(origin), Some(body.clone())).await?;
            match result["approved"].as_bool() {
                Some(true) => {
                    let plan = result.get("plan_access").filter(|plan| plan.is_object())
                        .context("server did not provide account plan information; update the server and run `magic login` again")?;
                    return Ok((required_string(handoff, "secret")?.to_owned(), plan.clone()));
                }
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
    let _mode = crate::mode::login()?;
    let (origin, handoff, url) = start(api, client).await?;
    println!("Sign in and approve CLI access in your browser:\n{url}");
    if open_browser(&url).is_err() {
        eprintln!("Could not open a browser automatically. Open the URL above to continue.");
    }
    let (token, plan) = wait_for_approval(api, &origin, &handoff, Duration::from_secs(300)).await?;
    Credentials::new()?.save(&api.base, &token, Some(&plan))?;
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

async fn organization_plan_access(api: &Api, organization: Option<&Value>) -> Result<Value> {
    let path = match organization {
        Some(organization) => {
            let id = uuid::Uuid::parse_str(required_string(organization, "id")?)?;
            format!("auth/billing/access?organization_id={id}")
        }
        None => "auth/billing/access".to_owned(),
    };
    api.request(Method::GET, &path, None).await
}

pub(crate) async fn plan_access(api: &Api) -> Result<Value> {
    let organization = account_organization(api).await?;
    organization_plan_access(api, organization.as_ref()).await
}

pub(crate) async fn whoami(api: &Api) -> Result<String> {
    ensure!(
        api.token.is_some(),
        "not signed in; run `magic login` for this server"
    );
    let user = api.request(Method::GET, "auth/me", None).await?;
    let email = required_string(&user, "email")?;
    let organization = account_organization(api).await?;
    let access = organization_plan_access(api, organization.as_ref()).await?;
    let active = access["access_active"]
        .as_bool()
        .context("invalid plan access response")?;
    let plan = match required_string(&access, "plan")? {
        "ascent" => "Ascent",
        plan => plan,
    };
    let mut output = format!("Email: {email}\n");
    if let Some(organization) = organization {
        output.push_str(&format!(
            "Organization: {}\n",
            required_string(&organization, "display_name")?
        ));
    }
    output.push_str(&format!("Plan: {plan}"));
    if access["plan"].is_string() && !active {
        output.push_str(" (inactive)");
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
    async fn local_start_checks_all_saved_sessions_and_allows_expired_credentials() -> Result<()> {
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
        credentials.ensure_signed_out().await?;
        credentials.save(&url, "valid", None)?;
        let error = credentials
            .ensure_signed_out()
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("logout"));
        credentials.save(&url, "expired", None)?;
        credentials.ensure_signed_out().await?;
        server.abort();
        assert!(credentials.ensure_signed_out().await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn whoami_and_plan_resolve_sole_organization_or_personal_scope() -> Result<()> {
        use axum::extract::Query;
        use std::collections::HashMap;
        let first = "11111111-1111-4111-8111-111111111111";
        let app = Router::new()
            .route("/auth/me", get(|headers: HeaderMap| async move {
                if headers["authorization"] == "Bearer expired" {
                    (axum::http::StatusCode::UNAUTHORIZED, Json(json!({})))
                } else {
                    (axum::http::StatusCode::OK, Json(json!({"email":"person@example.test"})))
                }
            }))
            .route("/auth/organizations", get(move |headers: HeaderMap| async move {
                let solo = matches!(headers["authorization"].to_str().unwrap(), "Bearer solo" | "Bearer unsubscribed");
                Json(if solo { json!([]) } else {
                    json!([{"id":first,"display_name":"Acme"}])
                })
            }))
            .route("/auth/billing/access", get(move |headers: HeaderMap, Query(query): Query<HashMap<String,String>>| async move {
                let plan = match query.get("organization_id").map(String::as_str) {
                    Some(id) if id == first => "ascent",
                    None => "ascent",
                    _ => panic!("unexpected billing scope"),
                };
                let token = headers["authorization"].to_str().unwrap();
                Json(json!({"plan":plan,"access_active":token != "Bearer inactive"}))
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let mut api = Api::new(&format!("http://{}", listener.local_addr()?))?;
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert!(whoami(&api)
            .await
            .unwrap_err()
            .to_string()
            .contains("magic login"));
        api.token = Some("solo".into());
        assert_eq!(
            whoami(&api).await?,
            "Email: person@example.test\nPlan: Ascent"
        );
        assert_eq!(plan_access(&api).await?["plan"], "ascent");
        api.token = Some("unsubscribed".into());
        assert_eq!(
            whoami(&api).await?,
            "Email: person@example.test\nPlan: Ascent"
        );
        api.token = Some("member".into());
        assert_eq!(
            whoami(&api).await?,
            "Email: person@example.test\nOrganization: Acme\nPlan: Ascent"
        );
        assert_eq!(plan_access(&api).await?["plan"], "ascent");
        api.token = Some("inactive".into());
        assert_eq!(
            whoami(&api).await?,
            "Email: person@example.test\nOrganization: Acme\nPlan: Ascent (inactive)"
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
        credentials.save(
            &server,
            "first",
            Some(&json!({"plan":"ascent","access_active":true})),
        )?;
        assert_eq!(credentials.plan_access(&server)?["plan"], "ascent");
        credentials.save(&server, "replacement", None)?;
        assert!(credentials.plan_access(&server).is_err());
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
                assert_eq!(body, json!({"id": "request-id", "secret": "session-secret"}));
                Json(json!({"approved": polls.fetch_add(1, Ordering::SeqCst) > 0, "plan_access":{"plan":"ascent","access_active":true}}))
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
        let (token, plan) =
            wait_for_approval(&api, &origin, &handoff, Duration::from_secs(3)).await?;
        assert_eq!(polls.load(Ordering::SeqCst), 2);
        let directory = tempfile::tempdir()?;
        let credentials = Credentials {
            directory: directory.path().join("cli"),
        };
        credentials.save(&api.base, &token, Some(&plan))?;
        assert_eq!(credentials.plan_access(&api.base)?, plan);
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
