#![cfg(feature = "development-server")]

use axum::{extract::Request, http::StatusCode, response::IntoResponse, Json, Router};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_uses_server_access_even_with_missing_or_stale_cached_plan() {
    let organization = "11111111-1111-4111-8111-111111111111";
    for (plan, active, backend_allowed) in [
        (None, false, false),
        (None, false, true),
        (Some("ascent"), true, false),
        (Some("ascent"), true, true),
        (Some("ascent"), false, true),
        (Some("unknown"), true, true),
    ] {
        let operations = Arc::new(AtomicUsize::new(0));
        let recorded = operations.clone();
        let app = Router::new().fallback(move |request: Request| {
            let recorded = recorded.clone();
            async move {
                let catalog = request.uri().path() == "/v1/pi/catalog";
                assert!(catalog || request.uri().path().starts_with("/v1/sandbox"));
                if catalog && backend_allowed {
                    return Json(json!({"scope":{"organization_id":"11111111-1111-4111-8111-111111111111","user_id":"22222222-2222-4222-8222-222222222222"},"defaults":{"model_id":"33333333-3333-4333-8333-333333333333"}})).into_response();
                }
                assert_eq!(request.headers()["authorization"], "Bearer test-session");
                let mut response = json!([]);
                if request.method() == axum::http::Method::POST
                    && matches!(
                        request.uri().path(),
                        "/v1/sandboxes" | "/v1/sandboxes/managed"
                    )
                {
                    let managed = request.uri().path().ends_with("/managed");
                    let body = axum::body::to_bytes(request.into_body(), 8192)
                        .await
                        .unwrap();
                    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    assert!(
                        uuid::Uuid::parse_str(body["idempotency_key"].as_str().unwrap()).is_ok()
                    );
                    response = json!({"id": body["idempotency_key"]});
                    if managed {
                        assert!(body.get("template").is_none());
                        assert!(body.get("provider").is_none());
                    } else {
                        assert_eq!(body["template"], "image:alpine");
                        if body["connection_id"].is_null() {
                            assert_eq!(body["provider"], "apple-container");
                        } else {
                            assert_eq!(body["provider"], "firecracker");
                            assert_eq!(
                                body["connection_id"],
                                "11111111-1111-4111-8111-111111111111"
                            );
                            assert_eq!(body["notes"], "example");
                        }
                    }
                }
                recorded.fetch_add(1, Ordering::SeqCst);
                if backend_allowed {
                    Json(response).into_response()
                } else {
                    (
                        StatusCode::FORBIDDEN,
                        Json(json!({"error":"Account access denied."})),
                    )
                        .into_response()
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".config/magicmtn/cli");
        fs::create_dir_all(&directory).unwrap();
        let session = if plan.is_none() {
            json!({"server": base, "token": "test-session"})
        } else {
            json!({"server": base, "token": "test-session", "plan_access":{"plan":plan,"access_active":active}})
        };
        fs::write(
            directory.join(format!("{:x}.json", Sha256::digest(base.as_bytes()))),
            session.to_string(),
        )
        .unwrap();

        for args in [
            vec!["new"],
            vec!["list"],
            vec!["show", organization],
            vec!["new", "--image", "alpine"],
            vec![
                "new",
                "--image",
                "alpine",
                "--provider",
                "firecracker",
                "--connection",
                organization,
                "--notes",
                "example",
            ],
            vec!["terminate", organization, "--yes"],
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_magic"))
                .args(["--server-url", &base, "sandbox"])
                .args(&args)
                .env("HOME", home.path())
                .env_remove("MAGICMTN_CA_CERT")
                .env("MAGICMTN_ORGANIZATION_ID", "ignored")
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(if backend_allowed { 0 } else { 1 }),
                "{args:?}: {:?}",
                output
            );
            if !backend_allowed {
                assert!(String::from_utf8_lossy(&output.stderr).contains("403 Forbidden"));
            }
        }
        assert_eq!(operations.load(Ordering::SeqCst), 6);
        server.abort();
    }
}
