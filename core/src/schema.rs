//! Shared engine schema history, composed by each application with its own migrations.
use sqlx::migrate::{Migration, Migrator};
use std::borrow::Cow;

pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Preserve historical versions and checksums when composing service packages.
pub fn compose(additional: impl IntoIterator<Item = Migration>) -> Migrator {
    let mut migrations: Vec<_> = MIGRATOR.iter().cloned().chain(additional).collect();
    migrations.sort_by_key(|migration| migration.version);
    assert!(
        migrations
            .windows(2)
            .all(|pair| pair[0].version != pair[1].version),
        "duplicate schema migration version"
    );
    Migrator {
        migrations: Cow::Owned(migrations),
        ..Migrator::DEFAULT
    }
}
