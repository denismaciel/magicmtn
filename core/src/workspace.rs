//! Workspace-scoped services use the database pool supplied by their host.
use axum::{
    async_trait,
    extract::{FromRef, FromRequestParts},
    http::{request::Parts, StatusCode},
};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone)]
pub struct WorkspaceContext {
    pub id: Uuid,
    pub pool: PgPool,
}

pub trait WorkspaceState: Clone {
    fn with_pool(self, pool: PgPool) -> Self;
}

/// Uses a request workspace when supplied; otherwise retains the host
/// state, allowing a local server to use one fixed workspace pool.
pub struct State<T>(pub T);
#[async_trait]
impl<S, T> FromRequestParts<S> for State<T>
where
    S: Send + Sync,
    T: FromRef<S> + WorkspaceState + Send,
{
    type Rejection = (StatusCode, &'static str);
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let value = T::from_ref(state);
        Ok(Self(match parts.extensions.get::<WorkspaceContext>() {
            Some(context) => value.with_pool(context.pool.clone()),
            None => value,
        }))
    }
}

pub async fn current(pool: &PgPool) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar("SELECT magicmtn_current_organization()")
        .fetch_one(pool)
        .await
}

pub async fn set_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('magicmtn.organization_id',$1,true)")
        .bind(id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    #[derive(Clone)]
    struct PoolState(PgPool);

    impl WorkspaceState for PoolState {
        fn with_pool(self, pool: PgPool) -> Self {
            Self(pool)
        }
    }

    fn pool(database: &str) -> PgPool {
        PgPool::connect_lazy(&format!("postgresql://localhost/{database}")).unwrap()
    }

    fn router() -> Router {
        Router::new()
            .route(
                "/",
                get(|State(state): State<PoolState>| async move {
                    state.0.connect_options().get_database().unwrap().to_owned()
                }),
            )
            .with_state(PoolState(pool("local")))
    }

    #[tokio::test]
    async fn local_workspace_cannot_be_selected_by_request_headers() {
        let request = Request::builder()
            .uri("/")
            .header("x-magicmtn-organization", Uuid::new_v4().to_string())
            .body(Body::empty())
            .unwrap();
        let response = router().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(to_bytes(response.into_body(), 1024).await.unwrap(), "local");
    }

    #[tokio::test]
    async fn host_supplied_workspace_is_scoped_to_its_request() {
        let router = router();
        let mut request = Request::new(Body::empty());
        request.extensions_mut().insert(WorkspaceContext {
            id: Uuid::new_v4(),
            pool: pool("selected"),
        });
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap(),
            "selected"
        );

        let response = router.oneshot(Request::new(Body::empty())).await.unwrap();
        assert_eq!(to_bytes(response.into_body(), 1024).await.unwrap(), "local");
    }
}
