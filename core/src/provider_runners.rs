//! Outbound polling lets a user's Mac execute local providers for a cloud service.
//! Claimed operations are never redispatched after a lost response.
use crate::{
    provider_connections::Binding,
    workspace::{self, State},
};
use anyhow::{ensure, Context};
use axum::{
    extract::Path,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use magicmtn_sdk::plugin::{Operation, Outcome, Value as ProviderValue};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone)]
pub struct RunnerState {
    pub pool: PgPool,
}
impl workspace::WorkspaceState for RunnerState {
    fn with_pool(mut self, pool: PgPool) -> Self {
        self.pool = pool;
        self
    }
}
type Reply = Result<Json<Value>, (StatusCode, String)>;
fn failure(_: impl std::fmt::Display) -> (StatusCode, String) {
    (
        StatusCode::CONFLICT,
        "runner operation unavailable or fenced".into(),
    )
}
fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
pub async fn authenticate(pool: &PgPool, headers: &HeaderMap) -> anyhow::Result<(Uuid, Uuid)> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .context("runner token required")?;
    let row=sqlx::query("SELECT id,organization_id FROM provider_runners WHERE token_sha256=$1 AND revoked_at IS NULL")
        .bind(token_hash(token)).fetch_one(pool).await?;
    Ok((row.get("id"), row.get("organization_id")))
}
pub fn router(state: RunnerState) -> Router {
    Router::new()
        .route("/v1/provider-runners", get(list).post(create))
        .route("/v1/provider-runners/:id/revoke", post(revoke))
        .route("/v1/provider-runner/claim", post(claim))
        .route("/v1/provider-runner/heartbeat", post(heartbeat))
        .route("/v1/provider-runner/jobs/:id/result", post(result))
        .with_state(state)
}
async fn list(State(s): State<RunnerState>) -> Reply {
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'display_name',display_name,'last_seen_at',last_seen_at,'revoked',revoked_at IS NOT NULL) FROM provider_runners ORDER BY display_name,id")
        .fetch_all(&s.pool).await.map_err(failure)?;
    Ok(Json(json!({"runners":rows})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewRunner {
    display_name: String,
}
async fn create(State(s): State<RunnerState>, Json(body): Json<NewRunner>) -> Reply {
    if body.display_name.trim().is_empty() || body.display_name.len() > 512 {
        return Err(failure("invalid name"));
    }
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO provider_runners(display_name,token_sha256) VALUES($1,$2) RETURNING id",
    )
    .bind(body.display_name)
    .bind(token_hash(&token))
    .fetch_one(&s.pool)
    .await
    .map_err(failure)?;
    Ok(Json(json!({"id":id,"token":token})))
}
async fn revoke(State(s): State<RunnerState>, Path(id): Path<Uuid>) -> Reply {
    let count = sqlx::query("UPDATE provider_runners SET revoked_at=now() WHERE id=$1")
        .bind(id)
        .execute(&s.pool)
        .await
        .map_err(failure)?
        .rows_affected();
    if count != 1 {
        return Err(failure("unknown runner"));
    }
    Ok(Json(json!({"revoked":true})))
}
async fn heartbeat(State(s): State<RunnerState>, h: HeaderMap) -> Reply {
    let (id, _) = authenticate(&s.pool, &h).await.map_err(failure)?;
    sqlx::query(
        "UPDATE provider_runners SET last_seen_at=now() WHERE id=$1 AND revoked_at IS NULL",
    )
    .bind(id)
    .execute(&s.pool)
    .await
    .map_err(failure)?;
    Ok(Json(json!({"ok":true})))
}
async fn claim(State(s): State<RunnerState>, h: HeaderMap) -> Reply {
    let (id, _) = authenticate(&s.pool, &h).await.map_err(failure)?;
    sqlx::query("UPDATE provider_runners SET last_seen_at=now() WHERE id=$1")
        .bind(id)
        .execute(&s.pool)
        .await
        .map_err(failure)?;
    let token = Uuid::new_v4();
    let job:Option<Value>=sqlx::query_scalar("WITH candidate AS (SELECT id FROM provider_rpc_jobs WHERE runner_id=$1 AND state='queued' AND expires_at>now() ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1) UPDATE provider_rpc_jobs j SET state='dispatched',lease_token=$2 FROM candidate c WHERE j.id=c.id RETURNING jsonb_build_object('id',j.id,'lease_token',j.lease_token,'binding',j.binding,'operation',j.operation,'expires_at',j.expires_at)")
        .bind(id).bind(token).fetch_optional(&s.pool).await.map_err(failure)?;
    Ok(Json(json!({"job":job})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultBody {
    lease_token: Uuid,
    outcome: Outcome,
}
async fn result(
    State(s): State<RunnerState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<ResultBody>,
) -> Reply {
    let (runner, _) = authenticate(&s.pool, &h).await.map_err(failure)?;
    let state = if matches!(body.outcome, Outcome::Ok(_)) {
        "completed"
    } else {
        "failed"
    };
    let changed=sqlx::query("UPDATE provider_rpc_jobs SET state=$4,result=$5 WHERE id=$1 AND runner_id=$2 AND lease_token=$3 AND ((state='dispatched' AND expires_at>now()) OR (state IN ('dispatched','uncertain') AND $4='completed' AND ((operation->>'name'='sandbox_create' AND $5#>>'{ok,kind}'='sandbox') OR (operation->>'name'='inference_deployment_create' AND $5#>>'{ok,kind}'='inference_deployment'))))")
        .bind(id).bind(runner).bind(body.lease_token).bind(state).bind(json!(body.outcome)).execute(&s.pool).await.map_err(failure)?;
    if changed.rows_affected() != 1 {
        return Err(failure("result fenced"));
    }
    Ok(Json(json!({"ok":true})))
}
pub async fn dispatch(
    pool: &PgPool,
    binding: &Binding,
    scope: &str,
    operation: Operation,
    timeout: Duration,
) -> anyhow::Result<ProviderValue> {
    let runner = binding.runner_id.context("runner binding missing")?;
    let online:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM provider_runners WHERE id=$1 AND organization_id=$2 AND revoked_at IS NULL AND last_seen_at>now()-interval '15 seconds')")
        .bind(runner).bind(binding.organization_id).fetch_one(pool).await?;
    ensure!(online, "local_runner_unavailable");
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO provider_rpc_jobs(id,organization_id,runner_id,binding,operation,expires_at,scope) VALUES($1,$2,$3,$4,$5,now()+make_interval(secs=>$6),$7)")
        .bind(id).bind(binding.organization_id).bind(runner).bind(json!(binding)).bind(json!(operation)).bind(timeout.as_secs_f64()).bind(scope).execute(pool).await?;
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let row = sqlx::query("SELECT state,result FROM provider_rpc_jobs WHERE id=$1")
            .bind(id)
            .fetch_one(pool)
            .await?;
        if let Some(value) = row.get::<Option<Value>, _>("result") {
            return match serde_json::from_value::<Outcome>(value)? {
                Outcome::Ok(value) => Ok(value),
                Outcome::Error(error) => Err(error.into()),
            };
        }
        if tokio::time::Instant::now() >= deadline {
            let changed=sqlx::query("UPDATE provider_rpc_jobs SET state=CASE state WHEN 'queued' THEN 'failed' ELSE 'uncertain' END WHERE id=$1 AND state IN ('queued','dispatched')")
                .bind(id).execute(pool).await?;
            // A response can win the deadline race. Read it on the next iteration.
            if changed.rows_affected() == 0 {
                continue;
            }
            anyhow::bail!("local_runner_response_lost; operation was not replayed");
        }
        ensure!(
            ["queued", "dispatched"].contains(&row.get::<String, _>("state").as_str()),
            "runner job fenced"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
