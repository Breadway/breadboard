//! Agent claims: exclusive presence, board side-effects, heartbeats, status.

mod common;
use common as tu;
use serde_json::{json, Value};

fn roster_row<'a>(roster: &'a Value, name: &str) -> &'a Value {
    roster
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["agent_name"] == name)
        .unwrap_or_else(|| panic!("roster row {name} not found in {roster}"))
}

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
    assert!(
        resp.status().is_success(),
        "project setup failed: {}",
        resp.status()
    );
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
        .delete(format!(
            "{base}/api/issues/{key}/claim?note=done%3B%20next%3B%20risky"
        ))
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

    c.delete(format!(
        "{base}/api/issues/{key}/claim?note=done%3B%20next%3B%20risky"
    ))
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
    c.delete(format!(
        "{base}/api/claims/{claim_id}?note=done%3B%20next%3B%20risky"
    ))
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
        .filter(|c| {
            c["body"]
                .as_str()
                .unwrap_or("")
                .contains("https://git.example/pulls/7")
        })
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
    assert_eq!(
        heartbeat.status(),
        200,
        "one claim => no disambiguation needed"
    );
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
    assert_ne!(
        claim["state"], "done",
        "the lifecycle state is still the agent's"
    );
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
    assert_eq!(
        claim["message"], "Shipping the sync fix",
        "message is normalised"
    );

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
        .json(
            &json!({ "agent_name": "bouncy bagel", "message": "Now verifying on the live stack" }),
        )
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

