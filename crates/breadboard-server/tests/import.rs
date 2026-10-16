mod common;

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

use axum::{extract::State, http::StatusCode, routing::post, Router};
use breadboard_server::{
    import::{self, ImportRequest},
    store,
};
use serde_json::json;

#[derive(Clone)]
struct FakeForgejo {
    deny: Arc<AtomicBool>,
    creates: Arc<AtomicUsize>,
}

async fn create_issue(
    State(fake): State<FakeForgejo>,
) -> (StatusCode, axum::Json<serde_json::Value>) {
    fake.creates.fetch_add(1, Ordering::SeqCst);
    if fake.deny.load(Ordering::SeqCst) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({"message":"denied"})),
        );
    }
    (
        StatusCode::CREATED,
        axum::Json(json!({
            "number": 40 + fake.creates.load(Ordering::SeqCst) as i64, "title": "Task", "body": "", "state": "open",
            "labels": [], "assignees": [], "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        })),
    )
}

async fn harness(deny: bool) -> (common::TestApp, FakeForgejo) {
    let fake = FakeForgejo {
        deny: Arc::new(AtomicBool::new(deny)),
        creates: Arc::new(AtomicUsize::new(0)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = Router::new()
        .route("/api/v1/repos/Breadway/test/issues", post(create_issue))
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
    let retry = import::run(&app.state, &request()).await.unwrap();
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
    let result = import::run(&app.state, &request()).await.unwrap();
    assert!(result.pending_writebacks.is_empty());
    let repaired = store::get_issue_by_key(&app.db, "TST-1").await.unwrap();
    assert_eq!(repaired.id, local.id);
    assert_eq!(repaired.body, "preserve body");
    assert_eq!(repaired.priority, breadboard_core::Priority::High);
    assert_eq!(repaired.forgejo_ref.unwrap().number, 41);
    import::run(&app.state, &request()).await.unwrap();
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
