//! Organization-owned image inventory. Enrollment is reserved for operators.
//! This does not pull/build images or change existing sandbox startup protocols.
use crate::{
    auth::AuthPolicy,
    workspace::{State, WorkspaceState},
};
use axum::{
    extract::Path,
    http::{header, HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

#[derive(Debug, Serialize, FromRow)]
pub struct OrganizationImage {
    pub id: Uuid,
    #[serde(skip_serializing)]
    pub repository: String,
    #[serde(skip_serializing)]
    pub digest: String,
    pub display_name: String,
    pub description: String,
    pub is_default: bool,
    pub status: String,
    pub generation: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct ImageList {
    pub images: Vec<OrganizationImage>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_image_omits_launch_reference() {
        let image = OrganizationImage {
            id: Uuid::new_v4(),
            repository: "123456789012.dkr.ecr.us-east-2.amazonaws.com/app".into(),
            digest: "sha256:abc".into(),
            display_name: "MagicMtn Canonical".into(),
            description: String::new(),
            is_default: true,
            status: "enabled".into(),
            generation: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let response = serde_json::to_value(&image).unwrap();
        assert!(response.get("repository").is_none());
        assert!(response.get("digest").is_none());
        assert_eq!(response["display_name"], "MagicMtn Canonical");
        assert_eq!(
            image.repository,
            "123456789012.dkr.ecr.us-east-2.amazonaws.com/app"
        );
    }
}

pub async fn list(pool: &PgPool) -> Result<ImageList, sqlx::Error> {
    let images = sqlx::query_as::<_, OrganizationImage>("SELECT id,repository,digest,display_name,description,is_default,status,generation,created_at,updated_at FROM organization_images WHERE organization_id=magicmtn_current_organization() ORDER BY is_default DESC,lower(display_name),id")
        .fetch_all(pool).await?;
    Ok(ImageList { images })
}

/// Resolve an enabled selection within the current organization to an immutable
/// SDK image. A disabled or foreign registration is not a runnable selection.
pub async fn resolve(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<magicmtn_sdk::image_registry::Image>, sqlx::Error> {
    let row: Option<(String, String)> = sqlx::query_as("SELECT repository,digest FROM organization_images WHERE organization_id=magicmtn_current_organization() AND id=$1 AND status='enabled'")
        .bind(id).fetch_optional(pool).await?;
    Ok(row.map(|(repository, digest)| magicmtn_sdk::image_registry::Image { repository, digest }))
}

#[derive(Clone)]
pub struct ImageApiState {
    pub pool: PgPool,
    pub auth: AuthPolicy,
}
impl WorkspaceState for ImageApiState {
    fn with_pool(mut self, pool: PgPool) -> Self {
        self.pool = pool;
        self
    }
}
type Failure = (StatusCode, &'static str);
fn authorize(state: &ImageApiState, headers: &HeaderMap) -> Result<(), Failure> {
    state
        .auth
        .authenticate(headers)
        .and_then(|p| p.require_inventory("images_read"))
        .map_err(|_| (StatusCode::FORBIDDEN, "image inventory action denied"))
}
pub fn router<S: Clone + Send + Sync + 'static>(state: ImageApiState) -> Router<S> {
    Router::new()
        .route("/v1/images", get(list_http))
        .route("/v1/images/:id", get(get_http))
        .layer(axum::middleware::map_response(
            |mut response: axum::response::Response| async move {
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    header::HeaderValue::from_static("no-store"),
                );
                response.headers_mut().insert(
                    header::VARY,
                    header::HeaderValue::from_static("Authorization, x-magicmtn-organization"),
                );
                response
            },
        ))
        .with_state(state)
}
async fn list_http(
    State(state): State<ImageApiState>,
    headers: HeaderMap,
) -> Result<Json<ImageList>, Failure> {
    authorize(&state, &headers)?;
    list(&state.pool).await.map(Json).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "image inventory unavailable",
        )
    })
}
async fn get_http(
    State(state): State<ImageApiState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<OrganizationImage>, Failure> {
    authorize(&state, &headers)?;
    sqlx::query_as::<_, OrganizationImage>("SELECT id,repository,digest,display_name,description,is_default,status,generation,created_at,updated_at FROM organization_images WHERE organization_id=magicmtn_current_organization() AND id=$1")
        .bind(id).fetch_optional(&state.pool).await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "image inventory unavailable"))?
        .map(Json).ok_or((StatusCode::NOT_FOUND, "image not found"))
}
