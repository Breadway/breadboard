//! Agent claims: exclusive presence, board side-effects, heartbeats, status.

mod common;
use common as tu;
use serde_json::{json, Value};

async fn create_issue(c: &reqwest::Client, base: &str, auth: &str, title: &str) -> Value {
    c.post(format!("{base}/api/issues"))
        .header("authorization", auth)
        .json(&json!({ "title": title, "project": "AGENT" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// A fresh database has no projects (no catch-all ROAD), so tests create the
/// one their issues belong to.
async fn ensure_project(c: &reqwest::Client, base: &str, auth: &str) {
    let resp = c
        .post(format!("{base}/api/projects"))
        .header("authorization", auth)
        .json(&json!({ "key": "AGENT", "name": "Agent work" }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "project setup failed: {}", resp.status());
}

#[tokio::test]
async fn claim_lifecycle() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let created = create_issue(&c, &base, &auth, "agent work item").await;
    let key = created["key"].as_str().unwrap().to_string();
    assert_eq!(created["status"], "backlog");

    // Claim: exclusive, moves backlog -> doing + assigns to claimer.
    let claim: Value = c
        .post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "opencode", "session_id": "ses_1", "state": "implementing", "note": "starting" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(claim["agent_name"], "opencode");
    assert_eq!(claim["state"], "implementing");

    let issue: Value = c
        .get(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(issue["status"], "doing");
    assert!(issue.get("assignee_id").and_then(|v| v.as_i64()).is_some());

    // Second claim conflicts, even with a different agent name.
    let conflict = c
        .post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "other-bot" }))
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), 409);

    // Heartbeat updates state + note and refreshes the heartbeat.
    let before = claim["last_heartbeat"].as_str().unwrap().to_string();
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let updated: Value = c
        .put(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "session_id": "ses_1", "state": "awaiting-review", "pr_url": "https://git.example/p/1" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["state"], "awaiting-review");
    assert!(updated["last_heartbeat"].as_str().unwrap() >= before.as_str());

    // Status roll-up mirrors the bb-status widget shape.
    let status: Value = c
        .get(format!("{base}/api/agents/status"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["total"], 1);
    assert!(status["line"].as_str().unwrap().contains("BB"));

    let agents: Value = c
        .get(format!("{base}/api/agents"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(agents.as_array().unwrap().len(), 1);

    // Release: card keeps its status, claim disappears.
    let rel = c
        .delete(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(rel.status(), 204);

    let agents: Value = c
        .get(format!("{base}/api/agents"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(agents.as_array().unwrap().len(), 0);

    let issue: Value = c
        .get(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(issue["status"], "doing");
}

#[tokio::test]
async fn claim_events_flow_over_sse() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let created = create_issue(&c, &base, &auth, "sse claim item").await;
    let key = created["key"].as_str().unwrap().to_string();

    let mut events = tu::EventStream::connect_with_auth(&base, &auth).await;
    c.post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "opencode" }))
        .send()
        .await
        .unwrap();
    assert_eq!(events.next_type().await, "card.moved");
    assert_eq!(events.next_type().await, "card.assigned");
    assert_eq!(events.next_type().await, "agent.claimed");

    c.put(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "state": "verifying" }))
        .send()
        .await
        .unwrap();
    let updated = events.next_event().await;
    assert_eq!(updated["type"], "agent.updated");
    // The heartbeat event carries the claim so clients patch one entry rather
    // than refetching /api/agents and /api/agents/status.
    assert_eq!(updated["claim"]["key"], key.as_str());
    assert_eq!(updated["claim"]["state"], "verifying");
    assert!(updated["claim"]["last_heartbeat"].is_string());

    c.delete(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(events.next_type().await, "agent.released");
}

/// Intents are soft reservations: they do not move the card, do not block an
/// exclusive claim, and may coexist.
#[tokio::test]
async fn intents_do_not_block_or_move() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let created = create_issue(&c, &base, &auth, "queued later").await;
    let key = created["key"].as_str().unwrap().to_string();

    // Intent: status stays backlog, and repeating it for the same agent updates
    // rather than duplicating.
    for _ in 0..2 {
        let intent: Value = c
            .post(format!("{base}/api/issues/{key}/claim"))
            .header("authorization", &auth)
            .json(&json!({ "agent_name": "planner", "mode": "intent", "state": "planned" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(intent["mode"], "intent");
    }
    let issue: Value = c
        .get(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(issue["status"], "backlog");

    // A second agent can also queue.
    c.post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "other-planner", "mode": "intent" }))
        .send()
        .await
        .unwrap();

    let agents: Value = c
        .get(format!("{base}/api/agents"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let intents: Vec<&Value> = agents
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["key"] == key.as_str() && a["mode"] == "intent")
        .collect();
    assert_eq!(intents.len(), 2, "one intent per (agent), deduped");

    // Exclusive work still claims it and moves it to doing.
    let claim: Value = c
        .post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "worker" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(claim["mode"], "exclusive");
    let issue: Value = c
        .get(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(issue["status"], "doing");

    // Status roll-up counts work, not intents.
    let status: Value = c
        .get(format!("{base}/api/agents/status"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["total"], 1);
    assert_eq!(status["queued"], 2);
}

/// Claim lifecycle changes are written to the issue activity timeline.
#[tokio::test]
async fn claim_changes_appear_in_activity() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let created = create_issue(&c, &base, &auth, "audited work").await;
    let key = created["key"].as_str().unwrap().to_string();

    let claim: Value = c
        .post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "auditor" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let claim_id = claim["id"].as_i64().unwrap();

    c.put(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "state": "awaiting-review" }))
        .send()
        .await
        .unwrap();
    c.delete(format!("{base}/api/claims/{claim_id}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();

    let activity: Value = c
        .get(format!("{base}/api/issues/{key}/activity"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let fields: Vec<&str> = activity
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["field"].as_str())
        .collect();
    assert!(fields.contains(&"agent_claim"), "{fields:?}");
    assert!(fields.contains(&"agent_state"), "{fields:?}");
    assert!(fields.contains(&"agent_release"), "{fields:?}");
}

/// Heartbeating a PR URL comments it once on the issue so the link reaches
/// Forgejo, and a repeated heartbeat does not duplicate the comment.
#[tokio::test]
async fn heartbeat_pr_url_comments_once() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let created = create_issue(&c, &base, &auth, "pr linkage").await;
    let key = created["key"].as_str().unwrap().to_string();

    c.post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "linker" }))
        .send()
        .await
        .unwrap();

    for _ in 0..2 {
        c.put(format!("{base}/api/issues/{key}/claim"))
            .header("authorization", &auth)
            .json(&json!({ "pr_url": "https://git.example/pulls/7", "pr_number": 7 }))
            .send()
            .await
            .unwrap();
    }

    let detail: Value = c
        .get(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let comments = detail["comments"].as_array().unwrap();
    let pr_comments: Vec<&Value> = comments
        .iter()
        .filter(|c| c["body"].as_str().unwrap_or("").contains("https://git.example/pulls/7"))
        .collect();
    assert_eq!(pr_comments.len(), 1, "commented exactly once");

    let activity: Value = c
        .get(format!("{base}/api/issues/{key}/activity"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        activity
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["field"] == "agent_pr"),
        "PR link is audited"
    );
}

/// The reviewer-found gaps: unsafe URLs, cross-user access, session mismatch,
/// and an agent that queued an intent then took the work.
#[tokio::test]
async fn claim_security_and_resolution() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    // Second user + token, straight into the tables like the harness does.
    let other_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (forgejo_id, login, display_name) VALUES (2, 'other', 'Other') RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();
    let other_token = "other-token-abcdef";
    sqlx::query(
        "INSERT INTO api_tokens (user_id, name, token_hash, created_at) VALUES (?1, 'test', ?2, '2026-01-01T00:00:00Z')",
    )
    .bind(other_id)
    .bind(breadboard_server::auth::sha256_hex(other_token))
    .execute(&app.db)
    .await
    .unwrap();
    let other_auth = format!("Bearer {other_token}");

    ensure_project(&c, &base, &auth).await;
    let created = create_issue(&c, &base, &auth, "guarded work").await;
    let key = created["key"].as_str().unwrap().to_string();

    // javascript: URLs are rejected on claim and on heartbeat.
    let bad_post = c
        .post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "x", "pr_url": "javascript:alert(1)" }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_post.status(), 400);

    let claim: Value = c
        .post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "owner", "session_id": "ses_owner" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let claim_id = claim["id"].as_i64().unwrap();

    let bad_put = c
        .put(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "pr_url": "javascript:alert(1)" }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_put.status(), 400);

    // Session mismatch is a conflict, not a silent takeover.
    let mismatch = c
        .put(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "session_id": "ses_imposter" }))
        .send()
        .await
        .unwrap();
    assert_eq!(mismatch.status(), 409);

    // Another user cannot heartbeat, release by id, or steal the claim.
    let other_put = c
        .put(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &other_auth)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(other_put.status(), 404);
    let other_release = c
        .delete(format!("{base}/api/claims/{claim_id}"))
        .header("authorization", &other_auth)
        .send()
        .await
        .unwrap();
    assert_eq!(other_release.status(), 403);
    let other_claim = c
        .post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &other_auth)
        .json(&json!({ "agent_name": "thief" }))
        .send()
        .await
        .unwrap();
    assert_eq!(other_claim.status(), 409);

    // An agent that queued an intent and then took the work ends up with one
    // claim, and heartbeat/release by key keep working without ?agent=.
    let second = create_issue(&c, &base, &auth, "queued then taken").await;
    let second_key = second["key"].as_str().unwrap().to_string();
    c.post(format!("{base}/api/issues/{second_key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "solo", "mode": "intent" }))
        .send()
        .await
        .unwrap();
    c.post(format!("{base}/api/issues/{second_key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "solo" }))
        .send()
        .await
        .unwrap();
    let heartbeat = c
        .put(format!("{base}/api/issues/{second_key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "state": "implementing" }))
        .send()
        .await
        .unwrap();
    assert_eq!(heartbeat.status(), 200, "one claim => no disambiguation needed");
    let agents: Value = c
        .get(format!("{base}/api/agents"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let solo: Vec<&Value> = agents
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["key"] == second_key.as_str() && a["agent_name"] == "solo")
        .collect();
    assert_eq!(solo.len(), 1, "exclusive replaced the redundant intent");
}

/// Completion follows the board: a card in Done counts even when the agent has
/// not heartbeated its lifecycle state yet.
#[tokio::test]
async fn completion_follows_board_done() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let created = create_issue(&c, &base, &auth, "finish me").await;
    let key = created["key"].as_str().unwrap().to_string();

    c.post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "finisher" }))
        .send()
        .await
        .unwrap();

    c.patch(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .json(&json!({ "status": "done" }))
        .send()
        .await
        .unwrap();

    let status: Value = c
        .get(format!("{base}/api/agents/status"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["total"], 1);
    assert_eq!(status["done"], 1, "board Done counts as done");
    assert!(status["line"].as_str().unwrap().contains("BB 1/1 done"));

    let claim = &status["claims"][0];
    assert_eq!(claim["board_status"], "done");
    assert_ne!(claim["state"], "done", "the lifecycle state is still the agent's");
}

/// An agent can carry a one-line message, set at claim time or later, and
/// clear it with an empty value.
#[tokio::test]
async fn agent_message_roundtrip() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let created = create_issue(&c, &base, &auth, "message work").await;
    let key = created["key"].as_str().unwrap().to_string();

    let claim: Value = c
        .post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "Bouncy Bagel", "message": "Shipping   the sync fix" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(claim["message"], "Shipping the sync fix", "message is normalised");

    let agents: Value = c
        .get(format!("{base}/api/agents"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(agents[0]["message"], "Shipping the sync fix");

    // Update through the profile endpoint.
    let updated: Value = c
        .put(format!("{base}/api/agents/message"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "bouncy bagel", "message": "Now verifying on the live stack" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["message"], "Now verifying on the live stack");

    // Too long is rejected.
    let long = "x".repeat(121);
    let bad = c
        .put(format!("{base}/api/agents/message"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "bouncy bagel", "message": long }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);

    // Empty clears it.
    let cleared: Value = c
        .put(format!("{base}/api/agents/message"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "bouncy bagel", "message": "" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(cleared["message"].is_null());

    let agents: Value = c
        .get(format!("{base}/api/agents"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(agents[0]["message"].is_null());
}
