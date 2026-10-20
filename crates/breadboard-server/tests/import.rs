mod common;

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

use axum::{extract::State, http::StatusCode, Router};
use breadboard_server::{
    import::{self, ImportRequest},
    store,
};
use serde_json::json;

#[derive(Clone)]
struct FakeForgejo {
    deny: Arc<AtomicBool>,
    creates: Arc<AtomicUsize>,
    issues: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    states: Arc<std::sync::Mutex<Vec<String>>>,
}

async fn create_issue(
    State(fake): State<FakeForgejo>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> (StatusCode, axum::Json<serde_json::Value>) {
    fake.creates.fetch_add(1, Ordering::SeqCst);
    if fake.deny.load(Ordering::SeqCst) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({"message":"denied"})),
        );
    }
    let number = 40 + fake.creates.load(Ordering::SeqCst) as i64;
    let issue = json!({
        "number": number,
        "title": payload.get("title").cloned().unwrap_or(json!("Task")),
        "body": payload.get("body").cloned().unwrap_or(json!("")), "state": "open",
        "labels": [], "assignees": [], "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z"
    });
    fake.issues.lock().unwrap().push(issue.clone());
    (StatusCode::CREATED, axum::Json(issue))
}

async fn patch_any_issue(
    State(fake): State<FakeForgejo>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    if let Some(state) = body.get("state").and_then(serde_json::Value::as_str) {
        fake.states.lock().unwrap().push(state.to_string());
    }
    axum::Json(json!({}))
}

async fn list_issues(State(fake): State<FakeForgejo>) -> axum::Json<Vec<serde_json::Value>> {
    axum::Json(fake.issues.lock().unwrap().clone())
}

