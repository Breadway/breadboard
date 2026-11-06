//! Agent claims: exclusive presence, board side-effects, heartbeats, status.

mod common;
use common as tu;
use serde_json::{json, Value};

async fn create_issue(c: &reqwest::Client, base: &str, auth: &str, title: &str) -> Value {
    c.post(format!("{base}/api/issues"))
        .header("authorization", auth)
        .json(&json!({ "title": title, "project": "ROAD" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn claim_lifecycle() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

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

    let created = create_issue(&c, &base, &auth, "sse claim item").await;
    let key = created["key"].as_str().unwrap().to_string();

    let mut events = tu::EventStream::connect(&base).await;
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
    assert_eq!(events.next_type().await, "agent.updated");

    c.delete(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(events.next_type().await, "agent.released");
}
