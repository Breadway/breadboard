//! SQLite pool setup. WAL + foreign keys on, migrations embedded from `migrations/`.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Sqlite, SqlitePool, Transaction};

pub type Db = SqlitePool;

/// Begin a write transaction up front with `BEGIN IMMEDIATE`.
///
/// A deferred transaction that reads before its first write (for example the
/// cycle lookup in `store::import_issue`) holds a WAL read snapshot. If another
/// writer commits in between, SQLite cannot upgrade that snapshot and returns
/// `SQLITE_BUSY` immediately instead of waiting on `busy_timeout`, which
/// surfaces as "database is locked". Taking the write lock before the first
/// statement removes that upgrade window entirely.
///
/// `busy_timeout` already serializes ordinary contention; the bounded retry
/// rides out the residual case where a burst outlasts that timeout.
pub async fn begin_write(pool: &SqlitePool) -> Result<Transaction<'static, Sqlite>, sqlx::Error> {
    const ATTEMPTS: u32 = 5;
    let mut delay = std::time::Duration::from_millis(20);
    let mut last_error = None;
    for attempt in 0..ATTEMPTS {
        match pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => return Ok(tx),
            Err(error) if is_busy(&error) => {
                last_error = Some(error);
                if attempt + 1 < ATTEMPTS {
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(std::time::Duration::from_millis(250));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.expect("a busy attempt is recorded before retrying"))
}

/// SQLite reports writer contention as SQLITE_BUSY (5) or SQLITE_LOCKED (6),
/// including extended codes such as SQLITE_BUSY_SNAPSHOT (517).
fn is_busy(error: &sqlx::Error) -> bool {
    let sqlx::Error::Database(db_error) = error else {
        return false;
    };
    if let Some(code) = db_error.code() {
        let code = code.as_ref();
        if code == "5" || code == "6" || code.starts_with("5") || code.starts_with("6") {
            return true;
        }
    }
    let message = db_error.message().to_ascii_lowercase();
    message.contains("database is locked") || message.contains("database table is locked")
}

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

/// Passive checkpoint cadence. SQLite's own autocheckpoint skips a commit when
/// another connection holds a read snapshot, so a busy pool can leave the log
/// above the threshold indefinitely (BB-131).
const WAL_CHECKPOINT_SECS: u64 = 300;
/// Daily quiet-point truncate and `PRAGMA optimize`.
const WAL_TRUNCATE_EVERY: u32 = 288;

/// Run a passive checkpoint every 5 minutes and truncate plus `optimize` once
/// a day. Sizes are logged so the behaviour is observable in production rather
/// than assumed. A checkpoint is cheap when there is nothing to move.
pub fn spawn_wal_maintenance(pool: Db, database_path: PathBuf) {
    tokio::spawn(async move {
        let mut tick =
            tokio::time::interval(std::time::Duration::from_secs(WAL_CHECKPOINT_SECS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut passes: u32 = 0;
        loop {
            tick.tick().await;
            passes = passes.wrapping_add(1);
            let truncate = passes.is_multiple_of(WAL_TRUNCATE_EVERY);
            if let Err(error) = checkpoint(&pool, &database_path, truncate).await {
                tracing::warn!(error = %error, "wal checkpoint failed");
            }
        }
    });
}

/// Checkpoint the WAL now. `truncate` waits for readers (use at a quiet
/// point); the passive form never blocks a reader.
pub async fn checkpoint(pool: &Db, database_path: &Path, truncate: bool) -> anyhow::Result<()> {
    let mode = if truncate { "TRUNCATE" } else { "PASSIVE" };
    if truncate {
        // `optimize` can write; run it before the truncate so the log ends empty.
        sqlx::query("PRAGMA optimize").execute(pool).await?;
    }
    let (busy, log_pages, moved): (i64, i64, i64) =
        sqlx::query_as(&format!("PRAGMA wal_checkpoint({mode})"))
            .fetch_one(pool)
            .await?;
    tracing::info!(
        mode,
        busy,
        log_pages,
        moved,
        wal_bytes = wal_bytes(database_path),
        "wal checkpoint"
    );
    Ok(())
}

fn wal_bytes(database_path: &Path) -> u64 {
    let mut path = database_path.as_os_str().to_owned();
    path.push("-wal");
    std::fs::metadata(PathBuf::from(path))
        .map(|m| m.len())
        .unwrap_or(0)
}