async fn harness(deny: bool) -> (common::TestApp, FakeForgejo) {
    let fake = FakeForgejo {
        deny: Arc::new(AtomicBool::new(deny)),
        creates: Arc::new(AtomicUsize::new(0)),
        issues: Arc::new(std::sync::Mutex::new(Vec::new())),
        states: Arc::new(std::sync::Mutex::new(Vec::new())),
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
            axum::routing::patch(patch_any_issue),
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

fn request() -> ImportRequest {
    ImportRequest {
        project: "TST".into(),
        markdown: "# Epic\n- [ ] Task".into(),
        dry_run: false,
        repair: false,
    }
}

#[tokio::test]
async fn import_mirrors_and_repeated_import_does_not_duplicate() {
    let (app, fake) = harness(false).await;
    let first = import::run(&app.state, &request()).await.unwrap();
    assert_eq!(first.issues_created, ["TST-1"]);
    assert!(first.pending_writebacks.is_empty());
    assert_eq!(
        store::get_issue_by_key(&app.db, "TST-1")
            .await
            .unwrap()
            .forgejo_ref
            .unwrap()
            .number,
        41
    );
    let second = import::run(&app.state, &request()).await.unwrap();
    assert!(second.skipped.iter().any(|s| s == "issue: Task"));
    assert_eq!(fake.creates.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn concurrent_imports_create_and_mirror_one_card() {
    let (app, fake) = harness(false).await;
    let req_a = request();
    let req_b = request();
    let (a, b) = tokio::join!(
        import::run(&app.state, &req_a),
        import::run(&app.state, &req_b)
    );
    assert!(a.unwrap().pending_writebacks.is_empty());
    assert!(b.unwrap().pending_writebacks.is_empty());
    assert!(store::get_issue_by_key(&app.db, "TST-1")
        .await
        .unwrap()
        .forgejo_ref
        .is_some());
    assert_eq!(fake.creates.load(Ordering::SeqCst), 1);
    assert!(store::get_issue_by_key(&app.db, "TST-2").await.is_err());
}

#[tokio::test]
async fn denied_write_is_reported_and_retry_links_existing_card() {
    let (app, fake) = harness(true).await;
    let first = import::run(&app.state, &request()).await.unwrap();
    assert_eq!(first.pending_writebacks.len(), 1);
    assert_eq!(first.pending_writebacks[0].key, "TST-1");
    assert!(first.pending_writebacks[0].error.contains("403"));
    fake.deny.store(false, Ordering::SeqCst);
    let mut repair = request();
    repair.repair = true;
    let retry = import::run(&app.state, &repair).await.unwrap();
    assert!(retry.skipped.iter().any(|s| s == "issue: Task"));
    assert!(retry.pending_writebacks.is_empty());
    assert_eq!(
        store::get_issue_by_key(&app.db, "TST-1")
            .await
            .unwrap()
            .forgejo_ref
            .unwrap()
            .number,
        42
    );
    assert_eq!(fake.creates.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn existing_unlinked_card_is_repaired_without_changing_its_key() {
    let (app, fake) = harness(false).await;
    let local = store::create_issue(
        &app.db,
        &breadboard_server::dto::NewIssue {
            project: Some("TST".into()),
            title: "Task".into(),
            body: "preserve body".into(),
            priority: Some(breadboard_core::Priority::High),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    let mut repair = request();
    repair.repair = true;
    let result = import::run(&app.state, &repair).await.unwrap();
    assert!(result.pending_writebacks.is_empty());
    let repaired = store::get_issue_by_key(&app.db, "TST-1").await.unwrap();
    assert_eq!(repaired.id, local.id);
    assert_eq!(repaired.body, "preserve body");
    assert_eq!(repaired.priority, breadboard_core::Priority::High);
    assert_eq!(repaired.forgejo_ref.unwrap().number, 41);
    import::run(&app.state, &repair).await.unwrap();
    assert_eq!(fake.creates.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn repair_ignores_pull_request_cards_with_the_same_title() {
    let (app, fake) = harness(false).await;
    let local = store::create_issue(
        &app.db,
        &breadboard_server::dto::NewIssue {
            project: Some("TST".into()),
            title: "Task".into(),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    let pr = store::create_issue(
        &app.db,
        &breadboard_server::dto::NewIssue {
            project: Some("TST".into()),
            title: "Task".into(),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE issues SET type = 'pr' WHERE id = ?1")
        .bind(pr.id)
        .execute(&app.db)
        .await
        .unwrap();

    let mut repair = request();
    repair.repair = true;
    let result = import::run(&app.state, &repair).await.unwrap();
    assert!(result.pending_writebacks.is_empty());
    assert_eq!(result.remote_would_create, Vec::<String>::new());
    assert_eq!(
        store::get_issue_by_key(&app.db, &local.key)
            .await
            .unwrap()
            .forgejo_ref
            .unwrap()
            .number,
        41
    );
    assert!(store::get_issue_by_key(&app.db, &pr.key)
        .await
        .unwrap()
        .forgejo_ref
        .is_none());
    assert_eq!(fake.creates.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn repositoryless_import_stays_local() {
    let app = common::spawn().await;
    let result = import::run(
        &app.state,
        &ImportRequest {
            project: "ROAD".into(),
            markdown: "# Epic\n- [ ] Task".into(),
            dry_run: false,
            repair: false,
        },
    )
    .await
    .unwrap();
    assert!(result.pending_writebacks.is_empty());
    assert!(store::get_issue_by_key(&app.db, "ROAD-1")
        .await
        .unwrap()
        .forgejo_ref
        .is_none());
}

#[tokio::test]
async fn import_rejects_repeated_titles_instead_of_collapsing_items() {
    let (app, _) = harness(false).await;
    let mut req = request();
    req.markdown = "# Epic\n- [ ] Same\n- [x] Same".into();
    assert!(import::run(&app.state, &req)
        .await
        .unwrap_err()
        .to_string()
        .contains("ambiguous"));
}

#[tokio::test]
async fn repair_dry_run_lists_existing_unlinked_keys_without_writeback() {
    let (app, fake) = harness(false).await;
    store::create_issue(
        &app.db,
        &breadboard_server::dto::NewIssue {
            project: Some("TST".into()),
            title: "Task".into(),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    let mut req = request();
    req.repair = true;
    req.dry_run = true;
    let result = import::run(&app.state, &req).await.unwrap();
    assert_eq!(result.remote_would_create, ["TST-1"]);
    assert_eq!(fake.creates.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn repair_retry_finds_remote_marker_after_webhook_echo_and_link_failure() {
    let (app, fake) = harness(false).await;
    let local = store::create_issue(
        &app.db,
        &breadboard_server::dto::NewIssue {
            project: Some("TST".into()),
            title: "Task".into(),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    // Simulate Forgejo accepted the POST and the webhook claimed the local
    // card, but the subsequent writeback link returned an error.
    let marker = format!("<!-- breadboard-import:{} -->", local.key);
    fake.issues.lock().unwrap().push(json!({
        "number": 41, "title": "Task", "body": marker, "state": "open", "labels": [],
        "assignees": [], "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
    }));
    store::claim_unlinked_issue(&app.db, local.project_id, "Task", "Breadway/test", 41)
        .await
        .unwrap();
    let mut req = request();
    req.repair = true;
    let result = import::run(&app.state, &req).await.unwrap();
    assert!(result.pending_writebacks.is_empty());
    assert_eq!(fake.creates.load(Ordering::SeqCst), 0);
    assert!(store::get_issue_by_key(&app.db, &local.key)
        .await
        .unwrap()
        .forgejo_ref
        .is_some());
}

#[tokio::test]
async fn checked_import_item_is_done_locally_and_closed_remotely() {
    let (app, fake) = harness(false).await;
    let mut req = request();
    req.markdown = "# Epic\n- [x] Task".into();
    import::run(&app.state, &req).await.unwrap();
    let local = store::get_issue_by_key(&app.db, "TST-1").await.unwrap();
    assert_eq!(local.status, breadboard_core::Status::Done);
    assert_eq!(&*fake.states.lock().unwrap(), &["closed"]);
}
