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
    assert_eq!(reg["killed"], false);

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

/// Kill releases every claim, blocks writes, is idempotent, and revives.
#[tokio::test]
async fn kill_releases_claims_blocks_writes_and_is_idempotent() {
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

    let killed: Value = c
        .post(format!("{base}/api/agents/victim/kill"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(killed["killed"], true);
    assert_eq!(killed["status"], "killed");
    assert_eq!(killed["killed_by_login"], "tester");
    assert_eq!(killed["claim_count"], 0);

    // Claims are gone.
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

    // Second kill is a no-op and keeps the original killer.
    let again: Value = c
        .post(format!("{base}/api/agents/victim/kill"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(again["killed"], true);
    assert_eq!(again["killed_by_login"], "tester");
    assert_eq!(again["killed_at"], killed["killed_at"]);

    // The next agent-scoped writes are refused with a clear message.
    let blocked_claim = c
        .post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "victim" }))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked_claim.status(), 403);
    let body: Value = blocked_claim.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("killed"), "{body}");

    let blocked_message = c
        .put(format!("{base}/api/agents/message"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "victim", "message": "still here" }))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked_message.status(), 403);

    // Registration still refreshes the row, but does not revive it.
    let reg: Value = c
        .post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "victim", "session_id": "ses_victim" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reg["killed"], true);

    // Revive is idempotent and the name can claim again.
    for _ in 0..2 {
        let revived: Value = c
            .post(format!("{base}/api/agents/victim/revive"))
            .header("authorization", &auth)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(revived["killed"], false);
    }
    let reclaimed = c
        .post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "victim" }))
        .send()
        .await
        .unwrap();
    assert_eq!(reclaimed.status(), 201);
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

/// Killing publishes `agent.killed` on the SSE stream.
#[tokio::test]
async fn kill_publishes_event() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let mut events = tu::EventStream::connect_with_auth(&base, &auth).await;
    c.post(format!("{base}/api/agents/event-target/kill"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    let event = events.next_event().await;
    assert_eq!(event["type"], "agent.killed");
    assert_eq!(event["agent"], "event-target");
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
    assert_eq!(roster_row(&roster, "child-agent")["killed"], true);
    assert_eq!(roster_row(&roster, "parent-orch")["killed"], false);
    assert_eq!(
        roster_row(&roster, "child-agent")["orchestrator"],
        "parent-orch"
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
