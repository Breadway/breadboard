//! Usage stats: import from an OpenCode session database, then read the
//! per-agent and per-issue roll-ups.

mod common;
use breadboard_server::config::{
    BillingMode, ModelAllowanceConfig, ProviderBillingConfig, TokenRates,
};
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
    insert_message_from(
        pool,
        id,
        session_id,
        "test",
        model,
        cost,
        input,
        output,
        reasoning,
        cache_read,
        cache_write,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn insert_message_from(
    pool: &sqlx::SqlitePool,
    id: &str,
    session_id: &str,
    provider: &str,
    model: &str,
    cost: f64,
    input: i64,
    output: i64,
    reasoning: i64,
    cache_read: i64,
    cache_write: i64,
) {
    insert_message_at(
        pool,
        id,
        session_id,
        provider,
        model,
        chrono::Utc::now().timestamp_millis(),
        cost,
        input,
        output,
        reasoning,
        cache_read,
        cache_write,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn insert_message_at(
    pool: &sqlx::SqlitePool,
    id: &str,
    session_id: &str,
    provider: &str,
    model: &str,
    time_millis: i64,
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
    .bind(time_millis)
    .bind(
        json!({
            "model": { "id": model, "providerID": provider },
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

fn rates(input: f64, output: f64, cache_read: f64, cache_write: f64) -> TokenRates {
    TokenRates {
        input,
        output,
        cache_read,
        cache_write,
        off_peak: None,
    }
}

fn allowance_provider(
    plan_price_usd: f64,
    models: &[(&str, f64, TokenRates)],
) -> ProviderBillingConfig {
    ProviderBillingConfig {
        mode: BillingMode::Allowance,
        plan_price_usd,
        off_peak_start_utc: None,
        off_peak_end_utc: None,
        models: models
            .iter()
            .map(|(id, allowance_usd, rates)| {
                (
                    (*id).to_string(),
                    ModelAllowanceConfig {
                        allowance_usd: *allowance_usd,
                        rates: rates.clone(),
                    },
                )
            })
            .collect(),
    }
}

fn at_hour(days_ago: i64, hour: u32, minute: u32) -> i64 {
    let day = chrono::Utc::now().date_naive() - chrono::Duration::days(days_ago);
    day.and_hms_opt(hour, minute, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis()
}

#[tokio::test]
async fn allowance_provider_computes_consumption_from_rates_and_ignores_reported_cost() {
    let app = tu::spawn_with_usage(|usage| {
        usage.providers.insert(
            "opencode-go".into(),
            allowance_provider(
                10.0,
                &[("deepseek-v4.1-flash", 100.0, rates(0.30, 1.20, 0.006, 0.30))],
            ),
        );
    })
    .await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let session = "ses_allowance";

    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "allowance agent", "session_id": session }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("opencode.db");
    let source = fake_opencode_db(&source_path).await;
    // 1M in at 0.30, 0.5M out at 1.20, 2M cache read at 0.006,
    // 0.1M cache write at 0.30 = 0.942. The reported 1.016 is ignored.
    insert_message_from(
        &source,
        "m1",
        session,
        "opencode-go",
        "deepseek-v4.1-flash",
        1.016,
        1_000_000,
        500_000,
        0,
        2_000_000,
        100_000,
    )
    .await;
    // A model missing from the allowance table cannot be priced.
    insert_message_from(
        &source,
        "m2",
        session,
        "opencode-go",
        "unlisted-model",
        0.5,
        500_000,
        0,
        0,
        0,
        0,
    )
    .await;
    source.close().await;

    c.post(format!("{base}/api/usage/import"))
        .header("authorization", &auth)
        .json(&json!({ "path": source_path }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let agent: Value = c
        .get(format!(
            "{base}/api/usage/agents/allowance%20agent?range=7d"
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(agent["cost"], 0.0, "allowance dollars are not billed spend");
    approx(agent["allowance_usd"].as_f64().unwrap(), 0.942);
    assert_eq!(agent["unpriced_tokens"], 0);
    assert_eq!(agent["unrated_tokens"], 500_000);
    assert_eq!(agent["total_tokens"], 4_100_000);
    assert_eq!(agent["share_basis"], "allowance");

    let model = agent["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["model"] == "opencode-go/deepseek-v4.1-flash")
        .unwrap();
    assert_eq!(model["billing"], "allowance");
    assert_eq!(model["cost"], 0.0);
    approx(model["allowance_usd"].as_f64().unwrap(), 0.942);
    approx(model["share"].as_f64().unwrap(), 1.0);

    let allowance = agent["allowances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["model"] == "opencode-go/deepseek-v4.1-flash")
        .unwrap();
    approx(allowance["allowance_usd"].as_f64().unwrap(), 100.0);
    approx(allowance["plan_price_usd"].as_f64().unwrap(), 10.0);
    approx(allowance["multiplier"].as_f64().unwrap(), 10.0);
    approx(allowance["used_usd"].as_f64().unwrap(), 0.942);
    let windows = allowance["windows"].as_array().unwrap();
    let labels: Vec<&str> = windows
        .iter()
        .map(|w| w["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["5h", "weekly", "monthly"]);
    approx(windows[0]["limit_usd"].as_f64().unwrap(), 20.0);
    approx(windows[1]["limit_usd"].as_f64().unwrap(), 50.0);
    approx(windows[2]["limit_usd"].as_f64().unwrap(), 100.0);
    assert!(!agent["over_allowance"].as_bool().unwrap());
}

#[tokio::test]
async fn allowance_rates_split_peak_and_off_peak_buckets() {
    let app = tu::spawn_with_usage(|usage| {
        let mut peak = rates(0.30, 1.20, 0.006, 0.30);
        peak.off_peak = Some(Box::new(rates(0.15, 0.60, 0.003, 0.15)));
        let mut provider = allowance_provider(10.0, &[("deepseek-v4.1-flash", 100.0, peak)]);
        provider.off_peak_start_utc = Some("16:30".into());
        provider.off_peak_end_utc = Some("00:30".into());
        usage.providers.insert("opencode-go".into(), provider);
    })
    .await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let session = "ses_peak_off_peak";

    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "peak spender", "session_id": session }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("opencode.db");
    let source = fake_opencode_db(&source_path).await;
    // 10:00 UTC yesterday is peak: 1M in + 1M out = 0.30 + 1.20 = 1.50.
    insert_message_at(
        &source,
        "m_peak",
        session,
        "opencode-go",
        "deepseek-v4.1-flash",
        at_hour(1, 10, 0),
        0.0,
        1_000_000,
        1_000_000,
        0,
        0,
        0,
    )
    .await;
    // 18:00 UTC yesterday is off-peak: 1M in + 1M out = 0.15 + 0.60 = 0.75.
    insert_message_at(
        &source,
        "m_off_peak",
        session,
        "opencode-go",
        "deepseek-v4.1-flash",
        at_hour(1, 18, 0),
        0.0,
        1_000_000,
        1_000_000,
        0,
        0,
        0,
    )
    .await;
    source.close().await;

    c.post(format!("{base}/api/usage/import"))
        .header("authorization", &auth)
        .json(&json!({ "path": source_path }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let agent: Value = c
        .get(format!("{base}/api/usage/agents/peak%20spender?range=all"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    approx(agent["allowance_usd"].as_f64().unwrap(), 2.25);
    assert_eq!(agent["cost"], 0.0);
    let allowance = &agent["allowances"][0];
    approx(allowance["used_usd"].as_f64().unwrap(), 2.25);
    approx(allowance["windows"][2]["used_usd"].as_f64().unwrap(), 2.25);
}

#[tokio::test]
async fn allowance_windows_are_fractions_of_the_monthly_limit() {
    let app = tu::spawn_with_usage(|usage| {
        usage.providers.insert(
            "opencode-go".into(),
            allowance_provider(
                10.0,
                &[("deepseek-v4.1-flash", 4.0, rates(1.0, 0.0, 0.0, 0.0))],
            ),
        );
    })
    .await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let session = "ses_windows";

    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "window watcher", "session_id": session }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("opencode.db");
    let source = fake_opencode_db(&source_path).await;
    let now = chrono::Utc::now().timestamp_millis();
    // 1M inside the 5h window, 2M inside the week, 5M inside the month only.
    insert_message_at(
        &source,
        "m_5h",
        session,
        "opencode-go",
        "deepseek-v4.1-flash",
        now - chrono::Duration::hours(1).num_milliseconds(),
        0.0,
        1_000_000,
        0,
        0,
        0,
        0,
    )
    .await;
    insert_message_at(
        &source,
        "m_week",
        session,
        "opencode-go",
        "deepseek-v4.1-flash",
        now - chrono::Duration::days(3).num_milliseconds(),
        0.0,
        2_000_000,
        0,
        0,
        0,
        0,
    )
    .await;
    insert_message_at(
        &source,
        "m_month",
        session,
        "opencode-go",
        "deepseek-v4.1-flash",
        now - chrono::Duration::days(10).num_milliseconds(),
        0.0,
        5_000_000,
        0,
        0,
        0,
        0,
    )
    .await;
    source.close().await;

    c.post(format!("{base}/api/usage/import"))
        .header("authorization", &auth)
        .json(&json!({ "path": source_path }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let agent: Value = c
        .get(format!("{base}/api/usage/agents/window%20watcher?range=7d"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // 7d range: 1 + 2 = 3; the 10-day-old row is outside it.
    approx(agent["allowance_usd"].as_f64().unwrap(), 3.0);

    let allowance = &agent["allowances"][0];
    approx(allowance["allowance_usd"].as_f64().unwrap(), 4.0);
    let windows = allowance["windows"].as_array().unwrap();
    for (window, used, limit) in [
        (&windows[0], 1.0, 0.8),
        (&windows[1], 3.0, 2.0),
        (&windows[2], 8.0, 4.0),
    ] {
        approx(window["used_usd"].as_f64().unwrap(), used);
        approx(window["limit_usd"].as_f64().unwrap(), limit);
        assert!(window["over_limit"].as_bool().unwrap());
    }
    assert!(agent["over_allowance"].as_bool().unwrap());
}

#[tokio::test]
async fn metered_provider_keeps_billed_dollars_and_unpriced_tokens() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let session = "ses_metered";

    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "metered agent", "session_id": session }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("opencode.db");
    let source = fake_opencode_db(&source_path).await;
    insert_message_from(
        &source,
        "m1",
        session,
        "openrouter",
        "priced-model",
        0.5,
        100,
        10,
        0,
        0,
        0,
    )
    .await;
    insert_message_from(
        &source,
        "m2",
        session,
        "openrouter",
        "free-model",
        0.0,
        50,
        5,
        0,
        0,
        0,
    )
    .await;
    source.close().await;

    c.post(format!("{base}/api/usage/import"))
        .header("authorization", &auth)
        .json(&json!({ "path": source_path }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let agent: Value = c
        .get(format!("{base}/api/usage/agents/metered%20agent?range=7d"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    approx(agent["cost"].as_f64().unwrap(), 0.5);
    assert_eq!(agent["allowance_usd"], 0.0);
    assert_eq!(agent["unpriced_tokens"], 55);
    assert_eq!(agent["unrated_tokens"], 0);
    assert_eq!(agent["total_tokens"], 165);
    assert_eq!(agent["share_basis"], "cost");

    let models = agent["models"].as_array().unwrap();
    let priced = models
        .iter()
        .find(|m| m["model"] == "openrouter/priced-model")
        .unwrap();
    let free = models
        .iter()
        .find(|m| m["model"] == "openrouter/free-model")
        .unwrap();
    assert_eq!(priced["billing"], "metered");
    assert_eq!(priced["allowance_usd"], 0.0);
    approx(priced["cost"].as_f64().unwrap(), 0.5);
    approx(priced["share"].as_f64().unwrap(), 1.0);
    assert_eq!(free["billing"], "metered");
    assert_eq!(free["cost"], 0.0);
    assert_eq!(free["unpriced_tokens"], 55);
    approx(free["share"].as_f64().unwrap(), 0.0);
}

#[tokio::test]
async fn mixed_billing_reports_metered_cost_and_allowance_separately() {
    let app = tu::spawn_with_usage(|usage| {
        usage.providers.insert(
            "opencode-go".into(),
            allowance_provider(
                10.0,
                &[("deepseek-v4.1-flash", 100.0, rates(1.0, 0.0, 0.0, 0.0))],
            ),
        );
    })
    .await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let session = "ses_mixed_billing";

    ensure_project(&c, &base, &auth).await;
    let key = create_issue(&c, &base, &auth, "mixed billing").await;
    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "mixed biller", "session_id": session }))
        .send()
        .await
        .unwrap()
        .assert_status_success();
    c.post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "mixed biller" }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    // Only billed metered cost counts toward the warning threshold.
    c.put(format!("{base}/api/usage/settings"))
        .header("authorization", &auth)
        .json(&json!({ "spend_warn_usd": 0.6 }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("opencode.db");
    let source = fake_opencode_db(&source_path).await;
    // Allowance: 1M input at 1.00/1M = $1.00, reported cost ignored.
    insert_message_from(
        &source,
        "m1",
        session,
        "opencode-go",
        "deepseek-v4.1-flash",
        1.02,
        1_000_000,
        0,
        0,
        0,
        0,
    )
    .await;
    // Billed metered: 40 tokens for 0.5.
    insert_message_from(
        &source,
        "m2",
        session,
        "openrouter",
        "metered-model",
        0.5,
        40,
        0,
        0,
        0,
        0,
    )
    .await;
    // Unpriced metered: 10 tokens with no reported cost.
    insert_message_from(
        &source,
        "m3",
        session,
        "openrouter",
        "free-model",
        0.0,
        10,
        0,
        0,
        0,
        0,
    )
    .await;
    source.close().await;

    c.post(format!("{base}/api/usage/import"))
        .header("authorization", &auth)
        .json(&json!({ "path": source_path }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let agent: Value = c
        .get(format!("{base}/api/usage/agents/mixed%20biller?range=7d"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    approx(agent["cost"].as_f64().unwrap(), 0.5);
    approx(agent["allowance_usd"].as_f64().unwrap(), 1.0);
    assert_eq!(agent["unpriced_tokens"], 10);
    assert_eq!(agent["unrated_tokens"], 0);
    assert_eq!(agent["total_tokens"], 1_000_050);
    assert_eq!(agent["share_basis"], "dollars");
    assert_eq!(agent["spend_warn_usd"].as_f64().unwrap(), 0.6);
    assert!(
        !agent["over_threshold"].as_bool().unwrap(),
        "allowance consumption must not count toward the billed threshold"
    );

    let models = agent["models"].as_array().unwrap();
    let allowance = models
        .iter()
        .find(|m| m["model"] == "opencode-go/deepseek-v4.1-flash")
        .unwrap();
    assert_eq!(allowance["billing"], "allowance");
    approx(allowance["share"].as_f64().unwrap(), 0.6667);
    let billed = models
        .iter()
        .find(|m| m["model"] == "openrouter/metered-model")
        .unwrap();
    approx(billed["share"].as_f64().unwrap(), 0.3333);

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
    approx(issue["allowance_usd"].as_f64().unwrap(), 1.0);
    assert_eq!(issue["unpriced_tokens"], 10);
    assert_eq!(issue["total_tokens"], 1_000_050);
    assert!(!issue["over_threshold"].as_bool().unwrap());
    let issue_agent = &issue["agents"][0];
    assert_eq!(issue_agent["agent_name"], "mixed biller");
    approx(issue_agent["cost"].as_f64().unwrap(), 0.5);
    approx(issue_agent["allowance_usd"].as_f64().unwrap(), 1.0);
    assert_eq!(issue_agent["unpriced_tokens"], 10);
    assert_eq!(issue_agent["total_tokens"], 1_000_050);
}

#[tokio::test]
async fn model_share_uses_cost_for_metered_and_consumption_for_allowance_models() {
    let app = tu::spawn_with_usage(|usage| {
        usage.providers.insert(
            "opencode-go".into(),
            allowance_provider(
                10.0,
                &[
                    ("pricey", 100.0, rates(2.0, 0.0, 0.0, 0.0)),
                    ("cheap", 100.0, rates(1.0, 0.0, 0.0, 0.0)),
                ],
            ),
        );
    })
    .await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    for (name, session) in [
        ("metered shares", "ses_metered_shares"),
        ("allowance shares", "ses_allowance_shares"),
    ] {
        c.post(format!("{base}/api/agents/register"))
            .header("authorization", &auth)
            .json(&json!({ "agent_name": name, "session_id": session }))
            .send()
            .await
            .unwrap()
            .assert_status_success();
    }

    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("opencode.db");
    let source = fake_opencode_db(&source_path).await;
    // Metered: equal tokens, unequal cost. Cost share must win.
    insert_message_from(
        &source,
        "m1",
        "ses_metered_shares",
        "openrouter",
        "costly",
        0.75,
        100,
        0,
        0,
        0,
        0,
    )
    .await;
    insert_message_from(
        &source,
        "m2",
        "ses_metered_shares",
        "openrouter",
        "cheap",
        0.25,
        100,
        0,
        0,
        0,
        0,
    )
    .await;
    // Allowance: equal tokens, different rates. Consumption share must win.
    insert_message_from(
        &source,
        "m3",
        "ses_allowance_shares",
        "opencode-go",
        "pricey",
        9.0,
        1_000_000,
        0,
        0,
        0,
        0,
    )
    .await;
    insert_message_from(
        &source,
        "m4",
        "ses_allowance_shares",
        "opencode-go",
        "cheap",
        0.1,
        1_000_000,
        0,
        0,
        0,
        0,
    )
    .await;
    source.close().await;

    c.post(format!("{base}/api/usage/import"))
        .header("authorization", &auth)
        .json(&json!({ "path": source_path }))
        .send()
        .await
        .unwrap()
        .assert_status_success();

    let metered: Value = c
        .get(format!("{base}/api/usage/agents/metered%20shares?range=7d"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(metered["share_basis"], "cost");
    approx(metered["cost"].as_f64().unwrap(), 1.0);
    for (model, expected) in [("openrouter/costly", 0.75), ("openrouter/cheap", 0.25)] {
        let row = metered["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["model"] == model)
            .unwrap();
        approx(row["share"].as_f64().unwrap(), expected);
    }

    let allowance: Value = c
        .get(format!(
            "{base}/api/usage/agents/allowance%20shares?range=7d"
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(allowance["share_basis"], "allowance");
    assert_eq!(allowance["cost"], 0.0);
    approx(allowance["allowance_usd"].as_f64().unwrap(), 3.0);
    let rows = allowance["models"].as_array().unwrap();
    for (model, expected) in [
        ("opencode-go/pricey", 0.6667),
        ("opencode-go/cheap", 0.3333),
    ] {
        let row = rows.iter().find(|m| m["model"] == model).unwrap();
        approx(row["share"].as_f64().unwrap(), expected);
        assert_eq!(row["cost"], 0.0);
        assert_eq!(row["billing"], "allowance");
    }
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
