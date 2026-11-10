//! Roster maintenance: abandoned agent rows are pruned, protected rows stay.

mod common;
use common as tu;
use std::collections::HashSet;

use breadboard_server::{agent_prune, config::Config};
use chrono::{Duration, Utc};
use serde_json::Value;
use sqlx::SqlitePool;

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

async fn insert_issue(db: &SqlitePool, user_id: i64) -> i64 {
    let project_id: i64 = sqlx::query_scalar(
        "INSERT INTO projects (key, name, next_number) VALUES ('PRUNE', 'Prune', 2) RETURNING id",
    )
    .fetch_one(db)
    .await
    .unwrap();
    let now = Utc::now().to_rfc3339();
    sqlx::query_scalar(
        "INSERT INTO issues
            (key, project_id, title, status, priority, type, reporter_id, created_at, updated_at)
         VALUES ('PRUNE-1', ?1, 'prune test', 'doing', 'none', 'issue', ?2, ?3, ?3)
         RETURNING id",
    )
    .bind(project_id)
    .bind(user_id)
    .bind(now)
    .fetch_one(db)
    .await
    .unwrap()
}

async fn insert_claim(db: &SqlitePool, issue_id: i64, user_id: i64, name: &str) {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO agent_claims
            (issue_id, claimed_by, agent_name, state, board_status, claimed_at, updated_at, last_heartbeat)
         VALUES (?1, ?2, ?3, 'implementing', 'doing', ?4, ?4, ?4)",
    )
    .bind(issue_id)
    .bind(user_id)
    .bind(name)
    .bind(now)
    .execute(db)
    .await
    .unwrap();
}

async fn insert_dm(db: &SqlitePool, name: &str) {
    let now = Utc::now().to_rfc3339();
    let room_id: i64 = sqlx::query_scalar(
        "INSERT INTO chat_rooms (kind, dm_key, created_at) VALUES ('dm', ?1, ?2) RETURNING id",
    )
    .bind(format!("agent:{name}|human:tester"))
    .bind(&now)
    .fetch_one(db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chat_room_members (room_id, member) VALUES (?1, ?2)")
        .bind(room_id)
        .bind(format!("agent:{name}"))
        .execute(db)
        .await
        .unwrap();
}

async fn insert_open_review(db: &SqlitePool, issue_id: i64, name: &str) {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO review_requests
            (issue_id, pr_url, reviewer, reviewer_kind, state, requested_at, updated_at)
         VALUES (?1, 'https://git.example/p/1', ?2, 'agent', 'requested', ?3, ?3)",
    )
    .bind(issue_id)
    .bind(name)
    .bind(now)
    .execute(db)
    .await
    .unwrap();
}

#[test]
fn prune_window_defaults_and_is_configurable() {
    let default = Config::default();
    assert_eq!(default.agents.prune_after_hours, 24);

    let configured: Config = toml::from_str("[agents]\nprune_after_hours = 48\n").unwrap();
    assert_eq!(configured.agents.prune_after_hours, 48);
}

