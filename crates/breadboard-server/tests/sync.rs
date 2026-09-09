//! Forgejo sync engine: inbound apply (LWW + drift + echo absorption) and
//! webhook HMAC verification.

mod common;

use breadboard_core::{IssueType, Status, SyncSource};
use breadboard_server::forgejo::{FgComment, FgIssue, FgMilestone, FgUser};
use breadboard_server::{store, sync};
use chrono::{Duration, Utc};
use serde_json::json;

fn fg_issue(number: i64, title: &str, updated: chrono::DateTime<Utc>) -> FgIssue {
    FgIssue {
        number,
        title: title.into(),
        body: String::new(),
        state: "open".into(),
        labels: vec![],
        assignees: None,
        milestone: None,
        user: None,
        pull_request: None,
        created_at: "2026-09-01T00:00:00Z".into(),
        updated_at: updated.to_rfc3339(),
    }
}

async fn sync_log_count(db: &sqlx::SqlitePool, field: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM sync_log WHERE field = ?1")
        .bind(field)
        .fetch_one(db)
        .await
        .unwrap()
}

#[tokio::test]
async fn inbound_import_update_drift_and_echo() {
    let app = common::spawn().await;
    let repo = "Breadway/bread";

    // Map the repo to a project + enable sync.
    store::upsert_project_for_repo(&app.db, "BREAD", "bread", repo)
        .await
        .unwrap();
    store::upsert_repo(&app.db, repo, true).await.unwrap();
    let project = store::get_project_by_repo(&app.db, repo).await.unwrap();

    // 1. First webhook: imports the issue.
    let mut fg = fg_issue(7, "OSD on wrong monitor", Utc::now());
    fg.assignees = Some(vec![user("riley", 42)]);
    fg.labels = vec![label("bug"), label("ui")];

    sync::apply_issue(&app.state, repo, &fg, SyncSource::ForgejoWebhook)
        .await
        .unwrap();

    let imported = store::find_issue_by_forgejo(&app.db, repo, 7)
        .await
        .unwrap()
        .expect("issue imported");
    assert_eq!(imported.key, "BREAD-1");
    assert_eq!(imported.title, "OSD on wrong monitor");
    assert_eq!(imported.labels, vec!["bug", "ui"]);
    assert!(imported.assignee_id.is_some());
    assert_eq!(imported.forgejo_ref.unwrap().number, 7);

    // 2. Echo (identical values, newer ts): no-op, no new sync_log rows.
    let before_logs = sync_log_count(&app.db, "title").await;
    let mut echo = fg.clone();
    echo.updated_at = (Utc::now() + Duration::seconds(1)).to_rfc3339();
    sync::apply_issue(&app.state, repo, &echo, SyncSource::ForgejoWebhook)
        .await
        .unwrap();
    assert_eq!(sync_log_count(&app.db, "title").await, before_logs);

    // 3. Real inbound change, Forgejo newer: applied + logged.
    let mut changed = fg.clone();
    changed.title = "OSD renders on the wrong monitor".into();
    changed.state = "closed".into();
    changed.updated_at = (Utc::now() + Duration::seconds(60)).to_rfc3339();
    sync::apply_issue(&app.state, repo, &changed, SyncSource::ForgejoWebhook)
        .await
        .unwrap();
    let updated = store::get_issue_by_key(&app.db, "BREAD-1").await.unwrap();
    assert_eq!(updated.title, "OSD renders on the wrong monitor");
    assert_eq!(updated.status, Status::Done); // closed -> done
    assert_eq!(sync_log_count(&app.db, "title").await, before_logs + 1);
    assert!(sync_log_count(&app.db, "status").await >= 1);

    // 4. Stale inbound change (Forgejo OLDER than our last write): drift, no apply.
    let logs_before_drift = sync_log_count(&app.db, "title").await;
    let mut stale = fg.clone();
    stale.title = "a stale title from the past".into();
    stale.updated_at = (Utc::now() - Duration::days(5)).to_rfc3339();
    sync::apply_issue(&app.state, repo, &stale, SyncSource::ForgejoReconcile)
        .await
        .unwrap();
    let still = store::get_issue_by_key(&app.db, "BREAD-1").await.unwrap();
    assert_eq!(still.title, "OSD renders on the wrong monitor"); // unchanged
    assert_eq!(sync_log_count(&app.db, "title").await, logs_before_drift); // not logged as a change

    _ = project;
}

