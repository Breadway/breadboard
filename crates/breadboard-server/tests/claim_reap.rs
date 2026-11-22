//! Stale claim reaping: dead claims are released and audited, live or paused
//! owners keep theirs, and the same maintenance pass can then prune the row.

mod common;
use common as tu;

use breadboard_server::config::Config;
use chrono::{Duration, Utc};
use sqlx::SqlitePool;

async fn insert_project(db: &SqlitePool, key: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO projects (key, name, next_number) VALUES (?1, ?1, 2) RETURNING id",
    )
    .bind(key)
    .fetch_one(db)
    .await
    .unwrap()
}

async fn insert_issue(db: &SqlitePool, project_id: i64, key: &str, user_id: i64) -> i64 {
    let now = Utc::now().to_rfc3339();
    sqlx::query_scalar(
        "INSERT INTO issues
            (key, project_id, title, status, priority, type, reporter_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'doing', 'none', 'issue', ?4, ?5, ?5)
         RETURNING id",
    )
    .bind(key)
    .bind(project_id)
    .bind(format!("{key} reap test"))
    .bind(user_id)
    .bind(now)
    .fetch_one(db)
    .await
    .unwrap()
}

async fn insert_session(
    db: &SqlitePool,
    name: &str,
    session_id: Option<&str>,
    last_seen: chrono::DateTime<Utc>,
    paused: bool,
) {
    sqlx::query(
        "INSERT INTO agent_sessions
            (agent_name, session_id, first_seen, last_seen, paused)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(name)
    .bind(session_id)
    .bind((last_seen - Duration::hours(1)).to_rfc3339())
    .bind(last_seen.to_rfc3339())
    .bind(i64::from(paused))
    .execute(db)
    .await
    .unwrap();
}

#[allow(clippy::too_many_arguments)]
async fn insert_claim(
    db: &SqlitePool,
    issue_id: i64,
    user_id: i64,
    name: &str,
    mode: &str,
    session_id: Option<&str>,
    heartbeat: chrono::DateTime<Utc>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO agent_claims
            (issue_id, claimed_by, agent_name, mode, session_id, state, board_status,
             claimed_at, updated_at, last_heartbeat)
         VALUES (?1, ?2, ?3, ?4, ?5, 'implementing', 'doing', ?6, ?6, ?6)
         RETURNING id",
    )
    .bind(issue_id)
    .bind(user_id)
    .bind(name)
    .bind(mode)
    .bind(session_id)
    .bind(heartbeat.to_rfc3339())
    .fetch_one(db)
    .await
    .unwrap()
}

async fn claim_count(db: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM agent_claims WHERE agent_name = ?1")
        .bind(name)
        .fetch_one(db)
        .await
        .unwrap()
}

#[test]
fn reap_window_defaults_and_is_configurable() {
    let default = Config::default();
    assert_eq!(default.agents.claim_reap_after_hours, 72);

    let configured: Config = toml::from_str("[agents]\nclaim_reap_after_hours = 48\n").unwrap();
    assert_eq!(configured.agents.claim_reap_after_hours, 48);
}

#[tokio::test]
async fn reaps_stale_claim_and_preserves_history() {
    let app = tu::spawn().await;
    let now = Utc::now();
    let user_id: i64 = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&app.db)
        .await
        .unwrap();
    let project = insert_project(&app.db, "REAP").await;
    let issue = insert_issue(&app.db, project, "REAP-1", user_id).await;
    let stale = now - Duration::hours(80);
    let session = "ses_dead_reap";

    insert_session(&app.db, "dead-reaper", Some(session), stale, false).await;
    sqlx::query(
        "INSERT INTO issue_sessions (issue_id, session_id, agent_name, first_seen, last_seen, last_state)
         VALUES (?1, ?2, 'dead-reaper', ?3, ?3, 'implementing')",
    )
    .bind(issue)
    .bind(session)
    .bind(stale.to_rfc3339())
    .execute(&app.db)
    .await
    .unwrap();
    insert_claim(
        &app.db,
        issue,
        user_id,
        "dead-reaper",
        "exclusive",
        Some(session),
        stale,
    )
    .await;

    let reaped = breadboard_server::claim_reap::reap_stale_claims(&app.db, 72, 120)
        .await
        .unwrap();
    assert_eq!(reaped.len(), 1);
    assert_eq!(reaped[0].agent_name, "dead-reaper");
    assert_eq!(reaped[0].issue_key, "REAP-1");
    assert_eq!(claim_count(&app.db, "dead-reaper").await, 0);

    // Claim history is preserved as a handoff row, not erased.
    let (agent, mode, state, note): (String, String, String, String) = sqlx::query_as(
        "SELECT agent_name, mode, state, note FROM handoff_notes WHERE issue_id = ?1",
    )
    .bind(issue)
    .fetch_one(&app.db)
    .await
    .unwrap();
    assert_eq!(agent, "dead-reaper");
    assert_eq!(mode, "exclusive");
    assert_eq!(state, "implementing");
    assert!(
        note.contains("dead-reaper"),
        "note names the previous owner: {note}"
    );
    assert!(note.contains("80h"), "note records the idle window: {note}");

    // Issue-level audit entry names the previous owner.
    let (field, old_value, new_value): (String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT field, old_value, new_value FROM sync_log
          WHERE issue_id = ?1 AND field = 'agent_reap'",
    )
    .bind(issue)
    .fetch_one(&app.db)
    .await
    .unwrap();
    assert_eq!(field, "agent_reap");
    assert_eq!(old_value.as_deref(), Some("dead-reaper (exclusive)"));
    assert!(new_value.unwrap().contains("no heartbeat"));

    // The agent timeline gets one released event naming the owner.
    let (kind, detail): (String, Option<String>) =
        sqlx::query_as("SELECT kind, detail FROM agent_events WHERE agent_name = 'dead-reaper'")
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert_eq!(kind, "released");
    assert!(detail.unwrap().contains("dead-reaper"));

    // The session link survives the release with a refreshed last_seen.
    let last_seen: String =
        sqlx::query_scalar("SELECT last_seen FROM issue_sessions WHERE issue_id = ?1")
            .bind(issue)
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert!(last_seen > stale.to_rfc3339(), "session link refreshed");
}

