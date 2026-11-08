//! Trusted request context. Set only after account authentication and membership checks.
//! Every runtime database checkout resets its role and scope, including reused connections.
use sqlx::{
    postgres::{PgConnection, PgPoolOptions},
    Executor,
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug)]
pub struct OrganizationScope {
    pub organization_id: Uuid,
    pub user_id: Uuid,
}
tokio::task_local! {
    pub static ORGANIZATION: OrganizationScope;
}
pub fn organization_id() -> Option<Uuid> {
    ORGANIZATION.try_with(|s| s.organization_id).ok()
}
pub fn actor_reference(actor: &str) -> String {
    ORGANIZATION
        .try_with(|s| {
            format!(
                "organization:{}:account:{}:{actor}",
                s.organization_id, s.user_id
            )
        })
        .unwrap_or_else(|_| actor.to_owned())
}
async fn configure(connection: &mut PgConnection) -> Result<(), sqlx::Error> {
    // The runtime role has no BYPASSRLS, even when local DATABASE_URL is a superuser.
    connection.execute("SET ROLE magicmtn_workspace").await?;
    sqlx::query("SELECT set_config('magicmtn.organization_id',$1,false), set_config('magicmtn.system',$2,false), set_config('magicmtn.worker',$3,false)")
        .bind(organization_id().map(|id| id.to_string()).unwrap_or_default())
        .bind(if organization_id().is_some() { "false" } else { "true" })
        .bind(if organization_id().is_some() { "off" } else { "on" })
        .execute(connection).await?;
    Ok(())
}
pub fn pool_options() -> PgPoolOptions {
    PgPoolOptions::new()
        .after_connect(|connection, _| Box::pin(configure(connection)))
        .before_acquire(|connection, _| Box::pin(async move {
            configure(connection).await?;
            Ok(true)
        }))
        .after_release(|connection, _| Box::pin(async move {
            // Never return organization authority to the idle pool.
            connection.execute("RESET ROLE").await?;
            sqlx::query("SELECT set_config('magicmtn.organization_id','',false), set_config('magicmtn.system','false',false), set_config('magicmtn.worker','off',false)")
                .execute(connection).await?;
            Ok(true)
        }))
}
