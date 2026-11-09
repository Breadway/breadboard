//! Review requests: notify the reviewer, show on both sides, gate `done`, and
//! escalate a request that sits untouched.

mod common;
use breadboard_server::auth::sha256_hex;
use chrono::Utc;
use common as tu;
use serde_json::{json, Value};

async fn project_and_issue(base: &str, auth: &str) -> String {
    let c = reqwest::Client::new();
    c.post(format!("{base}/api/projects"))
        .header("authorization", auth)
        .json(&json!({ "key": "BREAD", "name": "bread" }))
        .send()
        .await
        .unwrap();
    let created: Value = c
        .post(format!("{base}/api/issues"))
        .header("authorization", auth)
        .json(&json!({ "title": "review me", "project": "BREAD" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    created["key"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn review_request_notifies_and_gates_done_until_approved() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let key = project_and_issue(&base, &auth).await;

    // A second human, with their own bearer token.
    let reviewer_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (forgejo_id, login, display_name) VALUES (2, 'reviewer', 'Reviewer') RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();
    let reviewer_token = "reviewer-token-abcdef".to_string();
    sqlx::query(
        "INSERT INTO api_tokens (user_id, name, token_hash, created_at) VALUES (?1, 'test', ?2, ?3)",
    )
    .bind(reviewer_id)
    .bind(sha256_hex(&reviewer_token))
    .bind(Utc::now().to_rfc3339())
    .execute(&app.db)
    .await
    .unwrap();
    let reviewer_auth = format!("Bearer {reviewer_token}");

    // File the request. The reviewer is notified through the normal bell.
    let created: Value = c
        .post(format!("{base}/api/issues/{key}/reviews"))
        .header("authorization", &auth)
        .json(&json!({
            "reviewer": "reviewer",
            "pr_url": "https://git.breadway.dev/Breadway/breadboard/pulls/99",
            "note": "please look at the gate"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["state"], "requested");
    assert_eq!(created["reviewer_kind"], "human");
    assert_eq!(created["reviewer_id"], json!(reviewer_id));
    let review_id = created["id"].as_i64().unwrap();

    let notified: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notifications WHERE user_id = ?1 AND kind = 'review'",
    )
    .bind(reviewer_id)
    .fetch_one(&app.db)
    .await
    .unwrap();
    assert_eq!(notified, 1, "reviewer should have a review notification");

    // Shows on the issue and in the reviewer's queue.
    let for_issue: Vec<Value> = c
        .get(format!("{base}/api/issues/{key}/reviews"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(for_issue.len(), 1);
    assert_eq!(for_issue[0]["issue_key"], key);

    let queue: Vec<Value> = c
        .get(format!(
            "{base}/api/reviews?reviewer=reviewer&outstanding=true"
        ))
        .header("authorization", &reviewer_auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0]["id"], json!(review_id));

    // An outstanding request blocks done, exactly like CI.
    let blocked = c
        .patch(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .json(&json!({ "status": "done" }))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 409);
    let still: Value = c
        .get(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(still["status"], "backlog");

    // Accepting is recorded but does not resolve the request.
    let accepted: Value = c
        .patch(format!("{base}/api/reviews/{review_id}"))
        .header("authorization", &reviewer_auth)
        .json(&json!({ "state": "accepted" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(accepted["state"], "accepted");
    assert!(accepted["responded_at"].is_string());
    let blocked = c
        .patch(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .json(&json!({ "status": "done" }))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 409);

    // Changes requested is an outcome, not an approval.
    let changes: Value = c
        .patch(format!("{base}/api/reviews/{review_id}"))
        .header("authorization", &reviewer_auth)
        .json(&json!({ "state": "changes_requested", "note": "the gate needs a test" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(changes["state"], "changes_requested");
    let requester_notified: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notifications WHERE user_id = (SELECT id FROM users WHERE login = 'tester') AND kind = 'review'",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();
    assert!(
        requester_notified >= 1,
        "the requester should hear about the outcome"
    );

    // Approval resolves it, and only then does done succeed.
    let approved: Value = c
        .patch(format!("{base}/api/reviews/{review_id}"))
        .header("authorization", &reviewer_auth)
        .json(&json!({ "state": "approved" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(approved["state"], "approved");
    let done = c
        .patch(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .json(&json!({ "status": "done" }))
        .send()
        .await
        .unwrap();
    assert_eq!(done.status(), 200);

    // An approved request is history: a fresh round can be filed, but a
    // duplicate active request is a conflict.
    c.post(format!("{base}/api/issues/{key}/reviews"))
        .header("authorization", &auth)
        .json(&json!({ "reviewer": "reviewer", "pr_number": 100 }))
        .send()
        .await
        .unwrap();
    let dup = c
        .post(format!("{base}/api/issues/{key}/reviews"))
        .header("authorization", &auth)
        .json(&json!({ "reviewer": "reviewer", "pr_number": 101 }))
        .send()
        .await
        .unwrap();
    assert_eq!(dup.status(), 409);
}

#[tokio::test]
async fn agent_reviewers_use_the_same_records_and_chat_push_path() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let key = project_and_issue(&base, &auth).await;

    // A user who is neither the requester nor the agent's owner: only the
    // reviewer identity can authorize this update.
    let other_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (forgejo_id, login, display_name) VALUES (3, 'other', 'Other') RETURNING id",
    )
    .fetch_one(&app.db)
    .await
    .unwrap();
    let other_token = "other-token-abcdef".to_string();
    sqlx::query(
        "INSERT INTO api_tokens (user_id, name, token_hash, created_at) VALUES (?1, 'test', ?2, ?3)",
    )
    .bind(other_id)
    .bind(sha256_hex(&other_token))
    .bind(Utc::now().to_rfc3339())
    .execute(&app.db)
    .await
    .unwrap();
    let other_auth = format!("Bearer {other_token}");

    let registered: Value = c
        .post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "Zesty Rye" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(registered["agent_name"], "zesty rye");

    let created: Value = c
        .post(format!("{base}/api/issues/{key}/reviews"))
        .header("authorization", &auth)
        .json(&json!({
            "reviewer": "Zesty Rye",
            "reviewer_kind": "agent",
            "pr_number": 42,
            "pr_repo": "Breadway/breadboard"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["reviewer_kind"], "agent");

    // The agent is woken through the existing chat push inbox.
    let pushes: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chat_pushes WHERE member = 'agent:zesty rye'")
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert_eq!(pushes, 1, "agent reviewer should get a DM push");

    let queue: Vec<Value> = c
        .get(format!(
            "{base}/api/reviews?reviewer=Zesty%20Rye&outstanding=true"
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(queue.len(), 1);

    let id = created["id"].as_i64().unwrap();
    // The signed-in user owns the agent, but a named agent identity is still
    // checked so a different agent cannot approve it.
    let wrong = c
        .patch(format!("{base}/api/reviews/{id}"))
        .header("authorization", &other_auth)
        .json(&json!({ "state": "approved", "agent_name": "Snazzy Bagel" }))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 403);

    let ok: Value = c
        .patch(format!("{base}/api/reviews/{id}"))
        .header("authorization", &other_auth)
        .json(&json!({ "state": "approved", "agent_name": "zesty-rye" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ok["state"], "approved");

    // With the agent's request approved, done is allowed again.
    let done = c
        .patch(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .json(&json!({ "status": "done" }))
        .send()
        .await
        .unwrap();
    assert_eq!(done.status(), 200);
}

#[tokio::test]
async fn untouched_requests_escalate_once() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let key = project_and_issue(&base, &auth).await;

    let created: Value = c
        .post(format!("{base}/api/issues/{key}/reviews"))
        .header("authorization", &auth)
        .json(&json!({ "reviewer": "tester", "pr_number": 7 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_i64().unwrap();

    // Age the request past the one-hour threshold.
    sqlx::query("UPDATE review_requests SET requested_at = ?2, updated_at = ?2 WHERE id = ?1")
        .bind(id)
        .bind((Utc::now() - chrono::Duration::hours(2)).to_rfc3339())
        .execute(&app.db)
        .await
        .unwrap();

    let escalated = breadboard_server::reviews::escalate_stale(&app.db, Utc::now())
        .await
        .unwrap();
    assert_eq!(escalated.len(), 1);
    assert_eq!(escalated[0].id, id);
    assert!(escalated[0].escalated_at.is_some());

    // Escalation is a one-shot per untouched window.
    let again = breadboard_server::reviews::escalate_stale(&app.db, Utc::now())
        .await
        .unwrap();
    assert!(again.is_empty());
}
