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

    // /api and /events require auth.
    let unauth = c.get(format!("{base}/api/issues")).send().await.unwrap();
    assert_eq!(unauth.status(), 401);
    let unauth_events = c.get(format!("{base}/events")).send().await.unwrap();
    assert_eq!(unauth_events.status(), 401);

    // Seed project ROAD exists; create a real project too.
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
    let mut events = tu::EventStream::connect_with_auth(&base, &auth).await;

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
    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sync_log WHERE field IN ('status','labels')")
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
async fn cycle_project_defaults_only_apply_to_new_issues_and_explicit_cycle_wins() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let project: Value = c
        .post(format!("{base}/api/projects"))
        .header("authorization", &auth)
        .json(&json!({"key":"CYCLE","name":"Cycle project"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let project_id = project["id"].as_i64().unwrap();
    let cycle: Value = c.post(format!("{base}/api/cycles")).header("authorization", &auth)
        .json(&json!({"name":"Default", "starts_on":"2026-10-01", "ends_on":"2026-10-31", "project_ids":[project_id]}))
        .send().await.unwrap().json().await.unwrap();
    let cycle_id = cycle["id"].as_i64().unwrap();
    assert_eq!(cycle["project_ids"], json!([project_id]));
    let preexisting: Value = c.post(format!("{base}/api/issues")).header("authorization", &auth)
        .json(&json!({"title":"before selection", "project":"CYCLE", "cycle_id":null}))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(preexisting["cycle_id"], Value::Null);
    let created: Value = c
        .post(format!("{base}/api/issues"))
        .header("authorization", &auth)
        .json(&json!({"title":"inherited", "project":"CYCLE"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["cycle_id"], json!(cycle_id));
    let second: Value = c.post(format!("{base}/api/cycles")).header("authorization", &auth)
        .json(&json!({"name":"Also default", "starts_on":"2026-11-01", "ends_on":"2026-11-30", "project_ids":[project_id]}))
        .send().await.unwrap().json().await.unwrap();
    let explicit: Value = c
        .post(format!("{base}/api/issues"))
        .header("authorization", &auth)
        .json(&json!({"title":"explicit", "project":"CYCLE", "cycle_id":second["id"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(explicit["cycle_id"], second["id"]);
    let ambiguous = c
        .post(format!("{base}/api/issues"))
        .header("authorization", &auth)
        .json(&json!({"title":"ambiguous", "project":"CYCLE"}))
        .send()
        .await
        .unwrap();
    assert_eq!(ambiguous.status(), reqwest::StatusCode::CONFLICT);
    let explicit_id: Value = c
        .post(format!("{base}/api/issues"))
        .header("authorization", &auth)
        .json(&json!({"title":"chosen", "project":"CYCLE", "cycle_id":second["id"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(explicit_id["cycle_id"], second["id"]);
    let update = c
        .patch(format!("{base}/api/cycles/{cycle_id}"))
        .header("authorization", &auth)
        .json(&json!({"project_ids":[]}))
        .send()
        .await
        .unwrap();
    assert!(update.status().is_success());
    let old: Value = c
        .get(format!("{base}/api/issues/{}", created["key"].as_str().unwrap()))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(old["cycle_id"], json!(cycle_id));
    let unchanged: Value = c.get(format!("{base}/api/issues/{}", preexisting["key"].as_str().unwrap()))
        .header("authorization", &auth).send().await.unwrap().json().await.unwrap();
    assert_eq!(unchanged["cycle_id"], Value::Null);
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

    let mut events = tu::EventStream::connect_with_auth(&base, &auth).await;

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

#[tokio::test]
async fn events_accept_session_cookie_for_browser_clients() {
    let app = tu::spawn().await;
    let base = format!("http://{}", app.addr);
    let mut events = tu::EventStream::connect_with_cookie(&base, &app.session_token).await;

    let response = reqwest::Client::new()
        .post(format!("{base}/api/issues"))
        .header("authorization", format!("Bearer {}", app.token))
        .json(&json!({ "title": "cookie client event", "project": "ROAD" }))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert_eq!(events.next_type().await, "card.created");
}