/// The roster is the union of sessions, profiles and claims: a profile-only
/// agent with no claims still appears, live/stale follows the 120 s heartbeat,
/// and mixed-case registrations dedupe to one lowercased row.
#[tokio::test]
async fn roster_unions_sessions_profiles_and_claims() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let created = create_issue(&c, &base, &auth, "roster item").await;
    let key = created["key"].as_str().unwrap().to_string();

    // The brioche-bridge case: a registered session with zero claims.
    let reg: Value = c
        .post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "brioche-bridge",
            "session_id": "ses_e45b3c0cffehLdDqi1zIPhaBM",
            "host": "hestia",
            "pid": 4242,
            "note": "chat bridge"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reg["agent_name"], "brioche-bridge");
    assert_eq!(reg["paused"], false);

    // A profile with no session and no claim (historical agent).
    sqlx::query(
        "INSERT INTO agent_profiles (user_id, agent_name, message, updated_at)
         VALUES (1, 'profile-only', 'I only have a profile', ?1)",
    )
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.db)
    .await
    .unwrap();

    // A claimer with a session id, sent in mixed case.
    c.post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "Crumb-Marshal",
            "session_id": "ses_crumb",
            "state": "implementing"
        }))
        .send()
        .await
        .unwrap();

    let roster: Value = c
        .get(format!("{base}/api/agents/roster"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let brioche = roster_row(&roster, "brioche-bridge");
    assert_eq!(brioche["claim_count"], 0);
    assert_eq!(brioche["exclusive_count"], 0);
    assert_eq!(brioche["intent_count"], 0);
    assert_eq!(brioche["session_id"], "ses_e45b3c0cffehLdDqi1zIPhaBM");
    assert_eq!(brioche["host"], "hestia");
    assert_eq!(brioche["pid"], 4242);
    assert_eq!(brioche["live"], true);
    assert_eq!(brioche["status"], "live");

    let profile = roster_row(&roster, "profile-only");
    assert_eq!(profile["claim_count"], 0);
    assert_eq!(profile["message"], "I only have a profile");
    assert_eq!(profile["status"], "stale");
    assert_eq!(profile["live"], false);

    let crumb = roster_row(&roster, "crumb-marshal");
    assert_eq!(crumb["claim_count"], 1);
    assert_eq!(crumb["exclusive_count"], 1);
    assert_eq!(crumb["session_id"], "ses_crumb");
    assert_eq!(crumb["state"], "implementing");
    assert_eq!(crumb["live"], true);

    // Deduplicated by lowercased name even when another row uses mixed case.
    let count = roster
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["agent_name"] == "crumb-marshal")
        .count();
    assert_eq!(count, 1);

    // A heartbeat older than 120 s flips live -> stale.
    sqlx::query("UPDATE agent_sessions SET last_seen = ?1 WHERE agent_name = 'brioche-bridge'")
        .bind((chrono::Utc::now() - chrono::Duration::seconds(600)).to_rfc3339())
        .execute(&app.db)
        .await
        .unwrap();
    let roster: Value = c
        .get(format!("{base}/api/agents/roster"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let brioche = roster_row(&roster, "brioche-bridge");
    assert_eq!(brioche["live"], false);
    assert_eq!(brioche["status"], "stale");
    assert_eq!(brioche["session_id"], "ses_e45b3c0cffehLdDqi1zIPhaBM");
}

/// BB-189: registration stores the human session title, a later non-empty
/// report refreshes it, and the roster returns it alongside the full id.
#[tokio::test]
async fn registration_stores_and_refreshes_session_title() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let reg: Value = c
        .post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "titled-agent",
            "session_id": "ses_titled",
            "session_title": "Implement BB-170 name uniqueness"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reg["session_title"], "Implement BB-170 name uniqueness");

    let roster: Value = c
        .get(format!("{base}/api/agents/roster"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = roster_row(&roster, "titled-agent");
    assert_eq!(row["session_id"], "ses_titled");
    assert_eq!(row["session_title"], "Implement BB-170 name uniqueness");

    // A later non-empty title wins, still on the same live session.
    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "titled-agent",
            "session_id": "ses_titled",
            "session_title": "  Wire routers and deploy live  "
        }))
        .send()
        .await
        .unwrap();

    let roster: Value = c
        .get(format!("{base}/api/agents/roster"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = roster_row(&roster, "titled-agent");
    assert_eq!(row["session_title"], "Wire routers and deploy live");
}

/// BB-189: an empty or omitted title, a claim heartbeat and a rename never
/// clear a stored title. Only a new non-empty report replaces it.
#[tokio::test]
async fn blank_session_title_never_clears_stored_title() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let created = create_issue(&c, &base, &auth, "title keeper").await;
    let key = created["key"].as_str().unwrap().to_string();

    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "title-keeper",
            "session_id": "ses_keeper",
            "session_title": "Keep this title"
        }))
        .send()
        .await
        .unwrap();

    let title_of = |roster: &Value, name: &str| {
        roster_row(roster, name)["session_title"]
            .as_str()
            .map(str::to_string)
    };

    // Empty title, then an omitted one.
    for body in [
        json!({
            "agent_name": "title-keeper",
            "session_id": "ses_keeper",
            "session_title": "   "
        }),
        json!({
            "agent_name": "title-keeper",
            "session_id": "ses_keeper"
        }),
    ] {
        c.post(format!("{base}/api/agents/register"))
            .header("authorization", &auth)
            .json(&body)
            .send()
            .await
            .unwrap();
    }

    // A claim write auto-registers through the writable path without a title.
    c.post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "title-keeper",
            "session_id": "ses_keeper",
            "state": "implementing"
        }))
        .send()
        .await
        .unwrap();

    let roster: Value = c
        .get(format!("{base}/api/agents/roster"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        title_of(&roster, "title-keeper").as_deref(),
        Some("Keep this title")
    );

    // Rename moves the row; the title moves with it.
    c.post(format!("{base}/api/agents/title-keeper/rename"))
        .header("authorization", &auth)
        .json(&json!({ "new_name": "title-keeper-renamed" }))
        .send()
        .await
        .unwrap();

    let roster: Value = c
        .get(format!("{base}/api/agents/roster"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        title_of(&roster, "title-keeper-renamed").as_deref(),
        Some("Keep this title")
    );
}

/// Registering without a name assigns a unique curated name and returns
/// alternatives, so a caller can pick without rerolling.
#[tokio::test]
async fn register_assigns_curated_names_with_alternatives() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let first: Value = c
        .post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "session_id": "ses_auto_1" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let name = first["agent_name"].as_str().unwrap().to_string();
    assert_eq!(name, name.to_lowercase());
    // The curated list intentionally includes the single-word "doughminator",
    // so asserting a hyphen here was a flake. Membership is the real rule.
    assert!(
        breadboard_server::names::CURATED.contains(&name.as_str()),
        "{name} should be a curated name"
    );
    let alternatives: Vec<String> = first["alternatives"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!((2..=3).contains(&alternatives.len()), "{alternatives:?}");
    assert!(!alternatives.contains(&name));
    let unique: std::collections::HashSet<_> = alternatives.iter().collect();
    assert_eq!(unique.len(), alternatives.len());

    let second: Value = c
        .post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "session_id": "ses_auto_2" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(second["agent_name"], name.as_str());

    // An explicit name is preserved (lowercased) and the response still offers
    // alternatives.
    let explicit: Value = c
        .post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "Sourdough-Sentinel", "session_id": "ses_explicit" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(explicit["agent_name"], "sourdough-sentinel");
    assert!(!explicit["alternatives"].as_array().unwrap().is_empty());
}

/// Kill removes the agent completely: claims released, roster row, profile and
/// hidden flag gone, and the name immediately reusable. Unknown names 404
/// without creating the phantom row from BB-145.
#[tokio::test]
async fn kill_removes_agent_frees_name_and_unknown_is_404() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let created = create_issue(&c, &base, &auth, "kill me").await;
    let key = created["key"].as_str().unwrap().to_string();

    c.post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "victim",
            "session_id": "ses_victim",
            "message": "about to be killed"
        }))
        .send()
        .await
        .unwrap();

    let removed: Value = c
        .post(format!("{base}/api/agents/victim/kill"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(removed["agent_name"], "victim");
    assert_eq!(removed["removed"], true);
    assert_eq!(removed["claims_released"], 1);
    assert_eq!(removed["profile_removed"], true);

    // Claims and every roster source are gone.
    let claims: Value = c
        .get(format!("{base}/api/agents"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(claims.as_array().unwrap().len(), 0);
    let roster: Value = c
        .get(format!("{base}/api/agents/roster"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        roster
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["agent_name"] != "victim"),
        "removed agent still on roster: {roster}"
    );
    let profile_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_profiles WHERE agent_name = 'victim'")
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert_eq!(profile_count, 0);

    // Kill is no longer idempotent: the name is gone, so a second call is 404.
    let again = c
        .post(format!("{base}/api/agents/victim/kill"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 404);

    // Unknown names are 404 and never create a phantom roster row (BB-145).
    let before = roster.as_array().unwrap().len();
    let unknown = c
        .post(format!("{base}/api/agents/no-such-agent-zz/kill"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);
    let roster: Value = c
        .get(format!("{base}/api/agents/roster"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(roster.as_array().unwrap().len(), before);
    assert!(roster
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["agent_name"] != "no-such-agent-zz"));

    // The name is free: a fresh registration starts clean and can claim.
    let reg: Value = c
        .post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "victim", "session_id": "ses_victim_2" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reg["agent_name"], "victim");
    assert_eq!(reg["paused"], false);
    assert_eq!(reg["session_id"], "ses_victim_2");
    let reclaimed = c
        .post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "victim" }))
        .send()
        .await
        .unwrap();
    assert_eq!(reclaimed.status(), 201);
}

/// Pause keeps claims and the roster row, refuses new claims, suppresses chat
/// pushes and survives registration; resume restores claiming.
#[tokio::test]
async fn pause_keeps_claims_blocks_new_work_and_resumes() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let first = create_issue(&c, &base, &auth, "paused work").await;
    let first_key = first["key"].as_str().unwrap().to_string();
    let second = create_issue(&c, &base, &auth, "new work").await;
    let second_key = second["key"].as_str().unwrap().to_string();

    c.post(format!("{base}/api/issues/{first_key}/claim"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "napper",
            "session_id": "ses_napper",
            "message": "halfway through"
        }))
        .send()
        .await
        .unwrap();

    let paused: Value = c
        .post(format!("{base}/api/agents/napper/pause"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(paused["paused"], true);
    assert_eq!(paused["status"], "paused");
    assert_eq!(paused["live"], false);
    assert_eq!(paused["paused_by_login"], "tester");
    assert_eq!(paused["claim_count"], 1);

    // The claim survives and the roster still shows the paused agent.
    let claims: Value = c
        .get(format!("{base}/api/agents"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(claims.as_array().unwrap().len(), 1);
    assert_eq!(claims[0]["agent_name"], "napper");
    let roster: Value = c
        .get(format!("{base}/api/agents/roster"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let paused_row = roster_row(&roster, "napper");
    assert_eq!(paused_row["status"], "paused");
    assert_eq!(paused_row["paused"], true);
    assert_eq!(paused_row["claim_count"], 1);

    // A new claim is refused with the pause reason.
    let blocked = c
        .post(format!("{base}/api/issues/{second_key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "napper" }))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 403);
    let body: Value = blocked.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("paused"), "{body}");

    // Chat pushes are suppressed while paused. A DM from another agent would
    // normally push to napper, but pause drops it.
    c.post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "sender", "to": "napper", "body": "wake up" }))
        .send()
        .await
        .unwrap();
    let pushes: Value = c
        .get(format!("{base}/api/chat/pushes?agent=napper"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pushes.as_array().unwrap().len(), 0);

    // Registration refreshes the session but does not unpause it.
    let reg: Value = c
        .post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "napper", "session_id": "ses_napper" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reg["paused"], true);

    // Resume is idempotent and restores claiming.
    for _ in 0..2 {
        let resumed: Value = c
            .post(format!("{base}/api/agents/napper/resume"))
            .header("authorization", &auth)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(resumed["paused"], false);
    }
    let claimed = c
        .post(format!("{base}/api/issues/{second_key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "napper" }))
        .send()
        .await
        .unwrap();
    assert_eq!(claimed.status(), 201);
}

/// Pausing or resuming an unknown name is 404, not a phantom row.
#[tokio::test]
async fn pause_and_resume_unknown_are_404() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    for action in ["pause", "resume"] {
        let resp = c
            .post(format!("{base}/api/agents/no-such-agent-zz/{action}"))
            .header("authorization", &auth)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 404, "{action}");
    }
    let roster: Value = c
        .get(format!("{base}/api/agents/roster"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(roster.as_array().unwrap().len(), 0);
}

/// Pin is a roster-maintenance guard, not a lifecycle freeze: it is recorded
/// with its actor, survives re-registration, a session-id change and a rename,
/// does not block new work, and kill still removes the row. Unpin is
/// idempotent and lets the row age out.
#[tokio::test]
async fn pin_survives_registration_and_rename_but_kill_removes_it() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "keeper", "session_id": "ses_keeper" }))
        .send()
        .await
        .unwrap();

    let pinned: Value = c
        .post(format!("{base}/api/agents/keeper/pin"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pinned["pinned"], true);
    assert_eq!(pinned["pinned_by_login"], "tester");
    assert!(
        pinned["pinned_at"].is_string(),
        "pin records when it happened"
    );

    // Repeated pin is a no-op: one state_change event, summary "Pinned".
    let again: Value = c
        .post(format!("{base}/api/agents/keeper/pin"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(again["pinned"], true);
    let events: Vec<(String, String, Option<i64>)> = sqlx::query_as(
        "SELECT kind, summary, actor_id FROM agent_events
          WHERE agent_name = 'keeper' AND kind = 'state_change'
          ORDER BY id",
    )
    .fetch_all(&app.db)
    .await
    .unwrap();
    assert_eq!(events.len(), 1, "a repeated pin writes no duplicate event");
    assert_eq!(events[0].0, "state_change");
    assert_eq!(events[0].1, "Pinned");
    assert!(events[0].2.is_some(), "the event records the actor");

    // Re-registration with a changed session id refreshes the row and keeps
    // the pin: the upsert never lists pinned. Age the row past the live window
    // first, because a live name intentionally refuses a different session id.
    let stale_at = (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339();
    sqlx::query("UPDATE agent_sessions SET last_seen = ?1 WHERE agent_name = 'keeper'")
        .bind(&stale_at)
        .execute(&app.db)
        .await
        .unwrap();
    let reg: Value = c
        .post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "keeper",
            "session_id": "ses_keeper_new",
            "session_title": "keeper session"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reg["session_id"], "ses_keeper_new");
    let roster: Value = c
        .get(format!("{base}/api/agents/roster"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(roster_row(&roster, "keeper")["pinned"], true);

    // A pin does not freeze work: a pinned agent can still claim.
    let issue = create_issue(&c, &base, &auth, "pinned work").await;
    let key = issue["key"].as_str().unwrap();
    let claimed = c
        .post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "keeper", "session_id": "ses_keeper_new" }))
        .send()
        .await
        .unwrap();
    assert_eq!(claimed.status(), 201, "pin never blocks a claim");

    // Rename keeps the pin on the same row and the old name disappears.
    let renamed: Value = c
        .post(format!("{base}/api/agents/keeper/rename"))
        .header("authorization", &auth)
        .json(&json!({ "new_name": "keeper-renamed" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(renamed["agent_name"], "keeper-renamed");
    assert_eq!(renamed["pinned"], true, "rename keeps the pin");
    let roster: Value = c
        .get(format!("{base}/api/agents/roster"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(roster_row(&roster, "keeper-renamed")["pinned"], true);
    assert!(
        roster
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["agent_name"] != "keeper"),
        "the old name is gone"
    );

    // Unpin clears the metadata and is idempotent.
    for _ in 0..2 {
        let unpinned: Value = c
            .post(format!("{base}/api/agents/keeper-renamed/unpin"))
            .header("authorization", &auth)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(unpinned["pinned"], false);
        assert!(unpinned["pinned_by_login"].is_null());
        assert!(unpinned["pinned_at"].is_null());
    }

    // Kill is explicit and still removes a pinned row, even with claims.
    c.post(format!("{base}/api/agents/keeper-renamed/pin"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    let killed = c
        .post(format!("{base}/api/agents/keeper-renamed/kill"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(killed.status(), 200);
    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_sessions WHERE agent_name = 'keeper-renamed'",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();
    assert_eq!(remaining, 0, "kill removes a pinned row");
}

/// Pin and unpin use the same bearer auth as pause/resume and never create a
/// phantom row for an unknown name.
#[tokio::test]
async fn pin_and_unpin_require_auth_and_404_unknown() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    for action in ["pin", "unpin"] {
        let unauthenticated = c
            .post(format!("{base}/api/agents/whoever/{action}"))
            .send()
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), 401, "unauthenticated {action}");
        let unknown = c
            .post(format!("{base}/api/agents/no-such-agent-zz/{action}"))
            .header("authorization", &auth)
            .send()
            .await
            .unwrap();
        assert_eq!(unknown.status(), 404, "unknown {action}");
    }
    let roster: Value = c
        .get(format!("{base}/api/agents/roster"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(roster.as_array().unwrap().len(), 0, "no phantom row");
}

/// A rename before claims is free; after claims it moves the claims, profile
/// and session row in one transaction and never leaves the old name behind.
#[tokio::test]
async fn rename_moves_claims_and_profile_or_refuses() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let created = create_issue(&c, &base, &auth, "rename item").await;
    let key = created["key"].as_str().unwrap().to_string();

    // Free rename while the agent has no claims.
    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "free-name", "session_id": "ses_free" }))
        .send()
        .await
        .unwrap();
    let freed: Value = c
        .post(format!("{base}/api/agents/free-name/rename"))
        .header("authorization", &auth)
        .json(&json!({ "new_name": "freed-name" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(freed["agent_name"], "freed-name");
    assert_eq!(freed["session_id"], "ses_free");

    // Claim, profile and session under the old name.
    c.post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "old-name",
            "session_id": "ses_old",
            "message": "old profile line",
            "state": "implementing"
        }))
        .send()
        .await
        .unwrap();

    let renamed: Value = c
        .post(format!("{base}/api/agents/old-name/rename"))
        .header("authorization", &auth)
        .json(&json!({ "new_name": "new-name" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(renamed["agent_name"], "new-name");
    assert_eq!(renamed["claim_count"], 1);
    assert_eq!(renamed["session_id"], "ses_old");
    assert_eq!(renamed["message"], "old profile line");

    // The claims moved and the profile followed; no orphan under the old name.
    let claims: Value = c
        .get(format!("{base}/api/agents"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(claims[0]["agent_name"], "new-name");
    assert_eq!(claims[0]["message"], "old profile line");

    let roster: Value = c
        .get(format!("{base}/api/agents/roster"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        roster
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["agent_name"] != "old-name"),
        "{roster}"
    );

    // A target already in use is a conflict.
    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "taken-name" }))
        .send()
        .await
        .unwrap();
    let conflict = c
        .post(format!("{base}/api/agents/new-name/rename"))
        .header("authorization", &auth)
        .json(&json!({ "new_name": "taken-name" }))
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), 409);
}

/// Killing publishes `agent.removed`; pausing publishes `agent.paused`.
#[tokio::test]
async fn removal_and_pause_publish_events() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "event-target", "session_id": "ses_event" }))
        .send()
        .await
        .unwrap();

    let mut events = tu::EventStream::connect_with_auth(&base, &auth).await;
    c.post(format!("{base}/api/agents/event-target/pause"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    let event = events.next_event().await;
    assert_eq!(event["type"], "agent.paused");
    assert_eq!(event["agent"], "event-target");

    c.post(format!("{base}/api/agents/event-target/kill"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    let event = events.next_event().await;
    assert_eq!(event["type"], "agent.removed");
    assert_eq!(event["agent"], "event-target");
    assert_eq!(event["claims_released"], 0);
}

/// Hiding is a server-side display flag, not a delete. Claims stay, the flag
/// survives a fresh roster read, and a rename carries it to the new name.
#[tokio::test]
async fn hide_and_unhide_persist_and_keep_claims() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let created = create_issue(&c, &base, &auth, "hide noisy agent").await;
    let key = created["key"].as_str().unwrap().to_string();

    c.post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "noisy-probe",
            "session_id": "ses_noisy",
            "message": "test probe"
        }))
        .send()
        .await
        .unwrap();

    let hidden: Value = c
        .post(format!("{base}/api/agents/noisy-probe/hide"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(hidden["hidden"], true);
    assert_eq!(hidden["hidden_by_login"], "tester");
    assert_eq!(hidden["claim_count"], 1);

    // A fresh read sees the same flag: this is server state, not local UI.
    let roster: Value = c
        .get(format!("{base}/api/agents/roster"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = roster_row(&roster, "noisy-probe");
    assert_eq!(row["hidden"], true);
    assert_eq!(row["claim_count"], 1);

    // Hiding does not release or hide claims from the claim list.
    let claims: Value = c
        .get(format!("{base}/api/agents"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(claims.as_array().unwrap().len(), 1);
    assert_eq!(claims[0]["agent_name"], "noisy-probe");

    // Repeated hide is a no-op.
    let again: Value = c
        .post(format!("{base}/api/agents/noisy-probe/hide"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(again["hidden"], true);
    assert_eq!(again["hidden_at"], hidden["hidden_at"]);

    // Renaming a hidden agent carries the flag.
    let renamed: Value = c
        .post(format!("{base}/api/agents/noisy-probe/rename"))
        .header("authorization", &auth)
        .json(&json!({ "new_name": "renamed-probe" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(renamed["agent_name"], "renamed-probe");
    assert_eq!(renamed["hidden"], true);

    // Unhide is idempotent and clears the flag.
    for _ in 0..2 {
        let shown: Value = c
            .post(format!("{base}/api/agents/renamed-probe/unhide"))
            .header("authorization", &auth)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(shown["hidden"], false);
    }
    assert_eq!(shown_count(&c, &base, &auth).await, 1);

    // Unknown names cannot be hidden.
    let unknown = c
        .post(format!("{base}/api/agents/never-existed/hide"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);
}

async fn shown_count(c: &reqwest::Client, base: &str, auth: &str) -> usize {
    let roster: Value = c
        .get(format!("{base}/api/agents/roster"))
        .header("authorization", auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    roster
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["hidden"] == false)
        .count()
}

/// Hiding and unhiding publish SSE events so other devices refetch the roster.
#[tokio::test]
async fn hide_and_unhide_publish_events() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "event-hide" }))
        .send()
        .await
        .unwrap();

    let mut events = tu::EventStream::connect_with_auth(&base, &auth).await;
    c.post(format!("{base}/api/agents/event-hide/hide"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(events.next_type().await, "agent.hidden");

    c.post(format!("{base}/api/agents/event-hide/unhide"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(events.next_type().await, "agent.unhidden");
}

/// A registered parent session resolves to an orchestrator name on the child
/// row, including when the child holds no claims at all.
#[tokio::test]
async fn roster_resolves_parent_session_to_orchestrator() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "big-orch", "session_id": "ses_orch" }))
        .send()
        .await
        .unwrap();
    let child: Value = c
        .post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "little-sub",
            "session_id": "ses_sub",
            "parent_session_id": "ses_orch"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(child["parent_session_id"], "ses_orch");
    assert_eq!(child["orchestrator"], "big-orch");
    assert_eq!(child["orchestrator_session_id"], "ses_orch");

    let roster = get_roster(&c, &base, &auth).await;
    let orch = roster_row(&roster, "big-orch");
    assert!(orch.get("orchestrator").is_none() || orch["orchestrator"].is_null());

    let sub = roster_row(&roster, "little-sub");
    assert_eq!(sub["claim_count"], 0);
    assert_eq!(sub["session_id"], "ses_sub");
    assert_eq!(sub["parent_session_id"], "ses_orch");
    assert_eq!(sub["orchestrator"], "big-orch");
    assert_eq!(sub["orchestrator_session_id"], "ses_orch");

    // A name is enough when the caller knows the orchestrator but not its
    // session id; the server resolves it to the registered session.
    let named: Value = c
        .post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "named-sub",
            "session_id": "ses_named",
            "orchestrator": "BIG-ORCH"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(named["parent_session_id"], "ses_orch");
    assert_eq!(named["orchestrator"], "big-orch");

    // An unknown orchestrator name is ignored, not an error.
    let lost: Value = c
        .post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "lost-sub",
            "session_id": "ses_lost",
            "orchestrator": "nobody-registered"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(lost["agent_name"], "lost-sub");
    assert!(lost.get("parent_session_id").is_none() || lost["parent_session_id"].is_null());
}

/// An unregistered parent keeps the raw session id, and resolves later when
/// the orchestrator registers without any change to the child row.
#[tokio::test]
async fn unresolved_parent_keeps_raw_session_id() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "orphan-sub",
            "session_id": "ses_orphan",
            "parent_session_id": "ses_later"
        }))
        .send()
        .await
        .unwrap();

    let roster = get_roster(&c, &base, &auth).await;
    let child = roster_row(&roster, "orphan-sub");
    assert_eq!(child["parent_session_id"], "ses_later");
    assert!(child.get("orchestrator").is_none() || child["orchestrator"].is_null());
    assert!(
        child.get("orchestrator_session_id").is_none()
            || child["orchestrator_session_id"].is_null()
    );
    assert_eq!(child["claim_count"], 0);
    assert_eq!(child["session_id"], "ses_orphan");

    // Registering the parent later resolves the existing child row.
    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "late-orch", "session_id": "ses_later" }))
        .send()
        .await
        .unwrap();
    let roster = get_roster(&c, &base, &auth).await;
    let child = roster_row(&roster, "orphan-sub");
    assert_eq!(child["orchestrator"], "late-orch");
    assert_eq!(child["orchestrator_session_id"], "ses_later");
}

/// Hiding or killing a subagent affects only that name, never its orchestrator.
#[tokio::test]
async fn hiding_or_killing_child_leaves_orchestrator_alone() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "parent-orch", "session_id": "ses_parent" }))
        .send()
        .await
        .unwrap();
    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "child-agent",
            "session_id": "ses_child",
            "parent_session_id": "ses_parent"
        }))
        .send()
        .await
        .unwrap();

    c.post(format!("{base}/api/agents/child-agent/hide"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    let roster = get_roster(&c, &base, &auth).await;
    assert_eq!(roster_row(&roster, "child-agent")["hidden"], true);
    assert_eq!(roster_row(&roster, "parent-orch")["hidden"], false);
    assert_eq!(
        roster_row(&roster, "parent-orch")["orchestrator"],
        serde_json::Value::Null
    );

    c.post(format!("{base}/api/agents/child-agent/unhide"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    c.post(format!("{base}/api/agents/child-agent/kill"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    let roster = get_roster(&c, &base, &auth).await;
    assert!(
        roster
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["agent_name"] != "child-agent"),
        "removed child still on roster: {roster}"
    );
    assert_eq!(roster_row(&roster, "parent-orch")["paused"], false);
    assert_eq!(
        roster_row(&roster, "parent-orch")["orchestrator"],
        serde_json::Value::Null
    );
}

async fn get_roster(c: &reqwest::Client, base: &str, auth: &str) -> Value {
    c.get(format!("{base}/api/agents/roster"))
        .header("authorization", auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn claim_as(
    c: &reqwest::Client,
    base: &str,
    auth: &str,
    key: &str,
    agent: &str,
    mode: &str,
) -> reqwest::Response {
    c.post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", auth)
        .json(&json!({ "agent_name": agent, "mode": mode }))
        .send()
        .await
        .unwrap()
}

async fn register(c: &reqwest::Client, base: &str, auth: &str, body: Value) -> reqwest::Response {
    c.post(format!("{base}/api/agents/register"))
        .header("authorization", auth)
        .json(&body)
        .send()
        .await
        .unwrap()
}

/// The eleventh live exclusive claim for one name is refused, releasing one
/// frees capacity, and the roster exposes both budgets.
#[tokio::test]
async fn exclusive_claim_budget_per_agent_and_release() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let mut keys = Vec::new();
    for i in 0..11 {
        let issue = create_issue(&c, &base, &auth, &format!("budget item {i}")).await;
        keys.push(issue["key"].as_str().unwrap().to_string());
    }

    for key in &keys[..10] {
        let resp = claim_as(&c, &base, &auth, key, "greedy-loaf", "exclusive").await;
        assert_eq!(resp.status(), 201, "claim {key} should be allowed");
    }

    let refused = claim_as(&c, &base, &auth, &keys[10], "greedy-loaf", "exclusive").await;
    assert_eq!(refused.status(), 409, "eleventh claim must be refused");
    let body: Value = refused.json().await.unwrap();
    let message = body["error"].as_str().unwrap_or("");
    assert!(
        message.contains("10"),
        "limit missing from error: {message}"
    );
    assert!(
        message.contains("release"),
        "release guidance missing from error: {message}"
    );

    let roster = get_roster(&c, &base, &auth).await;
    let row = roster_row(&roster, "greedy-loaf");
    assert_eq!(row["exclusive_count"], 10);
    assert_eq!(row["exclusive_limit"], 10);
    assert_eq!(row["orchestrator_exclusive_count"], 10);
    assert_eq!(row["orchestrator_exclusive_limit"], 100);

    // Releasing one live claim frees exactly one slot.
    let released = c
        .delete(format!(
            "{base}/api/issues/{}/claim?note=done%3B%20next%3B%20risky",
            keys[0]
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(released.status(), 204);
    let allowed = claim_as(&c, &base, &auth, &keys[10], "greedy-loaf", "exclusive").await;
    assert_eq!(allowed.status(), 201, "release should free capacity");

    let roster = get_roster(&c, &base, &auth).await;
    assert_eq!(roster_row(&roster, "greedy-loaf")["exclusive_count"], 10);
}

/// Intents are reservations, not work: the per-agent cap never applies.
#[tokio::test]
async fn intent_claims_are_not_capped() {
    let app = tu::spawn_with_claim_limits(2, 5).await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    for i in 0..4 {
        let issue = create_issue(&c, &base, &auth, &format!("queued item {i}")).await;
        let key = issue["key"].as_str().unwrap();
        let resp = claim_as(&c, &base, &auth, key, "patient-rye", "intent").await;
        assert_eq!(resp.status(), 201, "intent {i} should be allowed");
    }

    let roster = get_roster(&c, &base, &auth).await;
    let row = roster_row(&roster, "patient-rye");
    assert_eq!(row["intent_count"], 4);
    assert_eq!(row["exclusive_count"], 0);
}

/// The hundred-and-first live exclusive claim across one orchestrator tree is
/// refused, even when the claiming subagent is far below its own cap.
#[tokio::test]
async fn orchestrator_subtree_budget_refuses_hundred_and_first() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    register(
        &c,
        &base,
        &auth,
        json!({ "agent_name": "tree-root", "session_id": "ses_tree_root" }),
    )
    .await;

    let children: Vec<String> = (0..12).map(|i| format!("tree-child-{i}")).collect();
    for (i, child) in children.iter().enumerate() {
        register(
            &c,
            &base,
            &auth,
            json!({
                "agent_name": child,
                "session_id": format!("ses_tree_child_{i}"),
                "parent_session_id": "ses_tree_root"
            }),
        )
        .await;
    }

    // 101 claimable issues plus one for the unlinked control.
    let mut keys = Vec::new();
    for i in 0..102 {
        let issue = create_issue(&c, &base, &auth, &format!("tree item {i}")).await;
        keys.push(issue["key"].as_str().unwrap().to_string());
    }

    let mut next = 0usize;
    for child in children.iter().take(11) {
        for _ in 0..9 {
            let resp = claim_as(&c, &base, &auth, &keys[next], child, "exclusive").await;
            assert_eq!(resp.status(), 201, "seeding {child} claim {next}");
            next += 1;
        }
    }
    assert_eq!(next, 99);

    // The hundredth claim in the tree is allowed.
    let hundredth = claim_as(&c, &base, &auth, &keys[next], &children[11], "exclusive").await;
    assert_eq!(hundredth.status(), 201, "hundredth tree claim should fit");
    next += 1;
    assert_eq!(next, 100);

    // The hundred-and-first is refused by the tree budget, not the per-agent
    // budget: this child holds only one claim.
    let refused = claim_as(&c, &base, &auth, &keys[next], &children[11], "exclusive").await;
    assert_eq!(refused.status(), 409, "101st tree claim must be refused");
    let body: Value = refused.json().await.unwrap();
    let message = body["error"].as_str().unwrap_or("");
    assert!(message.contains("100"), "tree limit missing: {message}");
    assert!(
        message.contains("release"),
        "release guidance missing: {message}"
    );

    let roster = get_roster(&c, &base, &auth).await;
    assert_eq!(
        roster_row(&roster, "tree-root")["orchestrator_exclusive_count"],
        100
    );
    assert_eq!(
        roster_row(&roster, &children[11])["orchestrator_exclusive_count"],
        100
    );

    // An unlinked agent is its own tree and still has room.
    let solo = claim_as(&c, &base, &auth, &keys[next], "solo-loaf", "exclusive").await;
    assert_eq!(
        solo.status(),
        201,
        "unlinked agent must not count in the tree"
    );
}

/// An unlinked agent is its own root: two of them never share one budget.
#[tokio::test]
async fn unlinked_agents_count_under_themselves_only() {
    let app = tu::spawn_with_claim_limits(10, 5).await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let mut keys = Vec::new();
    for i in 0..10 {
        let issue = create_issue(&c, &base, &auth, &format!("solo item {i}")).await;
        keys.push(issue["key"].as_str().unwrap().to_string());
    }

    for key in &keys[..5] {
        let resp = claim_as(&c, &base, &auth, key, "solo-one", "exclusive").await;
        assert_eq!(resp.status(), 201);
    }
    // If the two names shared a root, this would already exceed the 5-claim
    // tree budget.
    for key in &keys[5..] {
        let resp = claim_as(&c, &base, &auth, key, "solo-two", "exclusive").await;
        assert_eq!(
            resp.status(),
            201,
            "{key} should be under solo-two's own tree"
        );
    }

    let roster = get_roster(&c, &base, &auth).await;
    for name in ["solo-one", "solo-two"] {
        let row = roster_row(&roster, name);
        assert_eq!(row["exclusive_count"], 5);
        assert_eq!(row["orchestrator_exclusive_count"], 5);
        assert_eq!(row["orchestrator_exclusive_limit"], 5);
    }
}

/// BB-162: session links survive release, and a release requires a handoff
/// note unless it is an explicit no-work claim.
#[tokio::test]
async fn session_links_and_required_handoff() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let created = create_issue(&c, &base, &auth, "handoff target").await;
    let key = created["key"].as_str().unwrap().to_string();

    // Distinct names per session: one live name belongs to one session, so a
    // second session cannot reuse "handoff-agent" inside its 120 second
    // liveness window. The later session ids and handoff timestamps still
    // exercise the link and note behavior.
    let claim = |session: &str, agent: &str, state: &str, mode: &str, branch: Option<&str>| {
        let c = c.clone();
        let base = base.clone();
        let auth = auth.clone();
        let key = key.clone();
        let session = session.to_string();
        let agent = agent.to_string();
        let state = state.to_string();
        let mode = mode.to_string();
        let branch = branch.map(str::to_string);
        async move {
            let mut body = json!({
                "agent_name": agent,
                "session_id": session,
                "state": state,
                "mode": mode,
            });
            if let Some(branch) = branch {
                body["branch"] = json!(branch);
            }
            c.post(format!("{base}/api/issues/{key}/claim"))
                .header("authorization", &auth)
                .json(&body)
                .send()
                .await
                .unwrap()
        }
    };

    // Claim creating the session link.
    let first = claim(
        "ses_alpha",
        "handoff-agent",
        "implementing",
        "exclusive",
        None,
    )
    .await;
    assert_eq!(first.status(), 201);
    let issue: Value = c
        .get(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(issue["last_handoff"].is_null());
    let sessions = issue["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["session_id"], "ses_alpha");
    assert_eq!(sessions[0]["agent_name"], "handoff-agent");
    assert_eq!(sessions[0]["last_state"], "implementing");
    let first_seen = sessions[0]["first_seen"].as_str().unwrap().to_string();

    // Missing note is refused with the exact rule.
    let refused = c
        .delete(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);
    let body: Value = refused.json().await.unwrap();
    let message = body["error"].as_str().unwrap();
    assert!(message.contains("handoff note"), "{message}");
    assert!(message.contains("done"), "{message}");
    assert!(message.contains("no_note"), "{message}");

    // no_note is not a bypass for work that started.
    let bypass = c
        .delete(format!("{base}/api/issues/{key}/claim?no_note=true"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(bypass.status(), 400);
    let body: Value = bypass.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("no_note"));

    // A real note releases and is stored, shown and listed.
    let note = "done: core path; next: tests; risky: auth edge";
    let released = c
        .delete(format!(
            "{base}/api/issues/{key}/claim?note={}",
            urlencode(note)
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(released.status(), 204);

    let issue: Value = c
        .get(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(issue["last_handoff"]["note"], note);
    let sessions = issue["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1, "release must keep the session link");
    assert_eq!(sessions[0]["first_seen"], first_seen);
    assert_eq!(sessions[0]["last_state"], "implementing");

    let handoffs: Value = c
        .get(format!("{base}/api/issues/{key}/handoffs"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(handoffs.as_array().unwrap().len(), 1);
    assert_eq!(handoffs[0]["note"], note);

    let events: Value = c
        .get(format!(
            "{base}/api/agents/handoff-agent/events?kind=handoff"
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(events["events"][0]["kind"], "handoff");
    assert_eq!(events["events"][0]["summary"], note);

    // Back-link: the session knows the issue it touched.
    let back: Value = c
        .get(format!("{base}/api/sessions/ses_alpha/issues"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(back.as_array().unwrap()[0]["key"], key);

    // A planned exclusive claim with no evidence never held work: explicit
    // no_note is allowed and writes no handoff.
    assert_eq!(
        claim("ses_beta", "handoff-beta", "planned", "exclusive", None)
            .await
            .status(),
        201
    );
    let no_work = c
        .delete(format!("{base}/api/issues/{key}/claim?no_note=true"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(no_work.status(), 204);
    let handoffs: Value = c
        .get(format!("{base}/api/issues/{key}/handoffs"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(handoffs.as_array().unwrap().len(), 1);
    let issue: Value = c
        .get(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(issue["sessions"].as_array().unwrap().len(), 2);

    // A planned claim with a branch did hold work: no_note is refused.
    assert_eq!(
        claim(
            "ses_delta",
            "handoff-delta",
            "planned",
            "exclusive",
            Some("fix/x")
        )
        .await
        .status(),
        201
    );
    let bad = c
        .delete(format!("{base}/api/issues/{key}/claim?no_note=true"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
    let body: Value = bad.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("no_note"));
    let good = c
        .delete(format!(
            "{base}/api/issues/{key}/claim?note={}",
            urlencode("done: branch; next: review; risky: none")
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(good.status(), 204);

    // The most recent note is the resume context.
    let issue: Value = c
        .get(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        issue["last_handoff"]["note"],
        "done: branch; next: review; risky: none"
    );
    assert_eq!(issue["sessions"].as_array().unwrap().len(), 3);

    // An intent is always a no-work claim; releasing it without a note still
    // asks for the explicit opt-in.
    let intent = claim("ses_gamma", "handoff-gamma", "implementing", "intent", None).await;
    assert_eq!(intent.status(), 201);
    let refused = c
        .delete(format!("{base}/api/issues/{key}/claim?mode=intent"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);
    let intent_release = c
        .delete(format!(
            "{base}/api/issues/{key}/claim?mode=intent&no_note=true"
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(intent_release.status(), 204);
    let issue: Value = c
        .get(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(issue["sessions"].as_array().unwrap().len(), 4);
}

/// Set a name's orchestrator (or clear it) through the explicit endpoint.
async fn post_orchestrator(
    c: &reqwest::Client,
    base: &str,
    auth: &str,
    name: &str,
    orchestrator: Option<&str>,
) -> reqwest::Response {
    let body = match orchestrator {
        Some(value) => json!({ "orchestrator": value }),
        None => json!({}),
    };
    c.post(format!("{base}/api/agents/{name}/orchestrator"))
        .header("authorization", auth)
        .json(&body)
        .send()
        .await
        .unwrap()
}

async fn register_agent(
    c: &reqwest::Client,
    base: &str,
    auth: &str,
    name: &str,
    session: &str,
    parent: Option<&str>,
) -> Value {
    let mut body = json!({ "agent_name": name, "session_id": session });
    if let Some(parent) = parent {
        body["parent_session_id"] = json!(parent);
    }
    c.post(format!("{base}/api/agents/register"))
        .header("authorization", auth)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// Register a name with no OpenCode session id at all. Its registry row still
/// exists, which is what the name edge lives on.
async fn register_name_only(c: &reqwest::Client, base: &str, auth: &str, name: &str) -> Value {
    c.post(format!("{base}/api/agents/register"))
        .header("authorization", auth)
        .json(&json!({ "agent_name": name, "host": "test", "pid": 7 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn agent_events(c: &reqwest::Client, base: &str, auth: &str, name: &str) -> Value {
    c.get(format!("{base}/api/agents/{name}/events?kind=state_change"))
        .header("authorization", auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// The explicit endpoint sets a parent and clears it again. Clearing must stick
/// on a fresh roster read even though registration's COALESCE path cannot
/// remove a link, and hiding is not touched by either direction.
#[tokio::test]
async fn orchestrator_set_and_clear_persists() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    register_agent(&c, &base, &auth, "move-orch", "ses_move_orch", None).await;
    register_agent(&c, &base, &auth, "move-child", "ses_move_child", None).await;

    // Hidden is display-only state; a move must not reveal the card.
    c.post(format!("{base}/api/agents/move-child/hide"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();

    let moved: Value = post_orchestrator(&c, &base, &auth, "move-child", Some("move-orch"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(moved["agent_name"], "move-child");
    assert_eq!(moved["parent_session_id"], "ses_move_orch");
    assert_eq!(moved["parent_agent_name"], "move-orch");
    assert_eq!(moved["orchestrator"], "move-orch");
    assert_eq!(moved["orchestrator_session_id"], "ses_move_orch");
    assert_eq!(moved["hidden"], true);

    let roster = get_roster(&c, &base, &auth).await;
    let row = roster_row(&roster, "move-child");
    assert_eq!(row["parent_session_id"], "ses_move_orch");
    assert_eq!(row["parent_agent_name"], "move-orch");
    assert_eq!(row["orchestrator"], "move-orch");
    assert_eq!(row["hidden"], true);

    // The timeline names both sides of the change.
    let events = agent_events(&c, &base, &auth, "move-child").await;
    let summary = events["events"][0]["summary"].as_str().unwrap();
    assert!(
        summary.contains("none") && summary.contains("move-orch"),
        "set event names old and new: {summary}"
    );

    // Clearing is allowed and persists. The roster omits the null parent.
    let cleared: Value = post_orchestrator(&c, &base, &auth, "move-child", None)
        .await
        .json()
        .await
        .unwrap();
    assert!(
        cleared.get("parent_session_id").is_none() || cleared["parent_session_id"].is_null(),
        "clear removes parent_session_id: {cleared}"
    );
    assert!(
        cleared.get("parent_agent_name").is_none() || cleared["parent_agent_name"].is_null(),
        "clear removes parent_agent_name: {cleared}"
    );
    assert!(
        cleared.get("orchestrator").is_none() || cleared["orchestrator"].is_null(),
        "clear removes the resolved orchestrator: {cleared}"
    );
    assert_eq!(cleared["hidden"], true, "clearing does not unhide");

    let roster = get_roster(&c, &base, &auth).await;
    let row = roster_row(&roster, "move-child");
    assert!(row.get("parent_session_id").is_none() || row["parent_session_id"].is_null());
    assert_eq!(row["hidden"], true);

    let events = agent_events(&c, &base, &auth, "move-child").await;
    let summary = events["events"][0]["summary"].as_str().unwrap();
    assert!(
        summary.contains("move-orch") && summary.contains("none"),
        "clear event names old and new: {summary}"
    );

    // An unknown subject is a 404, not a create.
    let unknown = post_orchestrator(&c, &base, &auth, "never-registered", Some("move-orch")).await;
    assert_eq!(unknown.status(), 404);
}

/// A drop that would close a loop is refused with a message, and so is an
/// unknown or removed target. The roster is unchanged after each refusal.
#[tokio::test]
async fn orchestrator_refuses_cycles_and_unknown_targets() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    register_agent(&c, &base, &auth, "cycle-root", "ses_cycle_root", None).await;
    register_agent(
        &c,
        &base,
        &auth,
        "cycle-mid",
        "ses_cycle_mid",
        Some("ses_cycle_root"),
    )
    .await;
    register_agent(
        &c,
        &base,
        &auth,
        "cycle-leaf",
        "ses_cycle_leaf",
        Some("ses_cycle_mid"),
    )
    .await;

    // root -> leaf would close root <- mid <- leaf.
    let cycle = post_orchestrator(&c, &base, &auth, "cycle-root", Some("cycle-leaf")).await;
    assert_eq!(cycle.status(), 409, "a descendant target is a cycle");
    let body: Value = cycle.json().await.unwrap();
    assert!(
        body["error"].as_str().unwrap().contains("cycle"),
        "the refusal explains why: {body}"
    );

    // A self-link is the smallest cycle.
    let self_link = post_orchestrator(&c, &base, &auth, "cycle-mid", Some("cycle-mid")).await;
    assert_eq!(self_link.status(), 409);

    // Unknown names are refused in the body, not silently dropped.
    let unknown = post_orchestrator(&c, &base, &auth, "cycle-leaf", Some("ghost-agent")).await;
    assert_eq!(unknown.status(), 400);
    let body: Value = unknown.json().await.unwrap();
    assert!(
        body["error"].as_str().unwrap().contains("ghost-agent"),
        "the refusal names the target: {body}"
    );

    // A removed agent cannot be a drop target afterwards.
    register_agent(&c, &base, &auth, "doomed-orch", "ses_doomed", None).await;
    c.post(format!("{base}/api/agents/doomed-orch/kill"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    let removed = post_orchestrator(&c, &base, &auth, "cycle-leaf", Some("doomed-orch")).await;
    assert_eq!(removed.status(), 400);

    // Every refusal left the original tree alone.
    let roster = get_roster(&c, &base, &auth).await;
    assert_eq!(
        roster_row(&roster, "cycle-root")["parent_session_id"],
        serde_json::Value::Null
    );
    assert_eq!(
        roster_row(&roster, "cycle-mid")["parent_session_id"],
        "ses_cycle_root"
    );
    assert_eq!(
        roster_row(&roster, "cycle-leaf")["parent_session_id"],
        "ses_cycle_mid"
    );
}

/// Setting and clearing publish `agent.orchestrator` with both sides so every
/// open client can re-render without waiting for its 15 second poll.
#[tokio::test]
async fn orchestrator_change_publishes_events() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    register_agent(&c, &base, &auth, "event-parent", "ses_event_parent", None).await;
    register_agent(&c, &base, &auth, "event-child", "ses_event_child", None).await;

    let mut events = tu::EventStream::connect_with_auth(&base, &auth).await;

    post_orchestrator(&c, &base, &auth, "event-child", Some("event-parent")).await;
    let event = events.next_event().await;
    assert_eq!(event["type"], "agent.orchestrator");
    assert_eq!(event["agent"], "event-child");
    assert!(event["previous"].is_null());
    assert_eq!(event["orchestrator"], "event-parent");

    post_orchestrator(&c, &base, &auth, "event-child", None).await;
    let event = events.next_event().await;
    assert_eq!(event["type"], "agent.orchestrator");
    assert_eq!(event["agent"], "event-child");
    assert_eq!(event["previous"], "event-parent");
    assert!(event["orchestrator"].is_null());
}

/// BB-219: the hierarchy edge is organisational, not a property of the
/// OpenCode session. A sessionless card can be placed under a sessionful
/// orchestrator, and a sessionful card under a sessionless one. Both are
/// persisted as `parent_agent_name`; the session id stays only as a fallback.
#[tokio::test]
async fn orchestrator_links_sessionless_child_and_sessionless_target() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    register_agent(&c, &base, &auth, "session-orch", "ses_session_orch", None).await;
    register_name_only(&c, &base, &auth, "sessionless-child").await;

    let moved =
        post_orchestrator(&c, &base, &auth, "sessionless-child", Some("session-orch")).await;
    assert_eq!(moved.status(), 200, "a sessionless child is placeable");
    let moved: Value = moved.json().await.unwrap();
    assert_eq!(moved["parent_agent_name"], "session-orch");
    assert_eq!(moved["parent_session_id"], "ses_session_orch");
    assert_eq!(moved["orchestrator"], "session-orch");
    assert_eq!(moved["orchestrator_session_id"], "ses_session_orch");

    let roster = get_roster(&c, &base, &auth).await;
    let row = roster_row(&roster, "sessionless-child");
    assert_eq!(row["parent_agent_name"], "session-orch");
    assert_eq!(row["orchestrator"], "session-orch");

    // The reverse case: a sessionful child under a sessionless orchestrator.
    register_name_only(&c, &base, &auth, "name-only-orch").await;
    register_agent(&c, &base, &auth, "session-child", "ses_session_child", None).await;

    let moved = post_orchestrator(&c, &base, &auth, "session-child", Some("name-only-orch")).await;
    assert_eq!(moved.status(), 200, "a sessionless target accepts a drop");
    let moved: Value = moved.json().await.unwrap();
    assert_eq!(moved["parent_agent_name"], "name-only-orch");
    assert!(
        moved.get("parent_session_id").is_none() || moved["parent_session_id"].is_null(),
        "no fake session id is invented for a sessionless target: {moved}"
    );
    assert_eq!(moved["orchestrator"], "name-only-orch");
    assert!(
        moved.get("orchestrator_session_id").is_none()
            || moved["orchestrator_session_id"].is_null(),
        "the resolved orchestrator has no session: {moved}"
    );

    let roster = get_roster(&c, &base, &auth).await;
    let row = roster_row(&roster, "session-child");
    assert_eq!(row["parent_agent_name"], "name-only-orch");
    assert_eq!(row["orchestrator"], "name-only-orch");
}

/// The endpoint's target may be a registered session id rather than a name;
/// it resolves to the name edge and keeps the session for fallback.
#[tokio::test]
async fn orchestrator_endpoint_accepts_a_session_id_target() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    register_agent(&c, &base, &auth, "by-session-orch", "ses_by_session", None).await;
    register_agent(
        &c,
        &base,
        &auth,
        "by-session-child",
        "ses_by_session_child",
        None,
    )
    .await;

    let moved =
        post_orchestrator(&c, &base, &auth, "by-session-child", Some("ses_by_session")).await;
    assert_eq!(moved.status(), 200);
    let moved: Value = moved.json().await.unwrap();
    assert_eq!(moved["parent_agent_name"], "by-session-orch");
    assert_eq!(moved["parent_session_id"], "ses_by_session");
    assert_eq!(moved["orchestrator"], "by-session-orch");
}

/// Cycle prevention walks the mixed name/session graph, including a chain
/// where the middle link is a name and the leaf link is a session id.
#[tokio::test]
async fn orchestrator_refuses_cycles_in_the_mixed_graph() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    register_name_only(&c, &base, &auth, "mixed-root").await;
    register_agent(&c, &base, &auth, "mixed-mid", "ses_mixed_mid", None).await;
    let set = post_orchestrator(&c, &base, &auth, "mixed-mid", Some("mixed-root")).await;
    assert_eq!(set.status(), 200);
    // The leaf is linked by the session edge, so the walk must cross both
    // representations to spot the loop.
    register_agent(
        &c,
        &base,
        &auth,
        "mixed-leaf",
        "ses_mixed_leaf",
        Some("ses_mixed_mid"),
    )
    .await;

    let cycle = post_orchestrator(&c, &base, &auth, "mixed-root", Some("mixed-leaf")).await;
    assert_eq!(
        cycle.status(),
        409,
        "mixed names and sessions still form a cycle"
    );
    let body: Value = cycle.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("cycle"), "{body}");

    let roster = get_roster(&c, &base, &auth).await;
    let mid = roster_row(&roster, "mixed-mid");
    assert_eq!(mid["parent_agent_name"], "mixed-root");
    assert!(mid["parent_session_id"].is_null());
    let leaf = roster_row(&roster, "mixed-leaf");
    assert_eq!(leaf["parent_session_id"], "ses_mixed_mid");

    // A pure name chain refuses the same way.
    for name in ["name-a", "name-b", "name-c"] {
        register_name_only(&c, &base, &auth, name).await;
    }
    post_orchestrator(&c, &base, &auth, "name-b", Some("name-a")).await;
    post_orchestrator(&c, &base, &auth, "name-c", Some("name-b")).await;
    let cycle = post_orchestrator(&c, &base, &auth, "name-a", Some("name-c")).await;
    assert_eq!(cycle.status(), 409);
}

/// Renaming an orchestrator moves every name edge that points at it, the same
/// way rename already moves claims, profiles and chat membership.
#[tokio::test]
async fn rename_moves_name_based_parent_edges() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    register_name_only(&c, &base, &auth, "edge-parent").await;
    register_name_only(&c, &base, &auth, "edge-child").await;
    let set = post_orchestrator(&c, &base, &auth, "edge-child", Some("edge-parent")).await;
    assert_eq!(set.status(), 200);

    let renamed = c
        .post(format!("{base}/api/agents/edge-parent/rename"))
        .header("authorization", &auth)
        .json(&json!({ "new_name": "edge-parent-renamed" }))
        .send()
        .await
        .unwrap();
    assert_eq!(renamed.status(), 200);

    let roster = get_roster(&c, &base, &auth).await;
    let child = roster_row(&roster, "edge-child");
    assert_eq!(
        child["parent_agent_name"], "edge-parent-renamed",
        "the name edge followed the rename"
    );
    assert_eq!(child["orchestrator"], "edge-parent-renamed");
}

/// Removing an orchestrator detaches its children instead of leaving a name
/// edge that a later agent could inherit by reusing the name.
#[tokio::test]
async fn removing_an_orchestrator_detaches_name_edges() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    register_name_only(&c, &base, &auth, "doomed-name-orch").await;
    register_name_only(&c, &base, &auth, "orphaned-name-child").await;
    post_orchestrator(
        &c,
        &base,
        &auth,
        "orphaned-name-child",
        Some("doomed-name-orch"),
    )
    .await;

    let killed = c
        .post(format!("{base}/api/agents/doomed-name-orch/kill"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(killed.status(), 200);

    let roster = get_roster(&c, &base, &auth).await;
    let child = roster_row(&roster, "orphaned-name-child");
    assert!(
        child.get("parent_agent_name").is_none() || child["parent_agent_name"].is_null(),
        "the removed parent's name edge is gone: {child}"
    );
    assert!(
        child.get("orchestrator").is_none() || child["orchestrator"].is_null(),
        "the removed parent is not displayed as an orchestrator: {child}"
    );
}

fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "%20".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}
