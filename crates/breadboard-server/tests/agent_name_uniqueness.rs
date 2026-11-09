//! BB-170: a live agent name belongs to exactly one session.
//!
//! The roster still keys by lowercased name, so the server must refuse a
//! second live session that tries to take a name rather than letting it
//! overwrite the holder's session id. A stale holder is reclaimable once its
//! 120 second heartbeat window has passed.

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

async fn register(
    c: &reqwest::Client,
    base: &str,
    auth: &str,
    name: &str,
    session: &str,
) -> reqwest::Response {
    c.post(format!("{base}/api/agents/register"))
        .header("authorization", auth)
        .json(&json!({ "agent_name": name, "session_id": session }))
        .send()
        .await
        .unwrap()
}

async fn roster_row(c: &reqwest::Client, base: &str, auth: &str, name: &str) -> Value {
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
        .find(|row| row["agent_name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("roster row {name} not found in {roster}"))
}

#[tokio::test]
async fn second_live_session_with_same_name_is_refused() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let first = register(&c, &base, &auth, "shared-name", "ses_first").await;
    assert!(first.status().is_success(), "{}", first.status());

    let refused = register(&c, &base, &auth, "Shared-Name", "ses_second").await;
    assert_eq!(refused.status(), 409);
    let body: Value = refused.json().await.unwrap();
    let error = body["error"].as_str().unwrap();
    assert!(error.contains("ses_first"), "{error}");
    assert!(error.contains("choose another name"), "{error}");
    assert_eq!(body["agent_name"], "shared-name");
    assert_eq!(body["session_id"], "ses_first");

    let alternatives: Vec<&str> = body["alternatives"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        (2..=3).contains(&alternatives.len()),
        "alternatives: {alternatives:?}"
    );
    assert!(!alternatives.contains(&"shared-name"));

    // The holder still owns the name: the second session did not overwrite it.
    let row = roster_row(&c, &base, &auth, "shared-name").await;
    assert_eq!(row["session_id"], "ses_first");
    assert_eq!(row["live"], true);
}

#[tokio::test]
async fn stale_name_is_reclaimable_after_the_live_window() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let first = register(&c, &base, &auth, "sleepy-name", "ses_asleep").await;
    assert!(first.status().is_success());

    sqlx::query("UPDATE agent_sessions SET last_seen = ?1 WHERE agent_name = 'sleepy-name'")
        .bind((chrono::Utc::now() - chrono::Duration::seconds(600)).to_rfc3339())
        .execute(&app.db)
        .await
        .unwrap();

    let reclaimed = register(&c, &base, &auth, "sleepy-name", "ses_awake").await;
    assert_eq!(reclaimed.status(), 200);
    let body: Value = reclaimed.json().await.unwrap();
    assert_eq!(body["session_id"], "ses_awake");

    let row = roster_row(&c, &base, &auth, "sleepy-name").await;
    assert_eq!(row["session_id"], "ses_awake");
}

#[tokio::test]
async fn rename_then_claim_under_the_new_name_works() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let issue = create_issue(&c, &base, &auth, "rename then claim").await;
    let key = issue["key"].as_str().unwrap().to_string();

    let reg = register(&c, &base, &auth, "before-name", "ses_rename").await;
    assert!(reg.status().is_success());

    // This is what name_agent does before claiming: rename the existing
    // session (moving its profile) and then claim under the new name.
    let renamed = c
        .post(format!("{base}/api/agents/before-name/rename"))
        .header("authorization", &auth)
        .json(&json!({ "new_name": "after-name" }))
        .send()
        .await
        .unwrap();
    assert_eq!(renamed.status(), 200);

    let claimed = c
        .post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "after-name",
            "session_id": "ses_rename",
            "state": "implementing"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(claimed.status(), 201);
    let claim: Value = claimed.json().await.unwrap();
    assert_eq!(claim["agent_name"], "after-name");
    assert_eq!(claim["session_id"], "ses_rename");
}

#[tokio::test]
async fn claim_under_a_name_held_by_another_live_session_is_refused() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    ensure_project(&c, &base, &auth).await;
    let issue = create_issue(&c, &base, &auth, "held name claim").await;
    let key = issue["key"].as_str().unwrap().to_string();

    let reg = register(&c, &base, &auth, "held-name", "ses_holder").await;
    assert!(reg.status().is_success());

    let refused = c
        .post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "held-name", "session_id": "ses_intruder" }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 409);
    let body: Value = refused.json().await.unwrap();
    assert!(
        body["error"].as_str().unwrap().contains("ses_holder"),
        "{body}"
    );
    assert_eq!(body["session_id"], "ses_holder");

    // The refusal happens before the claim is inserted and before the card
    // moves to doing.
    let refreshed: Value = c
        .get(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(refreshed["status"], "backlog");
    let claims: Value = c
        .get(format!("{base}/api/agents"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(claims.as_array().unwrap().is_empty(), "{claims}");
}

#[tokio::test]
async fn chat_under_a_name_held_by_another_live_session_is_refused() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let reg = register(&c, &base, &auth, "talker-name", "ses_talker").await;
    assert!(reg.status().is_success());

    let send = |session: &'static str| {
        let c = c.clone();
        let base = base.clone();
        let auth = auth.clone();
        async move {
            c.post(format!("{base}/api/chat/messages"))
                .header("authorization", &auth)
                .json(&json!({
                    "agent_name": "talker-name",
                    "session_id": session,
                    "room": "lobby",
                    "body": "hello"
                }))
                .send()
                .await
                .unwrap()
        }
    };

    let refused = send("ses_other").await;
    assert_eq!(refused.status(), 409);
    let body: Value = refused.json().await.unwrap();
    assert!(
        body["error"].as_str().unwrap().contains("ses_talker"),
        "{body}"
    );

    let accepted = send("ses_talker").await;
    assert_eq!(accepted.status(), 201);
}