#[tokio::test]
async fn unmapped_repo_is_skipped() {
    let app = common::spawn().await;
    let repo = "Breadway/unmapped";
    store::upsert_repo(&app.db, repo, true).await.unwrap();
    let fg = fg_issue(1, "nope", Utc::now());
    sync::apply_issue(&app.state, repo, &fg, SyncSource::ForgejoWebhook)
        .await
        .unwrap();
    assert!(store::find_issue_by_forgejo(&app.db, repo, 1)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn webhook_hmac_gate() {
    let app = common::spawn().await;
    let base = format!("http://{}", app.addr);
    let c = reqwest::Client::new();

    store::upsert_project_for_repo(&app.db, "BREAD", "bread", "Breadway/bread")
        .await
        .unwrap();
    store::upsert_repo(&app.db, "Breadway/bread", true).await.unwrap();

    let payload = json!({
        "action": "opened",
        "issue": {
            "number": 99,
            "title": "from a webhook",
            "body": "",
            "state": "open",
            "created_at": "2026-09-01T00:00:00Z",
            "updated_at": Utc::now().to_rfc3339(),
        },
        "repository": { "full_name": "Breadway/bread" }
    });
    let body = serde_json::to_vec(&payload).unwrap();

    // Bad signature -> 401.
    let bad = c
        .post(format!("{base}/webhooks/forgejo"))
        .header("x-forgejo-event", "issues")
        .header("x-forgejo-signature", "deadbeef")
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 401);

    // Good signature -> 200 and the issue lands.
    let ok = c
        .post(format!("{base}/webhooks/forgejo"))
        .header("x-forgejo-event", "issues")
        .header("x-forgejo-signature", common::sign(&body))
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);

    let imported = store::find_issue_by_forgejo(&app.db, "Breadway/bread", 99)
        .await
        .unwrap();
    assert!(imported.is_some(), "webhook issue imported");
}

#[tokio::test]
async fn pr_import_and_milestone_sync() {
    let app = common::spawn().await;
    let repo = "Breadway/bread";
    store::upsert_project_for_repo(&app.db, "BREAD", "bread", repo)
        .await
        .unwrap();
    store::upsert_repo(&app.db, repo, true).await.unwrap();

    // A PR (is_pr via pull_request field) with a milestone.
    let mut pr = fg_issue(12, "add the sync engine", Utc::now());
    pr.pull_request = Some(breadboard_server::forgejo::FgPullRequestStub { merged: false });
    pr.milestone = Some(FgMilestone { title: "v1.0".into() });
    sync::apply_issue(&app.state, repo, &pr, SyncSource::ForgejoWebhook)
        .await
        .unwrap();

    let imported = store::find_issue_by_forgejo(&app.db, repo, 12)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(imported.issue_type, IssueType::Pr);
    assert_eq!(imported.milestone.as_deref(), Some("v1.0"));

    // Milestone change flows in.
    let mut pr2 = pr.clone();
    pr2.milestone = Some(FgMilestone { title: "v1.1".into() });
    pr2.updated_at = (Utc::now() + Duration::seconds(30)).to_rfc3339();
    sync::apply_issue(&app.state, repo, &pr2, SyncSource::ForgejoReconcile)
        .await
        .unwrap();
    let again = store::get_issue_by_key(&app.db, "BREAD-1").await.unwrap();
    assert_eq!(again.milestone.as_deref(), Some("v1.1"));
    assert!(sync_log_count(&app.db, "milestone").await >= 1);
}

#[tokio::test]
async fn breadboard_comment_absorbs_its_own_echo() {
    let app = common::spawn().await;
    let repo = "Breadway/bread";
    store::upsert_project_for_repo(&app.db, "BREAD", "bread", repo)
        .await
        .unwrap();
    store::upsert_repo(&app.db, repo, true).await.unwrap();
    let fg = fg_issue(3, "issue with comments", Utc::now());
    sync::apply_issue(&app.state, repo, &fg, SyncSource::ForgejoWebhook)
        .await
        .unwrap();
    let issue = store::get_issue_by_key(&app.db, "BREAD-1").await.unwrap();

    // breadboard-authored comment (no external_id; Forgejo not configured in tests).
    let local = store::create_comment(&app.db, issue.id, None, "looks good to me")
        .await
        .unwrap();
    assert!(local.external_id.is_none());

    // The webhook echo of that same comment arrives.
    let echo = FgComment {
        id: 555,
        body: "looks good to me".into(),
        user: None,
        created_at: Utc::now().to_rfc3339(),
        updated_at: Utc::now().to_rfc3339(),
    };
    sync::apply_comment(&app.state, repo, 3, &echo).await.unwrap();

    let comments = store::list_comments(&app.db, issue.id).await.unwrap();
    assert_eq!(comments.len(), 1, "echo claimed the local comment, no dup");
    assert_eq!(comments[0].external_id, Some(555));
}

#[test]
fn status_boundary_mapping() {
    // open keeps a fine-grained status; closed forces done; reopen -> todo.
    assert_eq!(sync::map_status(false, Status::Doing), Status::Doing);
    assert_eq!(sync::map_status(true, Status::Doing), Status::Done);
    assert_eq!(sync::map_status(false, Status::Done), Status::Todo);
    assert_eq!(sync::map_status(true, Status::Done), Status::Done);
}

// --- helpers ---

fn user(login: &str, id: i64) -> FgUser {
    FgUser {
        id,
        login: login.to_string(),
        full_name: String::new(),
        avatar_url: None,
    }
}

fn label(name: &str) -> breadboard_server::forgejo::FgLabel {
    breadboard_server::forgejo::FgLabel {
        id: 0,
        name: name.to_string(),
    }
}
