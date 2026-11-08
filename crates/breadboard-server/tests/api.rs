//! End-to-end API tests: boot the real router on an ephemeral port, talk to it
//! over HTTP. Auth uses a bearer token inserted straight into `api_tokens`
//! (the same path the CLI device flow will populate).

mod common;
use breadboard_client::Client;
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
    let cycle: Value = c
        .post(format!("{base}/api/cycles"))
        .header("authorization", &auth)
        .json(&json!({"name":"Default", "starts_on":"2026-10-01", "ends_on":"2026-10-31"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let cycle_id = cycle["id"].as_i64().unwrap();
    assert_eq!(cycle["project_ids"], json!([]));
    let preexisting: Value = c
        .post(format!("{base}/api/issues"))
        .header("authorization", &auth)
        .json(&json!({"title":"before selection", "project":"CYCLE"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(preexisting["cycle_id"], Value::Null);
    let selected_resp = c
        .patch(format!("{base}/api/cycles/{cycle_id}"))
        .header("authorization", &auth)
        .json(&json!({"project_ids":[project_id]}))
        .send()
        .await
        .unwrap();
    assert!(selected_resp.status().is_success());
    let selected: Value = selected_resp.json().await.unwrap();
    assert_eq!(selected["project_ids"], json!([project_id]));
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
        .get(format!(
            "{base}/api/issues/{}",
            created["key"].as_str().unwrap()
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(old["cycle_id"], json!(cycle_id));
    let unchanged: Value = c
        .get(format!(
            "{base}/api/issues/{}",
            preexisting["key"].as_str().unwrap()
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
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

#[tokio::test]
async fn reconcile_returns_json_body() {
    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    let resp = c
        .post(format!("{base}/api/sync/reconcile"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], json!(true));
}

#[tokio::test]
async fn client_reconcile_tolerates_json_body() {
    let app = tu::spawn().await;
    let base = format!("http://{}", app.addr);
    let client = Client::new(&base, Some(app.token.clone()));
    // must not panic or return Err on a 202 with body
    client.reconcile().await.unwrap();
}

#[tokio::test]
async fn events_accept_session_cookie_for_browser_clients() {
    let app = tu::spawn().await;
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let mut events = tu::EventStream::connect_with_cookie(&base, &app.session_token).await;

    // No seeded catch-all: this test files into its own project.
    c_ok(
        reqwest::Client::new()
            .post(format!("{base}/api/projects"))
            .header("authorization", &auth)
            .json(&json!({ "key": "EVT", "name": "Events" }))
            .send()
            .await
            .unwrap()
            .status(),
    );

    let response = reqwest::Client::new()
        .post(format!("{base}/api/issues"))
        .header("authorization", &auth)
        .json(&json!({ "title": "cookie client event", "project": "EVT" }))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert_eq!(events.next_type().await, "card.created");
}

fn c_ok(status: reqwest::StatusCode) {
    assert!(status.is_success(), "setup failed: {status}");
}

/// GET /api/issues is paged (`limit` default 500, `offset`, stable order) and
/// hydrates labels/links for a page in grouped queries rather than per row.
#[tokio::test]
async fn list_issues_pages_and_batches_hydration() {
    use breadboard_core::dto::NewIssue;

    let app = tu::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    c.post(format!("{base}/api/projects"))
        .header("authorization", &auth)
        .json(&json!({ "key": "PAGE", "name": "paging" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    for n in 0..600 {
        breadboard_server::store::create_issue(
            &app.db,
            &NewIssue {
                title: format!("paged item {n}"),
                project: Some("PAGE".into()),
                labels: vec!["batch".into(), format!("n{}", n % 3)],
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    }

    let get = |query: &str| {
        let c = c.clone();
        let base = base.clone();
        let auth = auth.clone();
        let query = query.to_string();
        async move {
            let body: Value = c
                .get(format!("{base}/api/issues{query}"))
                .header("authorization", &auth)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            body.as_array().unwrap().clone()
        }
    };

    // A bare request is capped at the default page, not the whole table.
    let first = get("").await;
    assert_eq!(first.len(), 500, "default page is 500 rows");
    assert!(first[0]["labels"].as_array().unwrap().contains(&json!("batch")));
    assert!(first[499]["labels"].as_array().unwrap().contains(&json!("batch")));

    let page = get("?limit=50").await;
    assert_eq!(page.len(), 50);

    let second = get("?limit=50&offset=50").await;
    assert_eq!(second.len(), 50);
    let keys: std::collections::HashSet<&str> =
        page.iter().map(|i| i["key"].as_str().unwrap()).collect();
    assert!(
        second
            .iter()
            .all(|i| !keys.contains(i["key"].as_str().unwrap())),
        "offset pages do not overlap"
    );

    let tail = get("?limit=50&offset=550").await;
    assert_eq!(tail.len(), 50);
    let past = get("?limit=50&offset=600").await;
    assert!(past.is_empty());

    // A limit above the table returns what exists; the hard cap is internal.
    let wide = get("?limit=9999").await;
    assert_eq!(wide.len(), 600);

    // Links hydrate for the whole page from the grouped query.
    let target = first[0]["id"].as_i64().unwrap();
    sqlx::query(
        "INSERT INTO issue_links (issue_id, target_id, kind) VALUES (?1, ?2, 'relates-to')",
    )
    .bind(first[1]["id"].as_i64().unwrap())
    .bind(target)
    .execute(&app.db)
    .await
    .unwrap();
    let with_link = get("?limit=500").await;
    let linked = with_link
        .iter()
        .find(|i| i["id"].as_i64() == Some(first[1]["id"].as_i64().unwrap()))
        .expect("linked row is in the page");
    assert_eq!(linked["links"][0]["target_id"], target);
    assert_eq!(linked["links"][0]["kind"], "relates-to");
}
