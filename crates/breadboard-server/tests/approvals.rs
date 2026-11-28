//! Approval asks: the agent/operator state machine, one-shot decisions,
//! revisions, expiry, attachments and maintenance windows.
//!
//! The product rules live on the server, so these exercise HTTP refusals rather
//! than trusting the web client.

mod common;
use std::sync::{Arc, Mutex};

use breadboard_server::approvals;
use breadboard_server::auth::sha256_hex;
use chrono::{Duration, Utc};
use common as tu;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Fixture {
    base: String,
    auth: String,
    key: String,
    agent: String,
    session: String,
}

async fn fixture(app: &tu::TestApp) -> Fixture {
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let _ = c
        .post(format!("{base}/api/projects"))
        .header("authorization", &auth)
        .json(&json!({ "key": "BREAD", "name": "bread" }))
        .send()
        .await
        .unwrap();
    let created: Value = c
        .post(format!("{base}/api/issues"))
        .header("authorization", &auth)
        .json(&json!({ "title": "ship the thing", "project": "BREAD" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = created["key"].as_str().unwrap().to_string();

    let agent = "crumb scout".to_string();
    let session = "ses_approval_fixture".to_string();
    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": agent, "session_id": session, "host": "hestia" }))
        .send()
        .await
        .unwrap();
    Fixture {
        base,
        auth,
        key,
        agent,
        session,
    }
}

fn ask_body(f: &Fixture, title: &str) -> Value {
    json!({
        "agent_name": f.agent,
        "session_id": f.session,
        "issue": f.key,
        "kind": "approval",
        "urgency": "high",
        "title": title,
        "summary": "A short summary",
        "body_md": "### Why\n\nBecause **the runner** is stale.\n\n1. drain\n2. rotate",
        "checklist": ["Approve the pause.", "I will not touch the database."],
        "risk_md": "Low: the runner is idle.",
        "rollback_md": "Restore the previous unit.",
        "deadline": "today 18:00"
    })
}

async fn create_ask(c: &reqwest::Client, f: &Fixture, body: Value) -> Value {
    let response = c
        .post(format!("{}/api/approvals", f.base))
        .header("authorization", &f.auth)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201, "{}", response.text().await.unwrap());
    response.json().await.unwrap()
}

async fn decide(c: &reqwest::Client, f: &Fixture, id: i64, body: Value) -> reqwest::Response {
    c.post(format!("{}/api/approvals/{id}/decision", f.base))
        .header("authorization", &f.auth)
        .json(&body)
        .send()
        .await
        .unwrap()
}

async fn install_second_user(app: &tu::TestApp, login: &str, token: &str) -> String {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO users (forgejo_id, login, display_name) VALUES
            ((SELECT COALESCE(MAX(forgejo_id), 0) + 1 FROM users), ?1, ?1) RETURNING id",
    )
    .bind(login)
    .fetch_one(&app.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO api_tokens (user_id, name, token_hash, created_at) VALUES (?1, 'test', ?2, ?3)",
    )
    .bind(id)
    .bind(sha256_hex(token))
    .bind(Utc::now().to_rfc3339())
    .execute(&app.db)
    .await
    .unwrap();
    format!("Bearer {token}")
}

#[tokio::test]
async fn agent_opens_and_operator_approves_exactly_once() {
    let app = tu::spawn().await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();

    let ask = create_ask(&c, &f, ask_body(&f, "Rotate the runner token")).await;
    assert_eq!(ask["status"], "pending");
    assert_eq!(ask["revision"], 1);
    assert_eq!(ask["agent_name"], f.agent);
    assert_eq!(ask["issue_key"], f.key);
    assert_eq!(ask["project_key"], "BREAD");
    assert_eq!(ask["events"][0]["kind"], "created");
    assert_eq!(ask["checklist"].as_array().unwrap().len(), 2);
    let id = ask["id"].as_i64().unwrap();

    // The operator inbox sees it, unread, and opening it advances the cursor.
    let counts: Value = c
        .get(format!("{}/api/approvals/counts", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(counts["needs"], 1);
    assert_eq!(counts["unread"], 1);
    assert_eq!(counts["approval"], 1);

    let opened: Value = c
        .get(format!("{}/api/approvals/{id}", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(opened["unread"], true);
    let after_read: Value = c
        .get(format!("{}/api/approvals/counts", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after_read["unread"], 0);

    let decided = decide(
        &c,
        &f,
        id,
        json!({ "decision": "approve", "note_md": "Go ahead, keep it short." }),
    )
    .await;
    assert_eq!(decided.status(), 200);
    let decided: Value = decided.json().await.unwrap();
    assert_eq!(decided["status"], "approved");
    assert_eq!(decided["decided_by_login"], "tester");
    let kinds: Vec<&str> = decided["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["created", "approved"]);
    assert_eq!(decided["decision_note_md"], "Go ahead, keep it short.");

    // One shot: a second decision cannot overwrite the first.
    let again = decide(&c, &f, id, json!({ "decision": "decline" })).await;
    assert_eq!(again.status(), 409, "{}", again.text().await.unwrap());
    let counts: Value = c
        .get(format!("{}/api/approvals/counts", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(counts["needs"], 0);
}

#[tokio::test]
async fn decisions_refuse_agents_and_non_operators() {
    let app = tu::spawn_with_config(|cfg| cfg.approvals.operators = vec!["tester".into()]).await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();
    let ask = create_ask(&c, &f, ask_body(&f, "Needs a human")).await;
    let id = ask["id"].as_i64().unwrap();

    // An agent presenting its own identity cannot decide, even its own ask.
    let refused = decide(
        &c,
        &f,
        id,
        json!({ "decision": "approve", "agent_name": f.agent }),
    )
    .await;
    assert_eq!(refused.status(), 403);

    // A human who is not a configured operator is refused too.
    let auditor = install_second_user(&app, "auditor", "auditor-token-abcdef").await;
    let refused = c
        .post(format!("{}/api/approvals/{id}/decision", f.base))
        .header("authorization", &auditor)
        .json(&json!({ "decision": "approve" }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);

    // The configured operator still can.
    let allowed = decide(&c, &f, id, json!({ "decision": "approve" })).await;
    assert_eq!(allowed.status(), 200);

    // Another human cannot speak as an agent registered to a different user.
    let auditor = install_second_user(&app, "auditor2", "auditor2-token-abcdef").await;
    let refused = c
        .post(format!("{}/api/approvals", f.base))
        .header("authorization", &auditor)
        .json(&ask_body(&f, "Not mine"))
        .send()
        .await
        .unwrap();
    assert!(
        matches!(refused.status().as_u16(), 403 | 409),
        "a shared human session cannot create an ask as someone else's agent"
    );

    // Unauthenticated calls never reach the state machine.
    let refused = c
        .post(format!("{}/api/approvals/{id}/decision", f.base))
        .json(&json!({ "decision": "approve" }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 401);
}

#[tokio::test]
async fn changes_hide_the_ask_until_the_same_row_returns_as_a_revision() {
    let app = tu::spawn().await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();
    let ask = create_ask(&c, &f, ask_body(&f, "Add a webhook receiver")).await;
    let id = ask["id"].as_i64().unwrap();

    let changes = decide(
        &c,
        &f,
        id,
        json!({ "decision": "changes", "note_md": "Use HMAC, not a secret in the URL." }),
    )
    .await;
    assert_eq!(changes.status(), 200);
    let changes: Value = changes.json().await.unwrap();
    assert_eq!(changes["status"], "changes");

    let needs: Vec<Value> = c
        .get(format!("{}/api/approvals?filter=needs", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(needs.is_empty(), "changes requested hides the ask from Needs you");
    let hidden: Vec<Value> = c
        .get(format!("{}/api/approvals?filter=changes", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(hidden.len(), 1);
    assert_eq!(hidden[0]["id"], json!(id));

    // The same row comes back with a new revision and the full thread.
    let revised = c
        .post(format!("{}/api/approvals/{id}/revisions", f.base))
        .header("authorization", &f.auth)
        .json(&json!({
            "agent_name": f.agent,
            "session_id": f.session,
            "summary": "Reworked to HMAC with a timestamp window.",
            "body_md": "### Rework\n\nNow signed.",
            "checklist": ["HMAC header", "Replay window"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(revised.status(), 200, "{}", revised.text().await.unwrap());
    let revised: Value = revised.json().await.unwrap();
    assert_eq!(revised["id"], json!(id), "the same ask returns");
    assert_eq!(revised["status"], "pending");
    assert_eq!(revised["revision"], 2);
    assert_eq!(revised["summary"], "Reworked to HMAC with a timestamp window.");
    let kinds: Vec<&str> = revised["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["created", "changes", "revised"]);

    let needs: Vec<Value> = c
        .get(format!("{}/api/approvals?filter=needs", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(needs.len(), 1);
    assert_eq!(needs[0]["revision"], 2);

    let approved = decide(&c, &f, id, json!({ "decision": "approve" })).await;
    assert_eq!(approved.status(), 200);
    // An approved ask is terminal for the agent too.
    let too_late = c
        .post(format!("{}/api/approvals/{id}/revisions", f.base))
        .header("authorization", &f.auth)
        .json(&json!({ "agent_name": f.agent }))
        .send()
        .await
        .unwrap();
    assert_eq!(too_late.status(), 409);
}

#[tokio::test]
async fn send_info_closes_and_withdraw_is_the_agents_own() {
    let app = tu::spawn().await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();

    let ask = create_ask(&c, &f, {
        let mut body = ask_body(&f, "Which token is authoritative?");
        body["kind"] = json!("info");
        body
    })
    .await;
    let id = ask["id"].as_i64().unwrap();

    let answered = decide(
        &c,
        &f,
        id,
        json!({ "decision": "info", "note_md": "The mirror file is authoritative." }),
    )
    .await;
    assert_eq!(answered.status(), 200);
    let answered: Value = answered.json().await.unwrap();
    assert_eq!(answered["status"], "answered");
    assert_eq!(
        answered["events"].as_array().unwrap().last().unwrap()["kind"],
        "info"
    );

    let reopen = c
        .post(format!("{}/api/approvals/{id}/revisions", f.base))
        .header("authorization", &f.auth)
        .json(&json!({ "agent_name": f.agent }))
        .send()
        .await
        .unwrap();
    assert_eq!(reopen.status(), 409, "send info closes the ask");

    // A second ask can be withdrawn by the owning agent only.
    let ask = create_ask(&c, &f, ask_body(&f, "Withdraw me")).await;
    let id = ask["id"].as_i64().unwrap();
    let wrong = c
        .post(format!("{}/api/approvals/{id}/withdraw", f.base))
        .header("authorization", &f.auth)
        .json(&json!({ "agent_name": "other agent" }))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 403);

    let withdrawn = c
        .post(format!("{}/api/approvals/{id}/withdraw", f.base))
        .header("authorization", &f.auth)
        .json(&json!({ "agent_name": f.agent, "note": "No longer needed." }))
        .send()
        .await
        .unwrap();
    assert_eq!(withdrawn.status(), 200);
    let withdrawn: Value = withdrawn.json().await.unwrap();
    assert_eq!(withdrawn["status"], "withdrawn");
    let decide_after = decide(&c, &f, id, json!({ "decision": "approve" })).await;
    assert_eq!(decide_after.status(), 409);
}

#[tokio::test]
async fn one_shot_expiry_expires_a_lapsed_pending_ask() {
    let app = tu::spawn().await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();

    let mut body = ask_body(&f, "Expired before a decision");
    body["expires_at"] = json!((Utc::now() - Duration::minutes(1)).to_rfc3339());
    let ask = create_ask(&c, &f, body).await;
    let id = ask["id"].as_i64().unwrap();

    let fetched: Value = c
        .get(format!("{}/api/approvals/{id}", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(fetched["status"], "expired");
    assert_eq!(
        fetched["events"].as_array().unwrap().last().unwrap()["kind"],
        "expired"
    );
    let refused = decide(&c, &f, id, json!({ "decision": "approve" })).await;
    assert_eq!(refused.status(), 409);

    // A future expiry is still decidable.
    let mut body = ask_body(&f, "Expires later");
    body["expires_at"] = json!((Utc::now() + Duration::hours(2)).to_rfc3339());
    let ask = create_ask(&c, &f, body).await;
    let id = ask["id"].as_i64().unwrap();
    let approved = decide(&c, &f, id, json!({ "decision": "approve" })).await;
    assert_eq!(approved.status(), 200);
}

#[tokio::test]
async fn a_revision_sets_and_explicitly_clears_expiry_and_deadline() {
    let app = tu::spawn().await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();
    let expiry = Utc::now() + Duration::hours(3);
    let mut body = ask_body(&f, "Expiry contract");
    body["expires_at"] = json!(expiry.to_rfc3339());
    body["deadline"] = json!("today 18:00");
    let ask = create_ask(&c, &f, body).await;
    let id = ask["id"].as_i64().unwrap();
    assert!(ask["expires_at"].is_string());

    // An explicit null clears both; this is what the MCP tool promises.
    let cleared = c
        .post(format!("{}/api/approvals/{id}/revisions", f.base))
        .header("authorization", &f.auth)
        .json(&json!({
            "agent_name": f.agent,
            "expires_at": null,
            "deadline": null,
            "summary": "No deadline any more."
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(cleared.status(), 200, "{}", cleared.text().await.unwrap());
    let cleared: Value = cleared.json().await.unwrap();
    assert!(cleared["expires_at"].is_null(), "a null expiry must clear it");
    assert!(cleared["deadline"].is_null(), "a null deadline must clear it");
    assert_eq!(cleared["revision"], 2);

    // Omitted fields keep the cleared values.
    let unchanged = c
        .post(format!("{}/api/approvals/{id}/revisions", f.base))
        .header("authorization", &f.auth)
        .json(&json!({ "agent_name": f.agent, "summary": "Still no deadline." }))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert!(unchanged["expires_at"].is_null());
    assert!(unchanged["deadline"].is_null());

    // Both can be set again on the next revision.
    let later = Utc::now() + Duration::hours(5);
    let reset: Value = c
        .post(format!("{}/api/approvals/{id}/revisions", f.base))
        .header("authorization", &f.auth)
        .json(&json!({
            "agent_name": f.agent,
            "expires_at": later.to_rfc3339(),
            "deadline": "before the freeze",
            "summary": "A new deadline."
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(reset["expires_at"].is_string());
    assert_eq!(reset["deadline"], "before the freeze");
    let stored: Value = c
        .get(format!("{}/api/approvals/{id}", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stored["deadline"], "before the freeze");
    assert!(stored["expires_at"].is_string());
}

#[tokio::test]
async fn screenshots_use_existing_artifacts_by_id_and_base64() {
    let app = tu::spawn().await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();

    // 1x1 png.
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==";
    let artifact: Value = c
        .post(format!("{}/api/issues/{}/artifacts/base64", f.base, f.key))
        .header("authorization", &f.auth)
        .json(&json!({
            "filename": "before.png",
            "kind": "screenshot",
            "data_base64": png,
            "mime": "image/png",
            "agent_name": f.agent,
            "session_id": f.session
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let artifact_id = artifact["id"].as_i64().unwrap();

    let mut body = ask_body(&f, "With a screenshot");
    body["attachments"] = json!([
        { "artifact_id": artifact_id, "caption": "before the change" },
        { "filename": "after.png", "mime": "image/png", "data_base64": png, "caption": "after" }
    ]);
    let ask = create_ask(&c, &f, body).await;
    let attachments = ask["attachments"].as_array().unwrap();
    assert_eq!(attachments.len(), 2);
    assert_eq!(attachments[0]["artifact_id"], json!(artifact_id));
    assert_eq!(attachments[0]["caption"], "before the change");
    assert_eq!(attachments[1]["caption"], "after");
    assert_eq!(attachments[1]["mime"], "image/png");
    let uploaded_id = attachments[1]["artifact_id"].as_i64().unwrap();

    let download = c
        .get(format!("{}/api/artifacts/{uploaded_id}", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap();
    assert_eq!(download.status(), 200);
    assert_eq!(download.headers()["content-type"], "image/png");
    assert!(!download.bytes().await.unwrap().is_empty());

    // An artifact from another issue and an SVG upload are both refused.
    let other: Value = c
        .post(format!("{}/api/issues", f.base))
        .header("authorization", &f.auth)
        .json(&json!({ "title": "other", "project": "BREAD" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_key = other["key"].as_str().unwrap();
    let other_artifact: Value = c
        .post(format!("{}/api/issues/{other_key}/artifacts/base64", f.base))
        .header("authorization", &f.auth)
        .json(&json!({
            "filename": "other.png",
            "kind": "screenshot",
            "data_base64": png,
            "mime": "image/png",
            "agent_name": f.agent
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut body = ask_body(&f, "Wrong issue artifact");
    body["attachments"] = json!([{ "artifact_id": other_artifact["id"] }]);
    let refused = c
        .post(format!("{}/api/approvals", f.base))
        .header("authorization", &f.auth)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);

    let mut body = ask_body(&f, "SVG is refused");
    body["attachments"] =
        json!([{ "filename": "x.svg", "mime": "image/svg+xml", "data_base64": png }]);
    let refused = c
        .post(format!("{}/api/approvals", f.base))
        .header("authorization", &f.auth)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 415);
}

#[tokio::test]
async fn maintenance_window_lifecycle_and_freeze_overlap_is_allowed() {
    let app = tu::spawn_with_config(|cfg| cfg.approvals.operators = vec!["tester".into()]).await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();
    let start = Utc::now() + Duration::days(2);
    let end = start + Duration::hours(2);

    let mut body = ask_body(&f, "Nextcloud MySQL 8.4 window");
    body["kind"] = json!("maintenance");
    body["window"] = json!({
        "title": "Nextcloud MySQL 8.4",
        "starts_at": start.to_rfc3339(),
        "ends_at": end.to_rfc3339(),
        "timezone": "Australia/Perth",
        "services": ["nextcloud", "mysql"],
        "reason": "Move to 8.4 before the freeze.",
        "risks": "Nextcloud is down for the window.",
        "rollback": "Restore the snapshot.",
        "runbook_md": "1. Announce\n2. Snapshot\n3. Upgrade\n4. Smoke test"
    });
    let ask = create_ask(&c, &f, body).await;
    let id = ask["id"].as_i64().unwrap();
    let window = &ask["window"];
    assert_eq!(window["status"], "proposed");
    assert_eq!(window["services"][0], "nextcloud");
    let window_id = window["id"].as_i64().unwrap();

    // The calendar range endpoint sees it.
    let from = (start - Duration::days(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let to = (end + Duration::days(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let list: Vec<Value> = c
        .get(format!(
            "{}/api/maintenance/windows?from={from}&to={to}",
            f.base
        ))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"], json!(window_id));

    // The agent can drag the proposal before a decision.
    let moved_resp = c
        .patch(format!("{}/api/maintenance/windows/{window_id}", f.base))
        .header("authorization", &f.auth)
        .json(&json!({
            "agent_name": f.agent,
            "starts_at": (start + Duration::hours(1)).to_rfc3339(),
            "ends_at": (end + Duration::hours(1)).to_rfc3339()
        }))
        .send()
        .await
        .unwrap();
    let moved_status = moved_resp.status();
    let moved_text = moved_resp.text().await.unwrap();
    assert!(moved_status.is_success(), "window patch: {moved_status} {moved_text}");
    let moved: Value = serde_json::from_str(&moved_text).unwrap();
    let moved_start: chrono::DateTime<Utc> = serde_json::from_value(moved["starts_at"].clone()).unwrap();
    assert_eq!(moved_start, start + Duration::hours(1));

    // Freeze creation is operator-only.
    let auditor = install_second_user(&app, "auditor3", "auditor3-token-abcdef").await;
    let refused = c
        .post(format!("{}/api/maintenance/freezes", f.base))
        .header("authorization", &auditor)
        .json(&json!({
            "label": "December change freeze",
            "starts_at": (start - Duration::days(1)).to_rfc3339(),
            "ends_at": (end + Duration::days(3)).to_rfc3339()
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    let freeze: Value = c
        .post(format!("{}/api/maintenance/freezes", f.base))
        .header("authorization", &f.auth)
        .json(&json!({
            "label": "December change freeze",
            "starts_at": (start - Duration::days(1)).to_rfc3339(),
            "ends_at": (end + Duration::days(3)).to_rfc3339()
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(freeze["label"], "December change freeze");

    // The window overlaps the freeze and is still approved: conflicts are
    // visual, never blocking.
    let approved = decide(&c, &f, id, json!({ "decision": "approve" })).await;
    assert_eq!(approved.status(), 200);
    let approved: Value = approved.json().await.unwrap();
    assert_eq!(approved["status"], "approved");
    assert_eq!(approved["window"]["status"], "approved");
    assert_eq!(approved["window"]["decided_by"], json!(1));

    let moved_after = c
        .patch(format!("{}/api/maintenance/windows/{window_id}", f.base))
        .header("authorization", &f.auth)
        .json(&json!({ "agent_name": f.agent, "starts_at": start.to_rfc3339() }))
        .send()
        .await
        .unwrap();
    assert_eq!(moved_after.status(), 409, "an approved window is fixed");
}

#[tokio::test]
async fn a_revised_window_must_still_be_valid() {
    let app = tu::spawn().await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();
    let start = Utc::now() + Duration::days(3);
    let mut body = ask_body(&f, "Window validation");
    body["kind"] = json!("maintenance");
    body["window"] = json!({
        "starts_at": start.to_rfc3339(),
        "ends_at": (start + Duration::hours(2)).to_rfc3339(),
        "services": ["caddy"],
        "reason": "TLS reload"
    });
    let ask = create_ask(&c, &f, body).await;
    let id = ask["id"].as_i64().unwrap();

    let too_short = decide(
        &c,
        &f,
        id,
        json!({
            "decision": "changes",
            "window": {
                "starts_at": start.to_rfc3339(),
                "ends_at": (start + Duration::minutes(5)).to_rfc3339()
            }
        }),
    )
    .await;
    assert_eq!(too_short.status(), 400, "a 5 minute window is refused");
    // The ask is untouched by a refused decision.
    let still: Value = c
        .get(format!("{}/api/approvals/{id}", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(still["status"], "pending");
    assert_eq!(still["events"].as_array().unwrap().len(), 1);

    let revised = decide(
        &c,
        &f,
        id,
        json!({
            "decision": "changes",
            "note_md": "Move it one hour later.",
            "window": {
                "starts_at": (start + Duration::hours(1)).to_rfc3339(),
                "ends_at": (start + Duration::hours(3)).to_rfc3339()
            }
        }),
    )
    .await;
    let revised_status = revised.status();
    let revised_text = revised.text().await.unwrap();
    assert!(revised_status.is_success(), "revised decision: {revised_status} {revised_text}");
    let revised: Value = serde_json::from_str(&revised_text).unwrap();
    assert_eq!(revised["status"], "changes");
    let revised_start: chrono::DateTime<Utc> =
        serde_json::from_value(revised["window"]["starts_at"].clone()).unwrap();
    assert_eq!(revised_start, start + Duration::hours(1));
}


#[tokio::test]
async fn operator_counts_track_decisions_and_revisions() {
    let app = tu::spawn().await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();
    let ask = create_ask(&c, &f, ask_body(&f, "Badge semantics")).await;
    let id = ask["id"].as_i64().unwrap();

    let counts = |c: reqwest::Client, base: String, auth: String| async move {
        c.get(format!("{base}/api/approvals/counts"))
            .header("authorization", auth)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };
    let before = counts(c.clone(), f.base.clone(), f.auth.clone()).await;
    assert_eq!(before["needs"], 1);
    assert_eq!(before["unread"], 1);

    // Opening it clears unread but it is still waiting for a decision.
    let _: Value = c
        .get(format!("{}/api/approvals/{id}", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let read = counts(c.clone(), f.base.clone(), f.auth.clone()).await;
    assert_eq!(read["needs"], 1);
    assert_eq!(read["unread"], 0);

    // A revision is new operator work: unread comes back.
    c.post(format!("{}/api/approvals/{id}/revisions", f.base))
        .header("authorization", &f.auth)
        .json(&json!({ "agent_name": f.agent, "summary": "More evidence." }))
        .send()
        .await
        .unwrap();
    let revised = counts(c.clone(), f.base.clone(), f.auth.clone()).await;
    assert_eq!(revised["needs"], 1);
    assert_eq!(revised["unread"], 1);

    // A decision removes it from Needs you and leaves nothing unread.
    let decided = decide(&c, &f, id, json!({ "decision": "approve" })).await;
    assert_eq!(decided.status(), 200);
    let done = counts(c.clone(), f.base.clone(), f.auth.clone()).await;
    assert_eq!(done["needs"], 0);
    assert_eq!(done["unread"], 0);
}

#[tokio::test]
async fn agent_unread_marks_new_decisions_and_clears_on_read() {
    let app = tu::spawn().await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();
    let ask = create_ask(&c, &f, ask_body(&f, "Agent cursor")).await;
    let id = ask["id"].as_i64().unwrap();

    let agent_list = |c: reqwest::Client, base: String, auth: String, agent: String| async move {
        c.get(format!("{base}/api/approvals?agent={agent}"))
            .header("authorization", auth)
            .send()
            .await
            .unwrap()
            .json::<Vec<Value>>()
            .await
            .unwrap()
    };

    // No decision yet, so nothing is unread for the agent.
    let mine = agent_list(c.clone(), f.base.clone(), f.auth.clone(), f.agent.clone()).await;
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0]["unread"], false);

    // Request changes is a decision the agent has to see.
    let changes = decide(
        &c,
        &f,
        id,
        json!({ "decision": "changes", "note_md": "Rework it." }),
    )
    .await;
    assert_eq!(changes.status(), 200);
    let mine = agent_list(c.clone(), f.base.clone(), f.auth.clone(), f.agent.clone()).await;
    assert_eq!(mine[0]["unread"], true, "a decision marks the agent view unread");

    // Reading as the agent advances only the agent cursor.
    let detail: Value = c
        .get(format!("{}/api/approvals/{id}?agent={}", f.base, f.agent))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["unread"], true);
    let mine = agent_list(c.clone(), f.base.clone(), f.auth.clone(), f.agent.clone()).await;
    assert_eq!(mine[0]["unread"], false);

    // Its own revision is not a decision to read.
    c.post(format!("{}/api/approvals/{id}/revisions", f.base))
        .header("authorization", &f.auth)
        .json(&json!({ "agent_name": f.agent, "summary": "Reworked." }))
        .send()
        .await
        .unwrap();
    let mine = agent_list(c.clone(), f.base.clone(), f.auth.clone(), f.agent.clone()).await;
    assert_eq!(mine[0]["unread"], false, "a revision does not mark the agent unread");

    // The next decision does.
    let approved = decide(&c, &f, id, json!({ "decision": "approve" })).await;
    assert_eq!(approved.status(), 200);
    let mine = agent_list(c.clone(), f.base.clone(), f.auth.clone(), f.agent.clone()).await;
    assert_eq!(mine[0]["unread"], true);
}

#[tokio::test]
async fn expiry_sweep_itself_expires_and_the_ask_leaves_needs_you() {
    let app = tu::spawn().await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();
    let mut body = ask_body(&f, "Sweep expiry");
    body["expires_at"] = json!((Utc::now() + Duration::hours(1)).to_rfc3339());
    let ask = create_ask(&c, &f, body).await;
    let id = ask["id"].as_i64().unwrap();

    // Move the deadline into the past without a read, so only the pass can
    // expire it.
    sqlx::query("UPDATE approval_asks SET expires_at = ?2 WHERE id = ?1")
        .bind(id)
        .bind((Utc::now() - Duration::minutes(5)).to_rfc3339())
        .execute(&app.db)
        .await
        .unwrap();

    let expired = approvals::expire_due(&app.state).await.unwrap();
    assert_eq!(expired, 1, "the sweep itself expires the ask");
    assert_eq!(approvals::expire_due(&app.state).await.unwrap(), 0, "a second pass is a no-op");

    let needs: Vec<Value> = c
        .get(format!("{}/api/approvals?filter=needs", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(needs.is_empty(), "an expired ask leaves Needs you");

    let all: Vec<Value> = c
        .get(format!("{}/api/approvals?filter=all", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let stored = all.iter().find(|row| row["id"] == json!(id)).expect("expired ask is visible");
    assert_eq!(stored["status"], "expired");
    let last = stored["events"].as_array().unwrap().last().unwrap();
    assert_eq!(last["kind"], "expired");
    assert_eq!(last["actor_kind"], "system");
}

#[tokio::test]
async fn audit_feed_lists_every_transition_with_actor_and_time() {
    let app = tu::spawn().await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();
    let ask = create_ask(&c, &f, ask_body(&f, "Audit me")).await;
    let id = ask["id"].as_i64().unwrap();
    c.post(format!("{}/api/approvals/{id}/revisions", f.base))
        .header("authorization", &f.auth)
        .json(&json!({ "agent_name": f.agent, "summary": "Revision one." }))
        .send()
        .await
        .unwrap();
    decide(&c, &f, id, json!({ "decision": "approve", "note_md": "Ship it." }))
        .await;

    let events: Vec<Value> = c
        .get(format!("{}/api/approvals/{id}/events", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, vec!["created", "revised", "approved"]);
    assert_eq!(events[0]["actor_kind"], "agent");
    assert_eq!(events[0]["actor_name"], f.agent);
    assert_eq!(events[2]["actor_kind"], "human");
    assert_eq!(events[2]["actor_name"], "tester");
    assert!(events.iter().all(|e| e["created_at"].is_string()));
    assert_eq!(events[2]["meta"]["decision"], "approve");

    // Expire also carries the revision it lapsed at.
    let mut expiring = ask_body(&f, "Audit expiry");
    expiring["expires_at"] = json!((Utc::now() + Duration::hours(1)).to_rfc3339());
    let expiring = create_ask(&c, &f, expiring).await;
    let expiring_id = expiring["id"].as_i64().unwrap();
    c.post(format!("{}/api/approvals/{expiring_id}/revisions", f.base))
        .header("authorization", &f.auth)
        .json(&json!({ "agent_name": f.agent, "summary": "Revision two." }))
        .send()
        .await
        .unwrap();
    sqlx::query("UPDATE approval_asks SET expires_at = ?2 WHERE id = ?1")
        .bind(expiring_id)
        .bind((Utc::now() - Duration::minutes(1)).to_rfc3339())
        .execute(&app.db)
        .await
        .unwrap();
    assert_eq!(approvals::expire_due(&app.state).await.unwrap(), 1);
    let events: Vec<Value> = c
        .get(format!("{}/api/approvals/{expiring_id}/events", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let expired = events
        .iter()
        .find(|event| event["kind"] == "expired")
        .expect("expired event");
    assert_eq!(expired["meta"]["revision"], "2", "expire carries the revision");
    assert!(expired["meta"]["expired_at"].is_string());
}

struct FakeGotify {
    url: String,
    requests: Arc<Mutex<Vec<Value>>>,
}

async fn start_fake_gotify() -> FakeGotify {
    start_fake_gotify_with(200, r#"{"id":1}"#).await
}

async fn start_fake_gotify_with(status: u16, response_body: &'static str) -> FakeGotify {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let sink = sink.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let mut content_length = 0usize;
                loop {
                    let Ok(n) = socket.read(&mut chunk).await else {
                        return;
                    };
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf);
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        if content_length == 0 {
                            content_length = head
                                .lines()
                                .find_map(|line| {
                                    line.strip_prefix("content-length:")
                                        .or_else(|| line.strip_prefix("Content-Length:"))
                                })
                                .and_then(|v| v.trim().parse().ok())
                                .unwrap_or(0);
                        }
                        if body.len() >= content_length {
                            let request_line = head.lines().next().unwrap_or_default().to_string();
                            let body: Value = serde_json::from_str(body).unwrap_or(Value::Null);
                            sink.lock().unwrap().push(json!({
                                "line": request_line,
                                "head": head,
                                "body": body,
                            }));
                            break;
                        }
                    }
                }
                let head = format!(
                    "HTTP/1.1 {status} Test\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    response_body.len()
                );
                let _ = socket
                    .write_all(format!("{head}{response_body}").as_bytes())
                    .await;
            });
        }
    });
    FakeGotify {
        url: format!("http://{addr}"),
        requests,
    }
}

fn gotify_count(fake: &FakeGotify) -> usize {
    fake.requests.lock().unwrap().len()
}

#[tokio::test]
async fn urgent_asks_send_one_gotify_message_per_created_transition() {
    let fake = start_fake_gotify().await;
    let url = fake.url.clone();
    let app = tu::spawn_with_config(move |cfg| {
        cfg.approvals.notifications.enabled = true;
        cfg.approvals.notifications.url = url;
        cfg.approvals.notifications.token = "app-token".into();
        cfg.approvals.notifications.app = "bb-test".into();
    })
    .await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();

    let ask = create_ask(&c, &f, ask_body(&f, "Urgent runner rotation")).await;
    let id = ask["id"].as_i64().unwrap();
    assert_eq!(gotify_count(&fake), 1, "an urgent ask pings once");
    {
        let requests = fake.requests.lock().unwrap();
        let request = &requests[0];
        let line = request["line"].as_str().unwrap();
        let head = request["head"].as_str().unwrap();
        assert_eq!(line, "POST /message HTTP/1.1");
        assert!(!line.contains("token"), "the app token must not be in the URL");
        assert!(head.to_ascii_lowercase().contains("x-gotify-key: app-token"));
        assert_eq!(request["body"]["priority"], 8);
        assert!(request["body"]["title"].as_str().unwrap().contains("bb-test"));
    }

    // A revision is not a new ping, and re-reads never ping.
    c.post(format!("{}/api/approvals/{id}/revisions", f.base))
        .header("authorization", &f.auth)
        .json(&json!({ "agent_name": f.agent, "summary": "Reworked." }))
        .send()
        .await
        .unwrap();
    for _ in 0..2 {
        c.get(format!("{}/api/approvals/{id}", f.base))
            .header("authorization", &f.auth)
            .send()
            .await
            .unwrap();
    }
    assert_eq!(gotify_count(&fake), 1, "revisions and re-reads do not ping");

    // A decision is the operator's own action, so it does not ping them again.
    decide(&c, &f, id, json!({ "decision": "approve" })).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(gotify_count(&fake), 1);

    // A second transition (expiry) pings once for an urgent ask.
    let mut expiring = ask_body(&f, "Expiring urgent ask");
    expiring["expires_at"] = json!((Utc::now() + Duration::hours(1)).to_rfc3339());
    let expiring = create_ask(&c, &f, expiring).await;
    assert_eq!(gotify_count(&fake), 2, "creation of the expiring ask pings");
    sqlx::query("UPDATE approval_asks SET expires_at = ?2 WHERE id = ?1")
        .bind(expiring["id"].as_i64().unwrap())
        .bind((Utc::now() - Duration::minutes(1)).to_rfc3339())
        .execute(&app.db)
        .await
        .unwrap();
    assert_eq!(approvals::expire_due(&app.state).await.unwrap(), 1);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(gotify_count(&fake), 3, "expiry is its own transition");
    assert!(
        fake.requests.lock().unwrap().last().unwrap()["body"]["message"]
            .as_str()
            .unwrap()
            .contains("Expired"),
        "the expiry ping explains the lapse"
    );

    // Normal urgency is quiet; an incident-labelled issue pings even at normal.
    let mut quiet = ask_body(&f, "Normal ask");
    quiet["urgency"] = json!("normal");
    create_ask(&c, &f, quiet).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(gotify_count(&fake), 3);

    let incident_issue: Value = c
        .post(format!("{}/api/issues", f.base))
        .header("authorization", &f.auth)
        .json(&json!({
            "title": "incident host",
            "project": "BREAD",
            "labels": ["incident"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut incident = ask_body(&f, "Incident ask");
    incident["issue"] = incident_issue["key"].clone();
    incident["urgency"] = json!("normal");
    create_ask(&c, &f, incident).await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(gotify_count(&fake), 4, "an incident label pings at normal urgency");
}

#[tokio::test]
async fn a_lazy_read_expires_and_notifies_without_swallowing_the_ping() {
    let fake = start_fake_gotify().await;
    let url = fake.url.clone();
    let app = tu::spawn_with_config(move |cfg| {
        cfg.approvals.notifications.enabled = true;
        cfg.approvals.notifications.url = url;
        cfg.approvals.notifications.token = "app-token".into();
    })
    .await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();

    let mut body = ask_body(&f, "Lazy expiry high ask");
    body["expires_at"] = json!((Utc::now() + Duration::hours(1)).to_rfc3339());
    let ask = create_ask(&c, &f, body).await;
    let id = ask["id"].as_i64().unwrap();
    assert_eq!(gotify_count(&fake), 1, "creation pings");

    sqlx::query("UPDATE approval_asks SET expires_at = ?2 WHERE id = ?1")
        .bind(id)
        .bind((Utc::now() - Duration::minutes(1)).to_rfc3339())
        .execute(&app.db)
        .await
        .unwrap();

    // A page read is the only thing that runs the expiry pass here.
    let detail: Value = c
        .get(format!("{}/api/approvals/{id}", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["status"], "expired");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(gotify_count(&fake), 2, "the lazy read sends the expiry ping");
    assert!(
        fake.requests.lock().unwrap().last().unwrap()["body"]["message"]
            .as_str()
            .unwrap()
            .contains("Expired"),
        "the second message is the expiry"
    );

    // A second read, and the sweep afterwards, cannot send it again.
    c.get(format!("{}/api/approvals/{id}", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap();
    assert_eq!(approvals::expire_due(&app.state).await.unwrap(), 0);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(gotify_count(&fake), 2, "created and expired, once each");
}

#[tokio::test]
async fn a_failed_gotify_send_never_logs_the_app_token() {
    let _log_guard = tu::LOG_LOCK.lock().await;
    let fake = start_fake_gotify_with(500, r#"{"error":"nope"}"#).await;
    let url = fake.url.clone();
    let app = tu::spawn_with_config(move |cfg| {
        cfg.approvals.notifications.enabled = true;
        cfg.approvals.notifications.url = url;
        cfg.approvals.notifications.token = "super-secret-app-token".into();
    })
    .await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();
    tu::clear_logs();

    create_ask(&c, &f, ask_body(&f, "Failing Gotify send")).await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let logs = tu::log_text();
    assert!(
        logs.contains("approval Gotify send refused"),
        "the refusal is logged: {logs}"
    );
    assert!(
        !logs.contains("super-secret-app-token"),
        "the token must never appear in a log line: {logs}"
    );
    assert!(
        !logs.to_ascii_lowercase().contains("token="),
        "no query token may appear in a log line: {logs}"
    );
    let requests = fake.requests.lock().unwrap();
    let head = requests[0]["head"].as_str().unwrap().to_ascii_lowercase();
    assert!(head.contains("x-gotify-key: super-secret-app-token"));
    assert!(!requests[0]["line"].as_str().unwrap().contains("token"));
}

#[tokio::test]
async fn a_transport_failure_logs_only_a_classification() {
    let _log_guard = tu::LOG_LOCK.lock().await;
    let app = tu::spawn_with_config(|cfg| {
        cfg.approvals.notifications.enabled = true;
        // Nothing listens here, so the request fails at connect.
        cfg.approvals.notifications.url = "http://127.0.0.1:1".into();
        cfg.approvals.notifications.token = "transport-secret-token".into();
    })
    .await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();
    tu::clear_logs();

    create_ask(&c, &f, ask_body(&f, "Transport failure")).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let logs = tu::log_text();
    assert!(
        logs.contains("approval Gotify send failed"),
        "the transport failure is logged: {logs}"
    );
    assert!(
        logs.contains("failure=\"connect\""),
        "the log carries only a classification: {logs}"
    );
    assert!(!logs.contains("transport-secret-token"));
    assert!(
        !logs.contains("http://127.0.0.1:1"),
        "the URL is not logged either: {logs}"
    );
}

#[tokio::test]
async fn withdrawing_a_lapsed_high_ask_still_sends_the_expiry_ping() {
    let fake = start_fake_gotify().await;
    let url = fake.url.clone();
    let app = tu::spawn_with_config(move |cfg| {
        cfg.approvals.notifications.enabled = true;
        cfg.approvals.notifications.url = url;
        cfg.approvals.notifications.token = "app-token".into();
    })
    .await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();

    let mut body = ask_body(&f, "Withdraw a lapsed high ask");
    body["expires_at"] = json!((Utc::now() + Duration::hours(1)).to_rfc3339());
    let ask = create_ask(&c, &f, body).await;
    let id = ask["id"].as_i64().unwrap();
    assert_eq!(gotify_count(&fake), 1, "creation pings");

    sqlx::query("UPDATE approval_asks SET expires_at = ?2 WHERE id = ?1")
        .bind(id)
        .bind((Utc::now() - Duration::minutes(1)).to_rfc3339())
        .execute(&app.db)
        .await
        .unwrap();

    // Withdraw is a lazy expiry path. The expired ask cannot be withdrawn, but
    // the expiry ping must not be swallowed by the refusal.
    let withdraw = c
        .post(format!("{}/api/approvals/{id}/withdraw", f.base))
        .header("authorization", &f.auth)
        .json(&json!({ "agent_name": f.agent }))
        .send()
        .await
        .unwrap();
    assert_eq!(withdraw.status(), 409, "an expired ask cannot be withdrawn");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(gotify_count(&fake), 2, "created plus expired, exactly once each");
    assert!(
        fake.requests.lock().unwrap().last().unwrap()["body"]["message"]
            .as_str()
            .unwrap()
            .contains("Expired")
    );

    // A second attempted withdraw and the sweep add nothing.
    c.post(format!("{}/api/approvals/{id}/withdraw", f.base))
        .header("authorization", &f.auth)
        .json(&json!({ "agent_name": f.agent }))
        .send()
        .await
        .unwrap();
    assert_eq!(approvals::expire_due(&app.state).await.unwrap(), 0);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(gotify_count(&fake), 2);
}

#[tokio::test]
async fn approval_notification_opt_out_is_respected() {
    let fake = start_fake_gotify().await;
    let url = fake.url.clone();
    let app = tu::spawn_with_config(move |cfg| {
        cfg.approvals.notifications.enabled = true;
        cfg.approvals.notifications.url = url;
        cfg.approvals.notifications.token = "app-token".into();
    })
    .await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();

    let opted_out: Value = c
        .put(format!("{}/api/notifications/prefs", f.base))
        .header("authorization", &f.auth)
        .json(&json!({
            "assigned": true,
            "mention": true,
            "cycle": true,
            "digest": false,
            "approvals": false
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(opted_out["approvals"], false);

    create_ask(&c, &f, ask_body(&f, "Opted out high ask")).await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(gotify_count(&fake), 0, "the opt-out silences Gotify");

    // The in-app badge still counts the ask that needs a decision.
    let counts: Value = c
        .get(format!("{}/api/approvals/counts", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(counts["needs"], 1);

    c.put(format!("{}/api/notifications/prefs", f.base))
        .header("authorization", &f.auth)
        .json(&json!({
            "assigned": true, "mention": true, "cycle": true,
            "digest": false, "approvals": true
        }))
        .send()
        .await
        .unwrap();
    create_ask(&c, &f, ask_body(&f, "Opted in high ask")).await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(gotify_count(&fake), 1, "re-enabling resumes pings");
}

#[tokio::test]
async fn list_filters_and_agent_queue() {
    let app = tu::spawn().await;
    let f = fixture(&app).await;
    let c = reqwest::Client::new();
    let first = create_ask(&c, &f, ask_body(&f, "Approval ask")).await;
    let mut info = ask_body(&f, "Info ask");
    info["kind"] = json!("info");
    let info = create_ask(&c, &f, info).await;
    let mut maint = ask_body(&f, "Maintenance ask");
    maint["kind"] = json!("maintenance");
    maint["window"] = json!({
        "starts_at": (Utc::now() + Duration::days(1)).to_rfc3339(),
        "ends_at": (Utc::now() + Duration::days(1) + Duration::hours(1)).to_rfc3339(),
        "services": ["caddy"],
        "reason": "reload"
    });
    let maint = create_ask(&c, &f, maint).await;

    for (kind, expected) in [("approval", 1), ("info", 1), ("maintenance", 1)] {
        let rows: Vec<Value> = c
            .get(format!("{}/api/approvals?kind={kind}", f.base))
            .header("authorization", &f.auth)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(rows.len(), expected, "kind={kind}");
    }

    let search: Vec<Value> = c
        .get(format!("{}/api/approvals?q=maintenance", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(search.len(), 1);
    assert_eq!(search[0]["id"], maint["id"]);

    // The agent sees its own queue, newest first, with the decision state.
    let mine: Vec<Value> = c
        .get(format!("{}/api/approvals?agent={}", f.base, f.agent))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(mine.len(), 3);
    assert!(mine.iter().all(|a| a["agent_name"] == f.agent));
    assert_eq!(mine[0]["id"], maint["id"]);

    // A known agent name owned by another user is refused, and a read never
    // registers an unknown identity as a side effect.
    let auditor = install_second_user(&app, "auditor4", "auditor4-token-abcdef").await;
    let spoof = c
        .get(format!("{}/api/approvals?agent={}", f.base, f.agent))
        .header("authorization", &auditor)
        .send()
        .await
        .unwrap();
    assert_eq!(spoof.status(), 403);
    let unknown: Vec<Value> = c
        .get(format!("{}/api/approvals?agent=somebody%20else", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(unknown.is_empty());

    // The filter chips match the mockup.
    let changes_count: Value = c
        .get(format!("{}/api/approvals/counts", f.base))
        .header("authorization", &f.auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(changes_count["needs"], 3);
    assert_eq!(changes_count["info"], 1);
    assert_eq!(changes_count["maintenance"], 1);
    assert_eq!(changes_count["approval"], 1);
    assert_eq!(first["kind"], "approval");
    assert_eq!(info["kind"], "info");
}
