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
    assert_eq!(default.agents.maintenance_interval_mins, 60);
    assert_eq!(default.agents.prune_after_hours, 24);
    assert_eq!(default.agents.ephemeral_prune_after_hours, 2);

    let configured: Config = toml::from_str(
        "[agents]\nmaintenance_interval_mins = 15\nprune_after_hours = 48\nephemeral_prune_after_hours = 4\n",
    )
    .unwrap();
    assert_eq!(configured.agents.maintenance_interval_mins, 15);
    assert_eq!(configured.agents.prune_after_hours, 48);
    assert_eq!(configured.agents.ephemeral_prune_after_hours, 4);

    let disabled: Config = toml::from_str(
        "[agents]\nmaintenance_interval_mins = 0\nprune_after_hours = 0\nephemeral_prune_after_hours = 0\n",
    )
    .unwrap();
    assert_eq!(disabled.agents.maintenance_interval_mins, 0);
    assert_eq!(disabled.agents.prune_after_hours, 0);
    assert_eq!(disabled.agents.ephemeral_prune_after_hours, 0);
}

#[test]
fn zero_interval_disables_only_the_periodic_pass() {
    assert_eq!(
        agent_prune::maintenance_interval(60),
        Some(std::time::Duration::from_secs(3600))
    );
    assert_eq!(
        agent_prune::maintenance_interval(5),
        Some(std::time::Duration::from_secs(300))
    );
    assert_eq!(agent_prune::maintenance_interval(0), None);
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
    let pruned = agent_prune::prune_abandoned_agents(&app.db, 24, 2, Some(&opencode))
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
async fn a_reused_pruned_name_inherits_no_children() {
    let app = tu::spawn().await;
    let now = Utc::now();
    let stale = now - Duration::hours(30);

    // The parent is abandoned; the child is fresh and must survive the pass.
    insert_session(&app.db, "reuse-orch", Some("ses_reuse_old"), stale, false).await;
    insert_session(
        &app.db,
        "reuse-child",
        Some("ses_reuse_child"),
        now - Duration::minutes(2),
        false,
    )
    .await;
    sqlx::query(
        "UPDATE agent_sessions
            SET parent_agent_name = 'reuse-orch', parent_session_id = 'ses_reuse_old'
          WHERE agent_name = 'reuse-child'",
    )
    .execute(&app.db)
    .await
    .unwrap();

    let pruned = agent_prune::prune_abandoned_agents(&app.db, 24, 0, None)
        .await
        .unwrap();
    let names: Vec<&str> = pruned.iter().map(|p| p.agent_name.as_str()).collect();
    assert_eq!(names, vec!["reuse-orch"]);

    // Both edges are detached inside the prune transaction, before the name is
    // free for reuse.
    let child_links: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT parent_agent_name, parent_session_id FROM agent_sessions
          WHERE agent_name = 'reuse-child'",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();
    assert_eq!(child_links, (None, None));

    // Re-register the same name with a new session, as the documented reuse
    // path allows, and confirm it inherited nobody.
    let registered: Value = reqwest::Client::new()
        .post(format!("http://{}/api/agents/register", app.addr))
        .header("authorization", format!("Bearer {}", app.token))
        .json(&serde_json::json!({
            "agent_name": "reuse-orch",
            "session_id": "ses_reuse_new",
            "host": "test",
            "pid": 7
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(registered["agent_name"], "reuse-orch");

    let children: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_sessions
          WHERE lower(parent_agent_name) = 'reuse-orch'
             OR parent_session_id IN ('ses_reuse_old', 'ses_reuse_new')",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();
    assert_eq!(children, 0, "the re-registered name has no children");

    let roster: Value = reqwest::Client::new()
        .get(format!("http://{}/api/agents/roster", app.addr))
        .header("authorization", format!("Bearer {}", app.token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let child = roster
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["agent_name"] == "reuse-child")
        .expect("child survives the prune");
    assert!(child["parent_agent_name"].is_null());
    assert!(child["parent_session_id"].is_null());
    assert!(child["orchestrator"].is_null());
}

#[tokio::test]
async fn a_shorter_window_prunes_a_row_a_longer_one_keeps() {
    let app = tu::spawn().await;
    let now = Utc::now();
    insert_session(
        &app.db,
        "six-hour-idle",
        Some("ses_six_hour"),
        now - Duration::hours(6),
        false,
    )
    .await;

    let kept = agent_prune::prune_abandoned_agents(&app.db, 24, 2, None)
        .await
        .unwrap();
    assert!(kept.is_empty());

    let pruned = agent_prune::prune_abandoned_agents(&app.db, 2, 2, None)
        .await
        .unwrap();
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0].agent_name, "six-hour-idle");
}

#[tokio::test]
async fn sessionless_rows_use_the_ephemeral_window() {
    let app = tu::spawn().await;
    let now = Utc::now();

    // Safely past the 2h window, and safely inside it.
    insert_session(
        &app.db,
        "two-hour-ghost",
        None,
        now - Duration::hours(2) - Duration::seconds(5),
        false,
    )
    .await;
    insert_session(
        &app.db,
        "one-hour-ghost",
        None,
        now - Duration::hours(1),
        false,
    )
    .await;

    let pruned = agent_prune::prune_abandoned_agents(&app.db, 24, 2, None)
        .await
        .unwrap();
    let names: Vec<&str> = pruned.iter().map(|p| p.agent_name.as_str()).collect();
    assert_eq!(names, vec!["two-hour-ghost"]);

    let remaining: Vec<String> =
        sqlx::query_scalar("SELECT agent_name FROM agent_sessions ORDER BY agent_name")
            .fetch_all(&app.db)
            .await
            .unwrap();
    assert_eq!(remaining, vec!["one-hour-ghost"]);
}

#[tokio::test]
async fn session_bound_rows_survive_ephemeral_and_use_the_long_window() {
    let app = tu::spawn().await;
    let now = Utc::now();
    insert_session(
        &app.db,
        "bound-agent",
        Some("ses_bound"),
        now - Duration::hours(6),
        false,
    )
    .await;

    // The short window never touches a row with a session id.
    let kept = agent_prune::prune_abandoned_agents(&app.db, 24, 2, None)
        .await
        .unwrap();
    assert!(kept.is_empty());

    // Once past the long window it is removed like any abandoned row.
    sqlx::query("UPDATE agent_sessions SET last_seen = ?1 WHERE agent_name = 'bound-agent'")
        .bind((now - Duration::hours(25)).to_rfc3339())
        .execute(&app.db)
        .await
        .unwrap();

    let pruned = agent_prune::prune_abandoned_agents(&app.db, 24, 2, None)
        .await
        .unwrap();
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0].agent_name, "bound-agent");
    assert!(pruned[0].reason.contains("idle 25h"));
}

#[tokio::test]
async fn ephemeral_protections_keep_rows_that_are_still_in_use() {
    let app = tu::spawn().await;
    let now = Utc::now();
    let stale = now - Duration::hours(5);

    let user_id: i64 = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&app.db)
        .await
        .unwrap();
    let issue_id = insert_issue(&app.db, user_id).await;

    insert_session(&app.db, "ephemeral-plain", None, stale, false).await;
    insert_session(&app.db, "ephemeral-claim", None, stale, false).await;
    insert_claim(&app.db, issue_id, user_id, "ephemeral-claim").await;
    insert_session(&app.db, "ephemeral-chat", None, stale, false).await;
    insert_dm(&app.db, "ephemeral-chat").await;
    insert_session(&app.db, "ephemeral-review", None, stale, false).await;
    insert_open_review(&app.db, issue_id, "ephemeral-review").await;
    insert_session(&app.db, "ephemeral-paused", None, stale, true).await;

    let pruned = agent_prune::prune_abandoned_agents(&app.db, 24, 2, None)
        .await
        .unwrap();
    let names: Vec<&str> = pruned.iter().map(|p| p.agent_name.as_str()).collect();
    assert_eq!(names, vec!["ephemeral-plain"]);

    let remaining: Vec<String> =
        sqlx::query_scalar("SELECT agent_name FROM agent_sessions ORDER BY agent_name")
            .fetch_all(&app.db)
            .await
            .unwrap();
    assert_eq!(
        remaining,
        vec![
            "ephemeral-chat",
            "ephemeral-claim",
            "ephemeral-paused",
            "ephemeral-review"
        ]
    );
}

