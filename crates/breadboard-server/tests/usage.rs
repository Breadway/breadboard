//! Usage stats: import from an OpenCode session database, then read the
//! per-agent and per-issue roll-ups.

mod common;
use common as tu;
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

async fn ensure_project(c: &reqwest::Client, base: &str, auth: &str) {
    let resp = c
        .post(format!("{base}/api/projects"))
        .header("authorization", auth)
        .json(&json!({ "key": "USAGE", "name": "Usage work" }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "project setup failed");
}

async fn create_issue(c: &reqwest::Client, base: &str, auth: &str, title: &str) -> String {
    let issue: Value = c
        .post(format!("{base}/api/issues"))
        .header("authorization", auth)
        .json(&json!({ "title": title, "project": "USAGE" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    issue["key"].as_str().unwrap().to_string()
}

/// Minimal OpenCode schema. Only the columns the importer reads are present.
async fn fake_opencode_db(path: &std::path::Path) -> sqlx::SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE session_v2 (
            id TEXT PRIMARY KEY,
            agent TEXT,
            model TEXT,
            cost REAL NOT NULL DEFAULT 0,
            tokens_input INTEGER NOT NULL DEFAULT 0,
            tokens_output INTEGER NOT NULL DEFAULT 0,
            tokens_reasoning INTEGER NOT NULL DEFAULT 0,
            tokens_cache_read INTEGER NOT NULL DEFAULT 0,
            tokens_cache_write INTEGER NOT NULL DEFAULT 0,
            time_updated INTEGER NOT NULL
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE session_message (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            \"type\" TEXT NOT NULL,
            time_created INTEGER NOT NULL,
            data TEXT NOT NULL
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

#[allow(clippy::too_many_arguments)]
async fn insert_message(
    pool: &sqlx::SqlitePool,
    id: &str,
    session_id: &str,
    model: &str,
    cost: f64,
    input: i64,
    output: i64,
    reasoning: i64,
    cache_read: i64,
    cache_write: i64,
) {
    sqlx::query(
        "INSERT INTO session_message (id, session_id, \"type\", time_created, data)
         VALUES (?1, ?2, 'assistant', ?3, ?4)",
    )
    .bind(id)
    .bind(session_id)
    .bind(chrono::Utc::now().timestamp_millis())
    .bind(
        json!({
            "model": { "id": model, "providerID": "test" },
            "tokens": {
                "input": input,
                "output": output,
                "reasoning": reasoning,
                "cache": { "read": cache_read, "write": cache_write }
            },
            "cost": cost
        })
        .to_string(),
    )
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn imports_per_agent_and_per_issue_and_warns_over_threshold() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let session = "ses_usage_one";

    ensure_project(&c, &base, &auth).await;
    let key = create_issue(&c, &base, &auth, "usage roll-up").await;

    // Register the session, then claim without a session id: the server fills
    // it from the roster so the issue can be joined to the imported usage.
    let reg: Value = c
        .post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "sesame auditor", "session_id": session }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reg["session_id"], session);

    let claim: Value = c
        .post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "sesame auditor" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        claim["session_id"], session,
        "claim should inherit roster session"
    );

    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("opencode.db");
    let source = fake_opencode_db(&source_path).await;
    insert_message(&source, "m1", session, "cheap", 0.25, 100, 10, 5, 50, 2).await;
    insert_message(
        &source, "m2", session, "pricey", 0.75, 1000, 200, 100, 500, 10,
    )
    .await;
    source.close().await;

    let report: Value = c
        .post(format!("{base}/api/usage/import"))
        .header("authorization", &auth)
        .json(&json!({ "path": source_path }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(report["sessions"], 1);
    assert_eq!(report["rows"], 2);

    let agent: Value = c
        .get(format!("{base}/api/usage/agents/sesame%20auditor?range=7d"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(agent["kind"], "agent");
    assert_eq!(agent["session_count"], 1);
    approx(agent["cost"].as_f64().unwrap(), 1.0);
    assert_eq!(agent["input_tokens"], 1100);
    assert_eq!(agent["output_tokens"], 210);
    assert_eq!(agent["reasoning_tokens"], 105);
    assert_eq!(agent["cache_read_tokens"], 550);
    assert_eq!(agent["cache_write_tokens"], 12);
    assert_eq!(agent["total_tokens"], 1977);
    assert_eq!(agent["models"].as_array().unwrap().len(), 2);
    let shares: f64 = agent["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["share"].as_f64().unwrap())
        .sum();
    approx(shares, 1.0);

    let issue: Value = c
        .get(format!("{base}/api/usage/issues/{key}?range=7d"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(issue["kind"], "issue");
    assert_eq!(issue["session_count"], 1);
    approx(issue["cost"].as_f64().unwrap(), 1.0);
    assert_eq!(issue["total_tokens"], 1977);
    assert_eq!(issue["agents"][0]["agent_name"], "sesame auditor");

    // Releasing the claim must not erase the issue's historical total: the
    // session-to-issue link is permanent, not derived from the live claim.
    c.delete(format!(
        "{base}/api/issues/{key}/claim?note=done%3B%20next%3B%20risky"
    ))
    .header("authorization", &auth)
    .send()
    .await
    .unwrap()
    .assert_status_success();
    let issue: Value = c
        .get(format!("{base}/api/usage/issues/{key}?range=7d"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(issue["session_count"], 1);
    approx(issue["cost"].as_f64().unwrap(), 1.0);

    // Under the default threshold.
    assert!(!agent["over_threshold"].as_bool().unwrap());
    let settings: Value = c
        .put(format!("{base}/api/usage/settings"))
        .header("authorization", &auth)
        .json(&json!({ "spend_warn_usd": 0.5 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(settings["spend_warn_usd"], 0.5);

    let agent: Value = c
        .get(format!("{base}/api/usage/agents/sesame%20auditor?range=7d"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(agent["over_threshold"].as_bool().unwrap());
    approx(agent["spend_warn_usd"].as_f64().unwrap(), 0.5);

    // Threshold 0 means "warnings off", not "warn on every nonzero cost".
    let settings: Value = c
        .put(format!("{base}/api/usage/settings"))
        .header("authorization", &auth)
        .json(&json!({ "spend_warn_usd": 0 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    approx(settings["spend_warn_usd"].as_f64().unwrap(), 0.0);
    let agent: Value = c
        .get(format!("{base}/api/usage/agents/sesame%20auditor?range=7d"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(agent["cost"].as_f64().unwrap() > 0.0);
    assert!(!agent["over_threshold"].as_bool().unwrap());
    approx(agent["spend_warn_usd"].as_f64().unwrap(), 0.0);

    // Re-import with one more message: replace, do not double count.
    let source = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&source_path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    insert_message(&source, "m3", session, "cheap", 0.5, 10, 1, 0, 0, 0).await;
    source.close().await;

    c.post(format!("{base}/api/usage/import"))
        .header("authorization", &auth)
        .json(&json!({ "path": source_path }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let agent: Value = c
        .get(format!("{base}/api/usage/agents/sesame%20auditor?range=7d"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    approx(agent["cost"].as_f64().unwrap(), 1.5);
    assert_eq!(agent["input_tokens"], 1110);
}

#[tokio::test]
async fn imports_zero_cost_and_session_v2_fallback_rows() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    for (name, session) in [("zero cost", "ses_zero"), ("fallback cost", "ses_fallback")] {
        c.post(format!("{base}/api/agents/register"))
            .header("authorization", &auth)
            .json(&json!({ "agent_name": name, "session_id": session }))
            .send()
            .await
            .unwrap();
    }

    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("opencode.db");
    let source = fake_opencode_db(&source_path).await;
    // SUM of integer zero costs must decode as REAL, not fail on type mismatch.
    insert_message(&source, "m0", "ses_zero", "free", 0.0, 5, 0, 0, 0, 0).await;
    sqlx::query(
        "INSERT INTO session_v2
            (id, agent, model, cost, tokens_input, tokens_output, tokens_reasoning,
             tokens_cache_read, tokens_cache_write, time_updated)
         VALUES ('ses_fallback', 'test', '{\"id\":\"fallback\",\"providerID\":\"test\"}',
                 0, 7, 0, 0, 0, 0, ?1)",
    )
    .bind(chrono::Utc::now().timestamp_millis())
    .execute(&source)
    .await
    .unwrap();
    source.close().await;

    let report: Value = c
        .post(format!("{base}/api/usage/import"))
        .header("authorization", &auth)
        .json(&json!({ "path": source_path }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(report["sessions"], 2);
    assert_eq!(report["rows"], 2);

    for (name, tokens) in [("zero cost", 5), ("fallback cost", 7)] {
        let summary: Value = c
            .get(format!("{base}/api/usage/agents/{name}?range=7d"))
            .header("authorization", &auth)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(summary["total_tokens"], tokens, "{name}");
        approx(summary["cost"].as_f64().unwrap(), 0.0);
        assert_eq!(
            summary["unpriced_tokens"], tokens,
            "zero-cost tokens are unpriced, not free: {name}"
        );
        assert_eq!(summary["models"][0]["unpriced_tokens"], tokens, "{name}");
        assert!(!summary["over_threshold"].as_bool().unwrap());
    }
}

#[tokio::test]
async fn sessions_without_a_registered_agent_are_skipped() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("opencode.db");
    let source = fake_opencode_db(&source_path).await;
    insert_message(&source, "m1", "ses_unknown", "cheap", 1.0, 10, 10, 0, 0, 0).await;
    source.close().await;

    let report: Value = c
        .post(format!("{base}/api/usage/import"))
        .header("authorization", &auth)
        .json(&json!({ "path": source_path }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(report["sessions"], 0);
    assert_eq!(report["skipped_sessions"], 1);
}

#[tokio::test]
async fn renamed_agent_keeps_usage_under_the_current_name() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let session = "ses_rename_usage";

    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "old crust", "session_id": session }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("opencode.db");
    let source = fake_opencode_db(&source_path).await;
    insert_message(&source, "m1", session, "cheap", 1.25, 100, 10, 0, 0, 0).await;
    source.close().await;

    c.post(format!("{base}/api/usage/import"))
        .header("authorization", &auth)
        .json(&json!({ "path": source_path }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    c.post(format!("{base}/api/agents/old%20crust/rename"))
        .header("authorization", &auth)
        .json(&json!({ "new_name": "new crust" }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let list: Value = c
        .get(format!("{base}/api/usage/agents?range=all"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let subjects: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["subject"].as_str().unwrap())
        .collect();
    assert!(
        subjects.contains(&"new crust"),
        "renamed agent should own its history: {subjects:?}"
    );
    assert!(
        !subjects.contains(&"old crust"),
        "the old name must not keep a row: {subjects:?}"
    );
    let renamed = list
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["subject"] == "new crust")
        .unwrap();
    approx(renamed["cost"].as_f64().unwrap(), 1.25);
    assert_eq!(renamed["total_tokens"], 110);

    let summary: Value = c
        .get(format!("{base}/api/usage/agents/new%20crust?range=all"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    approx(summary["cost"].as_f64().unwrap(), 1.25);
    assert_eq!(summary["sessions"][0]["agent_name"], "new crust");
}

#[tokio::test]
async fn removed_agent_history_survives_without_a_dead_name_row() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let session = "ses_removed_usage";

    ensure_project(&c, &base, &auth).await;
    let key = create_issue(&c, &base, &auth, "removed usage history").await;

    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "gone agent", "session_id": session }))
        .send()
        .await
        .unwrap()
        .assert_status_success();
    // Claim so the session-to-issue link exists before the agent is removed.
    c.post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "gone agent" }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("opencode.db");
    let source = fake_opencode_db(&source_path).await;
    insert_message(&source, "m1", session, "cheap", 0.75, 200, 20, 0, 0, 0).await;
    source.close().await;

    c.post(format!("{base}/api/usage/import"))
        .header("authorization", &auth)
        .json(&json!({ "path": source_path }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    c.post(format!("{base}/api/agents/gone%20agent/kill"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let list: Value = c
        .get(format!("{base}/api/usage/agents?range=all"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let subjects: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["subject"].as_str().unwrap())
        .collect();
    assert!(
        !subjects.contains(&"gone agent"),
        "a removed name must not reappear as a usage subject: {subjects:?}"
    );
    let unattributed = list
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["subject"] == "unattributed")
        .expect("removed history should stay in the unattributed bucket");
    approx(unattributed["cost"].as_f64().unwrap(), 0.75);
    assert_eq!(unattributed["total_tokens"], 220);

    // The issue total is the durable copy of that history.
    let issue: Value = c
        .get(format!("{base}/api/usage/issues/{key}?range=all"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    approx(issue["cost"].as_f64().unwrap(), 0.75);
    assert_eq!(issue["total_tokens"], 220);
    assert_eq!(issue["agents"][0]["agent_name"], "unattributed");
}

#[tokio::test]
async fn reported_and_unpriced_spend_stay_separate_and_totals_add_up() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let session = "ses_mixed_usage";

    ensure_project(&c, &base, &auth).await;
    let key = create_issue(&c, &base, &auth, "mixed usage").await;
    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "mixed spender", "session_id": session }))
        .send()
        .await
        .unwrap()
        .assert_status_success();
    c.post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "mixed spender" }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("opencode.db");
    let source = fake_opencode_db(&source_path).await;
    // Priced: 100 + 10 tokens for 0.5.
    insert_message(&source, "m1", session, "priced", 0.5, 100, 10, 0, 0, 0).await;
    // Unpriced: 50 + 5 tokens with cost 0.
    insert_message(&source, "m2", session, "free", 0.0, 50, 5, 0, 0, 0).await;
    source.close().await;

    c.post(format!("{base}/api/usage/import"))
        .header("authorization", &auth)
        .json(&json!({ "path": source_path }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let agent: Value = c
        .get(format!("{base}/api/usage/agents/mixed%20spender?range=7d"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    approx(agent["cost"].as_f64().unwrap(), 0.5);
    assert_eq!(agent["unpriced_tokens"], 55);
    assert_eq!(agent["total_tokens"], 165);
    assert_eq!(agent["sessions"][0]["unpriced_tokens"], 55);
    let free = agent["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["model"] == "test/free")
        .unwrap();
    assert_eq!(free["cost"], 0.0);
    assert_eq!(free["unpriced_tokens"], 55);
    let priced = agent["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["model"] == "test/priced")
        .unwrap();
    approx(priced["cost"].as_f64().unwrap(), 0.5);
    assert_eq!(priced["unpriced_tokens"], 0);

    let issue: Value = c
        .get(format!("{base}/api/usage/issues/{key}?range=7d"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    approx(issue["cost"].as_f64().unwrap(), 0.5);
    assert_eq!(issue["unpriced_tokens"], 55);
    assert_eq!(issue["total_tokens"], 165);
    assert_eq!(issue["agents"][0]["unpriced_tokens"], 55);
}

fn approx(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "expected {expected}, got {actual}"
    );
}

trait StatusSuccess {
    fn assert_status_success(&self);
}

impl StatusSuccess for reqwest::Response {
    fn assert_status_success(&self) {
        assert!(
            self.status().is_success(),
            "unexpected status {}",
            self.status()
        );
    }
}
