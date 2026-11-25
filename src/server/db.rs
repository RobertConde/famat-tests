//! Postgres: the connection pool and the migrations in `migrations/`.

use std::{str::FromStr, time::Duration};

use anyhow::Context;
use sqlx::{
    ConnectOptions, PgPool,
    migrate::Migrator,
    postgres::{PgConnectOptions, PgPoolOptions},
};

use super::config::DatabaseConfig;

static MIGRATOR: Migrator = sqlx::migrate!();

/// Connect and bring the schema up to date. Migrations take an advisory lock,
/// so several servers starting at once are safe.
pub async fn connect(config: &DatabaseConfig) -> anyhow::Result<PgPool> {
    let options = PgConnectOptions::from_str(&config.url)
        .context("parsing DATABASE_URL")?
        // Every statement at DEBUG drowns the dev log; slow ones still warn.
        .log_statements(log::LevelFilter::Trace);
    let pool = PgPoolOptions::new()
        .max_connections(config.max_connections)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(options)
        .await
        .context("connecting to Postgres (DATABASE_URL)")?;
    MIGRATOR.run(&pool).await.context("running database migrations")?;
    Ok(pool)
}