#[tokio::test]
async fn zero_windows_disable_their_pass() {
    let app = tu::spawn().await;
    let now = Utc::now();
    insert_session(
        &app.db,
        "ghost-three-hours",
        None,
        now - Duration::hours(3),
        false,
    )
    .await;
    insert_session(
        &app.db,
        "bound-thirty-hours",
        Some("ses_old"),
        now - Duration::hours(30),
        false,
    )
    .await;
    insert_session(
        &app.db,
        "ghost-thirty-hours",
        None,
        now - Duration::hours(30),
        false,
    )
    .await;

    // Both 0 disables pruning entirely, even for the long-idle session row.
    let kept = agent_prune::prune_abandoned_agents(&app.db, 0, 0, None)
        .await
        .unwrap();
    assert!(kept.is_empty());

    // Ephemeral 0 disables removal of sessionless rows at any age, so both
    // ghosts stay. The 30h session-bound row is past the long window and is
    // still removed.
    let pruned = agent_prune::prune_abandoned_agents(&app.db, 24, 0, None)
        .await
        .unwrap();
    let names: Vec<&str> = pruned.iter().map(|p| p.agent_name.as_str()).collect();
    assert_eq!(names, vec!["bound-thirty-hours"]);

    // Prune 0 stops the long-window pass; ephemeral still removes sessionless
    // ghosts and never touches a session-bound row, however old.
    insert_session(
        &app.db,
        "bound-forty-hours",
        Some("ses_older"),
        now - Duration::hours(40),
        false,
    )
    .await;
    let pruned = agent_prune::prune_abandoned_agents(&app.db, 0, 2, None)
        .await
        .unwrap();
    let mut names: Vec<&str> = pruned.iter().map(|p| p.agent_name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["ghost-thirty-hours", "ghost-three-hours"]);

    let remaining: Vec<String> =
        sqlx::query_scalar("SELECT agent_name FROM agent_sessions ORDER BY agent_name")
            .fetch_all(&app.db)
            .await
            .unwrap();
    assert_eq!(remaining, vec!["bound-forty-hours"]);
}