#[tokio::test]
async fn fresh_claim_is_untouched() {
    let app = tu::spawn().await;
    let now = Utc::now();
    let user_id: i64 = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&app.db)
        .await
        .unwrap();
    let project = insert_project(&app.db, "FRESH").await;
    let issue = insert_issue(&app.db, project, "FRESH-1", user_id).await;
    insert_claim(
        &app.db,
        issue,
        user_id,
        "fresh-reaper",
        "exclusive",
        None,
        now - Duration::hours(1),
    )
    .await;

    let reaped = breadboard_server::claim_reap::reap_stale_claims(&app.db, 72, 120)
        .await
        .unwrap();
    assert!(reaped.is_empty());
    assert_eq!(claim_count(&app.db, "fresh-reaper").await, 1);
}

#[tokio::test]
async fn live_session_keeps_its_stale_claim() {
    let app = tu::spawn().await;
    let now = Utc::now();
    let user_id: i64 = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&app.db)
        .await
        .unwrap();
    let project = insert_project(&app.db, "LIVE").await;
    let issue = insert_issue(&app.db, project, "LIVE-1", user_id).await;
    // The claim row was not refreshed for 80h, but the session is still
    // heartbeating right now. A live session always keeps its claim.
    insert_session(&app.db, "live-owner", Some("ses_live"), now, false).await;
    insert_claim(
        &app.db,
        issue,
        user_id,
        "live-owner",
        "exclusive",
        Some("ses_live"),
        now - Duration::hours(80),
    )
    .await;

    let reaped = breadboard_server::claim_reap::reap_stale_claims(&app.db, 72, 120)
        .await
        .unwrap();
    assert!(reaped.is_empty());
    assert_eq!(claim_count(&app.db, "live-owner").await, 1);
}

#[tokio::test]
async fn paused_agent_keeps_its_stale_claim() {
    let app = tu::spawn().await;
    let now = Utc::now();
    let user_id: i64 = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&app.db)
        .await
        .unwrap();
    let project = insert_project(&app.db, "PAUSE").await;
    let issue = insert_issue(&app.db, project, "PAUSE-1", user_id).await;
    // A hand-paused owner is never reaped, even after the same stale window.
    insert_session(
        &app.db,
        "paused-owner",
        Some("ses_paused"),
        now - Duration::hours(90),
        true,
    )
    .await;
    insert_claim(
        &app.db,
        issue,
        user_id,
        "paused-owner",
        "exclusive",
        Some("ses_paused"),
        now - Duration::hours(80),
    )
    .await;

    let reaped = breadboard_server::claim_reap::reap_stale_claims(&app.db, 72, 120)
        .await
        .unwrap();
    assert!(reaped.is_empty());
    assert_eq!(claim_count(&app.db, "paused-owner").await, 1);
}

#[tokio::test]
async fn maintenance_reaps_then_prunes_in_the_same_run() {
    let app = tu::spawn().await;
    let now = Utc::now();
    let user_id: i64 = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&app.db)
        .await
        .unwrap();
    let project = insert_project(&app.db, "MAINT").await;
    let issue = insert_issue(&app.db, project, "MAINT-1", user_id).await;
    // 80h dead: past both the 72h reap window and the 24h prune window.
    let dead = now - Duration::hours(80);
    insert_session(&app.db, "pinned-ghost", Some("ses_pinned"), dead, false).await;
    insert_claim(
        &app.db,
        issue,
        user_id,
        "pinned-ghost",
        "exclusive",
        Some("ses_pinned"),
        dead,
    )
    .await;

    breadboard_server::agent_prune::run_maintenance(&app.state, "test").await;

    assert_eq!(claim_count(&app.db, "pinned-ghost").await, 0);
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM agent_sessions WHERE agent_name = 'pinned-ghost'")
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert_eq!(rows, 0, "the released agent is pruned in the same pass");
    let removal: String =
        sqlx::query_scalar("SELECT reason FROM agent_removals WHERE agent_name = 'pinned-ghost'")
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert!(removal.contains("prune"));
}

#[tokio::test]
async fn roster_sorts_live_names_before_stale_ones() {
    let app = tu::spawn().await;
    let now = Utc::now();
    insert_session(&app.db, "zz-live-name", Some("ses_z"), now, false).await;
    insert_session(
        &app.db,
        "aa-stale-name",
        Some("ses_a"),
        now - Duration::hours(48),
        false,
    )
    .await;

    let roster: serde_json::Value = reqwest::Client::new()
        .get(format!("http://{}/api/agents/roster", app.addr))
        .header("authorization", format!("Bearer {}", app.token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows = roster.as_array().unwrap();
    let live = rows
        .iter()
        .position(|row| row["agent_name"] == "zz-live-name")
        .unwrap();
    let stale = rows
        .iter()
        .position(|row| row["agent_name"] == "aa-stale-name")
        .unwrap();
    assert!(live < stale, "live rows sort before stale rows: {roster}");
    assert_eq!(rows[live]["status"], "live");
    assert_eq!(rows[stale]["status"], "stale");
}
