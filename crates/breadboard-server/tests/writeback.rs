//! Forgejo writeback timing: create and patch answer from the local row and
//! push to the remote in the background, with a durable retry for failures.

mod common;

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

use axum::{extract::State, http::StatusCode, Router};
use breadboard_server::{store, sync};
use serde_json::{json, Value};

#[derive(Clone)]
struct FakeForgejo {
    deny: Arc<AtomicBool>,
    attempts: Arc<AtomicUsize>,
    creates: Arc<AtomicUsize>,
    issues: Arc<std::sync::Mutex<Vec<Value>>>,
    /// Held at zero by the create test so the writeback cannot finish before
    /// the assertion; released once the API response has been checked.
    gate: Arc<tokio::sync::Semaphore>,
}

async fn create_issue(
    State(fake): State<FakeForgejo>,
    axum::Json(payload): axum::Json<Value>,
) -> (StatusCode, axum::Json<Value>) {
    // The gate stands in for a slow remote: a synchronous writeback would
    // block the API response here.
    let _permit = fake.gate.acquire().await.unwrap();
    fake.attempts.fetch_add(1, Ordering::SeqCst);
    if fake.deny.load(Ordering::SeqCst) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({ "message": "denied" })),
        );
    }
    let n = fake.creates.fetch_add(1, Ordering::SeqCst) + 1;
    let issue = json!({
        "number": 100 + n,
        "title": payload.get("title").cloned().unwrap_or(json!("Task")),
        "body": payload.get("body").cloned().unwrap_or(json!("")),
        "state": "open",
        "labels": [], "assignees": [], "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z"
    });
    fake.issues.lock().unwrap().push(issue.clone());
    (StatusCode::CREATED, axum::Json(issue))
}

async fn list_issues(State(fake): State<FakeForgejo>) -> axum::Json<Vec<Value>> {
    axum::Json(fake.issues.lock().unwrap().clone())
}

async fn patch_issue() -> axum::Json<Value> {
    axum::Json(json!({}))
}

async fn harness() -> (common::TestApp, FakeForgejo) {
    let fake = FakeForgejo {
        deny: Arc::new(AtomicBool::new(false)),
        attempts: Arc::new(AtomicUsize::new(0)),
        creates: Arc::new(AtomicUsize::new(0)),
        issues: Arc::new(std::sync::Mutex::new(Vec::new())),
        gate: Arc::new(tokio::sync::Semaphore::new(0)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = Router::new()
        .route(
            "/api/v1/repos/Breadway/test/issues",
            axum::routing::get(list_issues).post(create_issue),
        )
        .route(
            "/api/v1/repos/Breadway/test/issues/{number}",
            axum::routing::patch(patch_issue),
        )
        .with_state(fake.clone());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let app =
        common::spawn_with_forgejo(Some((format!("http://{addr}"), "fake-token".into()))).await;
    store::upsert_project_for_repo(&app.db, "TST", "test", "Breadway/test")
        .await
        .unwrap();
    store::upsert_repo(&app.db, "Breadway/test", true)
        .await
        .unwrap();
    (app, fake)
}

async fn wait_linked(app: &common::TestApp, key: &str) -> bool {
    for _ in 0..100 {
        if store::get_issue_by_key(&app.db, key)
            .await
            .unwrap()
            .forgejo_ref
            .is_some()
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn create_answers_before_the_forgejo_writeback_runs() {
    let (app, fake) = harness().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let response = c
        .post(format!("{base}/api/issues"))
        .header("authorization", &auth)
        .json(&json!({ "title": "slow remote", "project": "TST" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    assert_eq!(
        response
            .headers()
            .get("x-breadboard-writeback")
            .and_then(|v| v.to_str().ok()),
        Some("pending")
    );
    let body: Value = response.json().await.unwrap();
    let key = body["key"].as_str().unwrap().to_string();
    assert!(
        body["forgejo_ref"].is_null(),
        "the caller sees the local card, with the link still pending"
    );
    assert_eq!(
        fake.creates.load(Ordering::SeqCst),
        0,
        "the response did not wait for the remote"
    );

    // Let the blocked remote call through; the background task links the card.
    fake.gate.add_permits(1);
    assert!(wait_linked(&app, &key).await, "background writeback links the card");
    assert_eq!(fake.creates.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failed_writeback_is_retried_on_the_reconcile_pass() {
    let (app, fake) = harness().await;
    fake.deny.store(true, Ordering::SeqCst);
    // This test does not check ordering, so let the fake through freely.
    fake.gate.add_permits(100);
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let created: Value = c
        .post(format!("{base}/api/issues"))
        .header("authorization", &auth)
        .json(&json!({ "title": "remote down", "project": "TST" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = created["key"].as_str().unwrap().to_string();
    assert!(created["forgejo_ref"].is_null());

    // The background attempt fails; the card stays unlinked.
    for _ in 0..40 {
        if fake.attempts.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(store::get_issue_by_key(&app.db, &key)
        .await
        .unwrap()
        .forgejo_ref
        .is_none());

    // The remote recovers; the reconcile sweep is the durable retry.
    fake.deny.store(false, Ordering::SeqCst);
    sync::retry_pending_writebacks(&app.state).await;
    assert!(
        store::get_issue_by_key(&app.db, &key)
            .await
            .unwrap()
            .forgejo_ref
            .is_some(),
        "retry links the card"
    );
    assert_eq!(fake.creates.load(Ordering::SeqCst), 1);

    // A second pass is a no-op: the card is linked.
    sync::retry_pending_writebacks(&app.state).await;
    assert_eq!(fake.creates.load(Ordering::SeqCst), 1);
}
