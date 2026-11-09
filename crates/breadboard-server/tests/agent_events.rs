//! Per-agent activity timeline: emit points, filters, pagination, links and
//! retention.

mod common;

use breadboard_core::{AgentEventKind, SyncSource};
use breadboard_server::agent_events::{self, AgentEventInsert};
use breadboard_server::forgejo::{FgIssue, FgPullRequestStub};
use breadboard_server::{store, sync};
use chrono::{Duration, Utc};
use serde_json::{json, Value};

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

async fn create_issue(c: &reqwest::Client, base: &str, auth: &str) -> Value {
    c.post(format!("{base}/api/issues"))
        .header("authorization", auth)
        .json(&json!({ "title": "timeline work", "project": "AGENT" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn get_events(c: &reqwest::Client, base: &str, auth: &str, name: &str, query: &str) -> Value {
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{query}")
    };
    let resp = c
        .get(format!("{base}/api/agents/{name}/events{suffix}"))
        .header("authorization", auth)
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "events query failed: {}",
        resp.status()
    );
    resp.json().await.unwrap()
}

fn kinds(page: &Value) -> Vec<String> {
    page["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn claim_state_pr_release_timeline_with_filters_and_paging() {
    let app = common::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    ensure_project(&c, &base, &auth).await;

    // Registered: only the first registration is an event.
    for _ in 0..2 {
        c.post(format!("{base}/api/agents/register"))
            .header("authorization", &auth)
            .json(&json!({
                "agent_name": "timeline-tester",
                "host": "hestia",
                "pid": 4242
            }))
            .send()
            .await
            .unwrap();
    }

    let issue = create_issue(&c, &base, &auth).await;
    let key = issue["key"].as_str().unwrap().to_string();

    // Claim with a PR already known: claimed + pr_opened.
    let claim = c
        .post(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "timeline-tester",
            "mode": "exclusive",
            "state": "implementing",
            "pr_url": "https://git.example/pulls/7",
            "pr_number": 7
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(claim.status(), reqwest::StatusCode::CREATED);

    // A real state change is logged.
    let heartbeat = c
        .put(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "state": "awaiting-review" }))
        .send()
        .await
        .unwrap();
    assert!(heartbeat.status().is_success());

    // An unchanged heartbeat is not another event.
    c.put(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .json(&json!({ "state": "awaiting-review" }))
        .send()
        .await
        .unwrap();

    let page = get_events(&c, &base, &auth, "timeline-tester", "").await;
    assert_eq!(
        kinds(&page),
        vec!["state_change", "pr_opened", "claimed", "registered"],
        "newest first, one row per real transition: {page}"
    );
    let first = &page["events"][0];
    assert_eq!(first["issue_key"], key);
    assert_eq!(
        first["summary"],
        format!("{key}: implementing -> awaiting-review")
    );
    let pr_event = page["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "pr_opened")
        .unwrap();
    assert_eq!(pr_event["pr_url"], "https://git.example/pulls/7");
    assert_eq!(pr_event["pr_number"], 7);
    assert_eq!(pr_event["issue_id"], issue["id"]);

    // Kind filter.
    let only_pr = get_events(&c, &base, &auth, "timeline-tester", "kind=pr_opened").await;
    assert_eq!(kinds(&only_pr), vec!["pr_opened"]);

    // Time range: a window that contains everything, and one that contains
    // nothing on either side.
    let since = (Utc::now() - Duration::hours(1))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let future = (Utc::now() + Duration::hours(1))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let past = (Utc::now() - Duration::hours(1))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let all = get_events(
        &c,
        &base,
        &auth,
        "timeline-tester",
        &format!("since={since}"),
    )
    .await;
    assert_eq!(all["events"].as_array().unwrap().len(), 4);
    let none = get_events(
        &c,
        &base,
        &auth,
        "timeline-tester",
        &format!("since={future}"),
    )
    .await;
    assert!(none["events"].as_array().unwrap().is_empty());
    let before_range = get_events(
        &c,
        &base,
        &auth,
        "timeline-tester",
        &format!("until={past}"),
    )
    .await;
    assert!(before_range["events"].as_array().unwrap().is_empty());

    // An invalid kind is a 400, not an empty page.
    let bad = c
        .get(format!(
            "{base}/api/agents/timeline-tester/events?kind=nope"
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), reqwest::StatusCode::BAD_REQUEST);

    // Cursor pagination: first page of two, then the rest.
    let page1 = get_events(&c, &base, &auth, "timeline-tester", "limit=2").await;
    assert_eq!(kinds(&page1), vec!["state_change", "pr_opened"]);
    assert_eq!(page1["has_more"], true);
    let cursor = page1["next_before"].as_i64().unwrap();
    let page2 = get_events(
        &c,
        &base,
        &auth,
        "timeline-tester",
        &format!("limit=2&before={cursor}"),
    )
    .await;
    assert_eq!(kinds(&page2), vec!["claimed", "registered"]);
    assert_eq!(page2["has_more"], false);
    assert!(page2["next_before"].is_null());

    // Release lands at the top with its actor.
    let released = c
        .delete(format!("{base}/api/issues/{key}/claim"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(released.status(), reqwest::StatusCode::NO_CONTENT);
    let after_release = get_events(&c, &base, &auth, "timeline-tester", "").await;
    assert_eq!(kinds(&after_release)[0], "released");
    assert_eq!(after_release["events"][0]["actor"], "tester");
}

#[tokio::test]
async fn agent_chat_records_a_linked_message_event() {
    let app = common::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);

    c.post(format!("{base}/api/agents/register"))
        .header("authorization", &auth)
        .json(&json!({ "agent_name": "chatty-agent" }))
        .send()
        .await
        .unwrap();

    let posted: Value = c
        .post(format!("{base}/api/chat/messages"))
        .header("authorization", &auth)
        .json(&json!({
            "agent_name": "chatty-agent",
            "room": "lobby",
            "body": "reporting in from the timeline"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let page = get_events(&c, &base, &auth, "chatty-agent", "kind=chat_sent").await;
    assert_eq!(kinds(&page), vec!["chat_sent"]);
    let event = &page["events"][0];
    assert_eq!(event["room_id"], posted["room_id"]);
    assert_eq!(event["message_id"], posted["id"]);
    assert_eq!(event["summary"], "reporting in from the timeline");
}

fn fg_issue(number: i64, title: &str, body: &str, state: &str, merged: bool) -> FgIssue {
    FgIssue {
        number,
        title: title.into(),
        body: body.into(),
        state: state.into(),
        labels: vec![],
        assignees: None,
        milestone: None,
        user: None,
        pull_request: merged.then_some(FgPullRequestStub { merged: true }),
        created_at: "2026-09-01T00:00:00Z".into(),
        updated_at: Utc::now().to_rfc3339(),
    }
}

#[tokio::test]
async fn observed_merge_writes_pr_merged_for_each_claimant() {
    let app = common::spawn().await;
    let c = reqwest::Client::new();
    let base = format!("http://{}", app.addr);
    let auth = format!("Bearer {}", app.token);
    let repo = "Breadway/timeline";

    store::upsert_project_for_repo(&app.db, "TST", "timeline", repo)
        .await
        .unwrap();
    store::upsert_repo(&app.db, repo, true).await.unwrap();

    // Two targets: one closed by a merged PR, one by a closed-unmerged PR.
    sync::apply_issue(
        &app.state,
        repo,
        &fg_issue(100, "merged target", "", "open", false),
        SyncSource::ForgejoWebhook,
    )
    .await
    .unwrap();
    sync::apply_issue(
        &app.state,
        repo,
        &fg_issue(101, "abandoned target", "", "open", false),
        SyncSource::ForgejoWebhook,
    )
    .await
    .unwrap();

    for (agent, key) in [("merge-watcher", "TST-1"), ("closed-watcher", "TST-2")] {
        let resp = c
            .post(format!("{base}/api/issues/{key}/claim"))
            .header("authorization", &auth)
            .json(&json!({ "agent_name": agent, "state": "awaiting-merge" }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::CREATED);
    }

    let merged = fg_issue(11, "land it", "Closes TST-1", "closed", true);
    sync::apply_issue(&app.state, repo, &merged, SyncSource::ForgejoWebhook)
        .await
        .unwrap();
    let abandoned = fg_issue(12, "drop it", "Closes TST-2", "closed", false);
    sync::apply_issue(&app.state, repo, &abandoned, SyncSource::ForgejoWebhook)
        .await
        .unwrap();

    let merged_page = get_events(&c, &base, &auth, "merge-watcher", "kind=pr_merged").await;
    assert_eq!(kinds(&merged_page), vec!["pr_merged"]);
    let event = &merged_page["events"][0];
    assert_eq!(event["issue_key"], "TST-1");
    assert_eq!(event["pr_number"], 11);
    assert!(event["pr_url"]
        .as_str()
        .unwrap()
        .ends_with("/Breadway/timeline/pulls/11"));

    // A closed-unmerged PR is not a merge: no event, and the target keeps its
    // status instead of being reported done.
    let abandoned_page = get_events(&c, &base, &auth, "closed-watcher", "kind=pr_merged").await;
    assert!(abandoned_page["events"].as_array().unwrap().is_empty());
    assert_eq!(
        store::get_issue_by_key(&app.db, "TST-2")
            .await
            .unwrap()
            .status,
        breadboard_core::Status::Doing
    );
}

#[tokio::test]
async fn retention_prunes_only_rows_older_than_the_window() {
    let app = common::spawn().await;

    agent_events::record(
        &app.db,
        AgentEventInsert::new("old-agent", AgentEventKind::Registered).summary("old"),
    )
    .await
    .unwrap();
    agent_events::record(
        &app.db,
        AgentEventInsert::new("new-agent", AgentEventKind::Registered).summary("new"),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE agent_events SET at = ?1 WHERE agent_name = 'old-agent'")
        .bind((Utc::now() - Duration::days(120)).to_rfc3339())
        .execute(&app.db)
        .await
        .unwrap();

    let deleted = agent_events::prune_agent_events(&app.db, 90).await.unwrap();
    assert_eq!(deleted, 1);
    let left: Vec<String> = sqlx::query_scalar("SELECT agent_name FROM agent_events")
        .fetch_all(&app.db)
        .await
        .unwrap();
    assert_eq!(left, vec!["new-agent".to_string()]);
}