#[tokio::test]
async fn ephemeral_audit_row_names_the_rule_and_idle_hours() {
    let app = tu::spawn().await;
    let now = Utc::now();
    insert_session(
        &app.db,
        "ephemeral-audit",
        None,
        now - Duration::hours(3) - Duration::minutes(10),
        false,
    )
    .await;

    let pruned = agent_prune::prune_abandoned_agents(&app.db, 24, 2, None)
        .await
        .unwrap();
    assert_eq!(pruned.len(), 1);

    let (name, reason): (String, String) =
        sqlx::query_as("SELECT agent_name, reason FROM agent_removals")
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert_eq!(name, "ephemeral-audit");
    assert!(reason.starts_with("prune: ephemeral sessionless removal"));
    assert!(reason.contains("no claims, chat or reviews"));
    assert!(reason.contains("idle 3h"));
    assert_eq!(pruned[0].reason, reason);
}

#[tokio::test]
async fn startup_pass_runs_the_same_reap_and_prune_rules() {
    let app = tu::spawn().await;
    insert_session(
        &app.db,
        "startup-ghost",
        None,
        Utc::now() - Duration::hours(3),
        false,
    )
    .await;

    // The startup call is exactly what spawn_pruner runs before it looks at
    // the periodic interval, so an interval of 0 still cleans the roster here.
    agent_prune::run_maintenance(&app.state, "startup").await;

    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_sessions WHERE agent_name = 'startup-ghost'",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();
    assert_eq!(rows, 0);

    let audit: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_removals WHERE agent_name = 'startup-ghost'",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();
    assert_eq!(audit, 1);
}
