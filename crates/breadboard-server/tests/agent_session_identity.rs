//! BB-188: one session holds exactly one agent name.
//!
//! BB-170 stopped one name belonging to two live sessions. This is the mirror:
//! the same session must not accumulate a name per restart. A nameless
//! registration reuses the name the session already holds, and an explicit
//! different name is refused with a 409 that points at rename.

mod common;
use common as tu;
use serde_json::{json, Value};

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

async fn register_nameless(
    c: &reqwest::Client,
    base: &str,
    auth: &str,
    session: &str,
) -> reqwest::Response {
    c.post(format!("{base}/api/agents/register"))
        .header("authorization", auth)
        .json(&json!({ "session_id": session }))
        .send()
        .await
        .unwrap()
}

async fn roster_rows_for_session(
    c: &reqwest::Client,
    base: &str,
    auth: &str,
    session: &str,
) -> Vec<Value> {
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
        .filter(|row| row["session_id"] == session)
        .cloned()
        .collect()
}

/// The every-restart path: no name in the body, the session already registered
/// once, so the server returns the existing name instead of rolling a new one.
#[tokio::test]
async fn nameless_reregistration_reuses_the_existing_name() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let first = register(&c, &base, &auth, "dough-sentinel", "ses_reuse").await;
    assert!(first.status().is_success(), "{}", first.status());

    let again = register_nameless(&c, &base, &auth, "ses_reuse").await;
    assert_eq!(again.status(), 200);
    let body: Value = again.json().await.unwrap();
    assert_eq!(body["agent_name"], "dough-sentinel");
    assert_eq!(body["session_id"], "ses_reuse");

    // The restart did not leave a second registered name behind.
    let rows = roster_rows_for_session(&c, &base, &auth, "ses_reuse").await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["agent_name"], "dough-sentinel");
    assert_eq!(rows[0]["live"], true);
}

/// An explicit name that differs from the session's existing name is a
/// conflict; rename is the operation that moves the identity.
#[tokio::test]
async fn explicit_second_name_from_the_same_session_is_refused_with_rename_hint() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let first = register(&c, &base, &auth, "first-crumb", "ses_two_names").await;
    assert!(first.status().is_success(), "{}", first.status());

    let refused = register(&c, &base, &auth, "second-crumb", "ses_two_names").await;
    assert_eq!(refused.status(), 409);
    let body: Value = refused.json().await.unwrap();
    let error = body["error"].as_str().unwrap();
    assert!(error.contains("already registered as 'first-crumb'"), "{error}");
    assert!(error.contains("rename"), "{error}");
    assert_eq!(body["agent_name"], "second-crumb");
    assert_eq!(body["existing_name"], "first-crumb");
    assert_eq!(body["session_id"], "ses_two_names");
    assert_eq!(body["rename_required"], true);

    // The refusal did not register the second name or touch the first.
    let rows = roster_rows_for_session(&c, &base, &auth, "ses_two_names").await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["agent_name"], "first-crumb");
    let roster: Value = c
        .get(format!("{base}/api/agents/roster"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(roster
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["agent_name"] != "second-crumb"));
}

/// Rename still moves the session row, and nameless registration follows the
/// new name rather than falling back to the old one.
#[tokio::test]
async fn rename_moves_the_identity_and_nameless_registration_follows() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let first = register(&c, &base, &auth, "before-rename", "ses_rename_flow").await;
    assert!(first.status().is_success(), "{}", first.status());

    let renamed = c
        .post(format!("{base}/api/agents/before-rename/rename"))
        .header("authorization", &auth)
        .json(&json!({ "new_name": "after-rename" }))
        .send()
        .await
        .unwrap();
    assert_eq!(renamed.status(), 200);
    let renamed: Value = renamed.json().await.unwrap();
    assert_eq!(renamed["agent_name"], "after-rename");
    assert_eq!(renamed["session_id"], "ses_rename_flow");

    let again = register_nameless(&c, &base, &auth, "ses_rename_flow").await;
    assert_eq!(again.status(), 200);
    let body: Value = again.json().await.unwrap();
    assert_eq!(body["agent_name"], "after-rename");
    assert_eq!(body["session_id"], "ses_rename_flow");

    let rows = roster_rows_for_session(&c, &base, &auth, "ses_rename_flow").await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["agent_name"], "after-rename");

    // The old name is free after the rename.
    let old = register(&c, &base, &auth, "before-rename", "ses_other_owner").await;
    assert_eq!(old.status(), 200);
    let old: Value = old.json().await.unwrap();
    assert_eq!(old["agent_name"], "before-rename");
}

/// Claiming is another explicit write path: the same rule protects it, so a
/// session cannot acquire a second name by claiming under one.
#[tokio::test]
async fn claiming_under_a_second_name_from_the_same_session_is_refused() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let project = c
        .post(format!("{base}/api/projects"))
        .header("authorization", &auth)
        .json(&json!({ "key": "AGENT", "name": "Agent work" }))
        .send()
        .await
        .unwrap();
    assert!(project.status().is_success(), "{}", project.status());
    let issue: Value = c
        .post(format!("{base}/api/issues"))
        .header("authorization", &auth)
        .json(&json!({ "title": "second name claim", "project": "AGENT" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = issue["key"].as_str().unwrap();

    let first = register(&c, &base, &auth, "claim-crumb", "ses_claim_name").await;
    assert!(first.status().is_success(), "{}", first.status());

    let refused = c
        .post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "other-claim-crumb",
            "session_id": "ses_claim_name"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 409);
    let body: Value = refused.json().await.unwrap();
    assert!(
        body["error"].as_str().unwrap().contains("rename"),
        "{body}"
    );

    // The card was not claimed under the second name.
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
}