#[tokio::test]
async fn prunes_only_abandoned_rows_and_audits_each_removal() {
    let app = tu::spawn().await;
    let now = Utc::now();
    let stale = now - Duration::hours(30);
    let fresh = now - Duration::minutes(2);

    let user_id: i64 = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&app.db)
        .await
        .unwrap();
    let issue_id = insert_issue(&app.db, user_id).await;

    // Two prunable rows: one sessionless, one whose OpenCode session is gone.
    insert_session(&app.db, "stale-ghost", None, stale, false).await;
    insert_session(&app.db, "dead-session", Some("ses_gone"), stale, false).await;

    // Every protection in the rule gets a row that must stay.
    insert_session(&app.db, "fresh-agent", None, fresh, false).await;
    insert_session(&app.db, "claim-holder", None, stale, false).await;
    insert_claim(&app.db, issue_id, user_id, "claim-holder").await;
    insert_session(&app.db, "dm-holder", None, stale, false).await;
    insert_dm(&app.db, "dm-holder").await;
    insert_session(&app.db, "review-holder", None, stale, false).await;
    insert_open_review(&app.db, issue_id, "review-holder").await;
    insert_session(&app.db, "paused-holder", None, stale, true).await;

    // Profile-only state is part of the row and must disappear with it.
    sqlx::query(
        "INSERT INTO agent_profiles (user_id, agent_name, message, updated_at)
         VALUES (?1, 'stale-ghost', 'left behind', ?2)",
    )
    .bind(user_id)
    .bind(now.to_rfc3339())
    .execute(&app.db)
    .await
    .unwrap();

    // The roster preview uses the same rule, before the pass runs.
    let roster: Value = reqwest::Client::new()
        .get(format!("http://{}/api/agents/roster", app.addr))
        .header("authorization", format!("Bearer {}", app.token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = |name: &str| {
        roster
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["agent_name"] == name)
            .unwrap_or_else(|| panic!("roster row {name} missing: {roster}"))
    };
    assert_eq!(row("stale-ghost")["abandoned"], true);
    assert!(row("stale-ghost")["abandoned_reason"]
        .as_str()
        .unwrap()
        .contains("sessionless"));
    assert_eq!(row("dead-session")["abandoned"], true);
    assert_eq!(row("fresh-agent")["abandoned"], false);
    assert_eq!(row("claim-holder")["abandoned"], false);
    assert_eq!(row("dm-holder")["abandoned"], false);
    assert_eq!(row("review-holder")["abandoned"], false);
    assert_eq!(row("paused-holder")["abandoned"], false);

    // Empty set means every named session is gone from OpenCode.
    let opencode = HashSet::new();
    let pruned = agent_prune::prune_abandoned_agents(&app.db, 24, Some(&opencode))
        .await
        .unwrap();
    let names: Vec<&str> = pruned.iter().map(|p| p.agent_name.as_str()).collect();
    assert_eq!(names, vec!["dead-session", "stale-ghost"]);

    let remaining: Vec<String> =
        sqlx::query_scalar("SELECT agent_name FROM agent_sessions ORDER BY agent_name")
            .fetch_all(&app.db)
            .await
            .unwrap();
    assert_eq!(
        remaining,
        vec![
            "claim-holder",
            "dm-holder",
            "fresh-agent",
            "paused-holder",
            "review-holder"
        ]
    );

    // Audit rows preserve the name and the reason, including the missing
    // session distinction.
    let audit: Vec<(String, String)> =
        sqlx::query_as("SELECT agent_name, reason FROM agent_removals ORDER BY agent_name")
            .fetch_all(&app.db)
            .await
            .unwrap();
    assert_eq!(audit.len(), 2);
    assert_eq!(audit[0].0, "dead-session");
    assert!(audit[0].1.contains("prune:"));
    assert!(audit[0].1.contains("session missing in OpenCode"));
    assert_eq!(audit[1].0, "stale-ghost");
    assert!(audit[1].1.contains("sessionless"));
    assert!(audit[1].1.contains("idle 30h"));

    let profile: Option<i64> =
        sqlx::query_scalar("SELECT user_id FROM agent_profiles WHERE agent_name = 'stale-ghost'")
            .fetch_optional(&app.db)
            .await
            .unwrap();
    assert!(profile.is_none(), "profile is removed with the row");

    let removed_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_events WHERE kind = 'removed' AND agent_name = 'stale-ghost'",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();
    assert_eq!(removed_events, 1);
}

#[tokio::test]
async fn a_shorter_window_prunes_a_row_a_longer_one_keeps() {
    let app = tu::spawn().await;
    let now = Utc::now();
    insert_session(
        &app.db,
        "six-hour-idle",
        None,
        now - Duration::hours(6),
        false,
    )
    .await;

    let kept = agent_prune::prune_abandoned_agents(&app.db, 24, None)
        .await
        .unwrap();
    assert!(kept.is_empty());

    let pruned = agent_prune::prune_abandoned_agents(&app.db, 2, None)
        .await
        .unwrap();
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0].agent_name, "six-hour-idle");
}
