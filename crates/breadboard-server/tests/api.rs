//! End-to-end API tests: boot the real router on an ephemeral port, talk to it
//! over HTTP. Auth uses a bearer token inserted straight into `api_tokens`
//! (the same path the CLI device flow will populate).

mod common;
use common as tu;
use serde_json::{json, Value};

#[tokio::test]
async fn issue_lifecycle_and_events() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    // /healthz is public.
    let health: Value = c
        .get(format!("{base}/healthz"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["db"], json!(true));

    // /api requires auth.
    let unauth = c.get(format!("{base}/api/issues")).send().await.unwrap();
    assert_eq!(unauth.status(), 401);

    // No project is seeded: create the one this work belongs to.
    let project: Value = c
        .post(format!("{base}/api/projects"))
        .header("authorization", &auth)
        .json(&json!({ "key": "BREAD", "name": "bread" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(project["key"], "BREAD");

    // Open the SSE stream before mutating so we capture the events.
    let mut events = tu::EventStream::connect(&base).await;

    // Create an issue with just a title.
    let created: Value = c
        .post(format!("{base}/api/issues"))
        .header("authorization", &auth)
        .json(&json!({ "title": "fix the OSD monitor bug", "project": "BREAD" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["key"], "BREAD-1");
    assert_eq!(created["status"], "backlog");
    let key = created["key"].as_str().unwrap().to_string();

    assert_eq!(events.next_type().await, "card.created");

    // Move it across the board.
    let moved: Value = c
        .patch(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .json(&json!({ "status": "doing", "labels": ["bug", "ui"] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(moved["status"], "doing");
    assert_eq!(moved["labels"], json!(["bug", "ui"]));

    let mut seen = std::collections::HashSet::new();
    for _ in 0..2 {
        seen.insert(events.next_type().await);
    }
    assert!(seen.contains("card.moved"), "got {seen:?}");
    assert!(seen.contains("card.updated"), "got {seen:?}");

    // sync_log recorded the status + labels change (synced fields).
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_log WHERE field IN ('status','labels')")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(n, 2);

    // Detail view includes (empty) comments.
    let detail: Value = c
        .get(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["comments"], json!([]));

    // Second issue increments the per-project counter.
    let second: Value = c
        .post(format!("{base}/api/issues"))
        .header("authorization", &auth)
        .json(&json!({ "title": "second", "project": "BREAD" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(second["key"], "BREAD-2");
    assert_eq!(events.next_type().await, "card.created");

    // Filter by status.
    let doing: Value = c
        .get(format!("{base}/api/issues?status=doing"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(doing.as_array().unwrap().len(), 1);

    // Delete.
    let del = c
        .delete(format!("{base}/api/issues/{key}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 204);
    assert_eq!(events.next_type().await, "card.deleted");
}

#[tokio::test]
async fn cycle_start_end_events() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let cycle: Value = c
        .post(format!("{base}/api/cycles"))
        .header("authorization", &auth)
        .json(&json!({ "name": "cycle 1", "starts_on": "2026-09-01", "ends_on": "2026-09-14" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = cycle["id"].as_i64().unwrap();

    let mut events = tu::EventStream::connect(&base).await;

    c.post(format!("{base}/api/cycles/{id}/start"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(events.next_type().await, "cycle.started");

    let ended: Value = c
        .post(format!("{base}/api/cycles/{id}/end"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ended["closed"], json!(true));
    assert_eq!(events.next_type().await, "cycle.ended");
}

/// There is no catch-all project: a title-only issue must name its home as
/// soon as more than one project exists, rather than silently landing in
/// whichever project happens to be first.
#[tokio::test]
async fn issue_requires_a_project_once_there_is_a_choice() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    // Empty database: nothing to file into, so say so.
    let none = c
        .post(format!("{base}/api/issues"))
        .header("authorization", &auth)
        .json(&json!({ "title": "orphan" }))
        .send()
        .await
        .unwrap();
    assert_eq!(none.status(), 400);
    let body: Value = none.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("no project exists"));

    // Exactly one project: the title-only shortcut is unambiguous again.
    c.post(format!("{base}/api/projects"))
        .header("authorization", &auth)
        .json(&json!({ "key": "ONLY", "name": "Only project" }))
        .send()
        .await
        .unwrap();
    let single = c
        .post(format!("{base}/api/issues"))
        .header("authorization", &auth)
        .json(&json!({ "title": "filed by default" }))
        .send()
        .await
        .unwrap();
    assert_eq!(single.status(), 201);
    let single: Value = single.json().await.unwrap();
    assert_eq!(single["key"], "ONLY-1");

    // A second project makes the choice ambiguous: refuse and list them.
    c.post(format!("{base}/api/projects"))
        .header("authorization", &auth)
        .json(&json!({ "key": "OTHER", "name": "Other project" }))
        .send()
        .await
        .unwrap();
    let ambiguous = c
        .post(format!("{base}/api/issues"))
        .header("authorization", &auth)
        .json(&json!({ "title": "who owns this?" }))
        .send()
        .await
        .unwrap();
    assert_eq!(ambiguous.status(), 400);
    let body: Value = ambiguous.json().await.unwrap();
    let err = body["error"].as_str().unwrap();
    assert!(err.contains("project is required"), "{err}");
    assert!(err.contains("ONLY") && err.contains("OTHER"), "{err}");
}
