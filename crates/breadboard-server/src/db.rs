//! SQLite pool setup. WAL + foreign keys on, migrations embedded from `migrations/`.

use std::str::FromStr;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

pub type Db = SqlitePool;

pub async fn connect(database_url: &str) -> anyhow::Result<Db> {
    let opts = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5));

    let started = std::time::Instant::now();
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await?;
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        "db: first connection open (WAL set)"
    );

    sqlx::migrate!("../../migrations").run(&pool).await?;
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        "db: migrations applied"
    );

    Ok(pool)
}
