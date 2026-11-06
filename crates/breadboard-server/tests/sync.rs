//! Forgejo sync engine: inbound apply (LWW + drift + echo absorption) and
//! webhook HMAC verification.

mod common;

use breadboard_core::{BoardEvent, IssueType, LinkKind, Status, SyncSource};
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
    store::upsert_repo(&app.db, "Breadway/bread", true)
        .await
        .unwrap();
    let project = store::get_project_by_repo(&app.db, "Breadway/bread")
        .await
        .unwrap();
    let cycle = store::create_cycle(
        &app.db,
        &breadboard_core::dto::NewCycle {
            name: "Webhook default".into(),
            starts_on: chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            ends_on: chrono::NaiveDate::from_ymd_opt(2026, 10, 31).unwrap(),
            project_ids: vec![],
        },
    )
    .await
    .unwrap();
    store::update_cycle_projects(&app.db, cycle.id, &[project.id])
        .await
        .unwrap();

    let payload = json!({
        "action": "opened",
        "issue": {
            "number": 99,
            "title": "from a webhook",
            "body": "",
            "state": "open",
            "created_at": Utc::now().to_rfc3339(),
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
    assert_eq!(
        imported.unwrap().cycle_id,
        Some(cycle.id),
        "signed webhook import inherits its project's unique default"
    );
}

#[tokio::test]
async fn new_webhook_issues_inherit_unique_cycle_default_but_history_and_prs_do_not() {
    let app = common::spawn().await;
    let repo = "Breadway/bread";
    store::upsert_project_for_repo(&app.db, "BREAD", "bread", repo)
        .await
        .unwrap();
    store::upsert_repo(&app.db, repo, true).await.unwrap();
    let project = store::get_project_by_repo(&app.db, repo).await.unwrap();
    let cycle = store::create_cycle(
        &app.db,
        &breadboard_core::dto::NewCycle {
            name: "Current".into(),
            starts_on: chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            ends_on: chrono::NaiveDate::from_ymd_opt(2026, 10, 31).unwrap(),
            project_ids: vec![project.id],
        },
    )
    .await
    .unwrap();
    store::update_cycle_projects(&app.db, cycle.id, &[project.id])
        .await
        .unwrap();

    let mut recent = fg_issue(301, "new external issue", Utc::now());
    recent.created_at = Utc::now().to_rfc3339();
    sync::apply_issue(&app.state, repo, &recent, SyncSource::ForgejoWebhook)
        .await
        .unwrap();
    let imported = store::find_issue_by_forgejo(&app.db, repo, 301)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(imported.cycle_id, Some(cycle.id));

    let mut historical = fg_issue(302, "old backlog issue", Utc::now());
    historical.created_at = "2020-01-01T00:00:00Z".into();
    sync::apply_issue(&app.state, repo, &historical, SyncSource::ForgejoReconcile)
        .await
        .unwrap();
    let historical = store::find_issue_by_forgejo(&app.db, repo, 302)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(historical.cycle_id, None);

    let mut pr = fg_issue(303, "pull request", Utc::now());
    pr.created_at = Utc::now().to_rfc3339();
    pr.pull_request = Some(breadboard_server::forgejo::FgPullRequestStub { merged: false });
    sync::apply_issue(&app.state, repo, &pr, SyncSource::ForgejoWebhook)
        .await
        .unwrap();
    let pr = store::find_issue_by_forgejo(&app.db, repo, 303)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pr.cycle_id, None);

    let second = store::create_cycle(
        &app.db,
        &breadboard_core::dto::NewCycle {
            name: "Also current".into(),
            starts_on: chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
            ends_on: chrono::NaiveDate::from_ymd_opt(2026, 11, 30).unwrap(),
            project_ids: vec![project.id],
        },
    )
    .await
    .unwrap();
    store::update_cycle_projects(&app.db, second.id, &[project.id])
        .await
        .unwrap();
    let second_id = second.id;
    let mut conflict = fg_issue(304, "ambiguous defaults", Utc::now());
    conflict.created_at = Utc::now().to_rfc3339();
    sync::apply_issue(&app.state, repo, &conflict, SyncSource::ForgejoWebhook)
        .await
        .unwrap();
    let imported = store::find_issue_by_forgejo(&app.db, repo, 304)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(imported.cycle_id, Some(second_id));
    assert_eq!(imported.cycle_id, None);
}

#[tokio::test]
async fn reconcile_new_external_issues_inherit_project_cycle_default() {
    let app = common::spawn().await;
    let repo = "Breadway/bread";
    store::upsert_project_for_repo(&app.db, "BREAD", "bread", repo)
        .await
        .unwrap();
    store::upsert_repo(&app.db, repo, true).await.unwrap();
    let project = store::get_project_by_repo(&app.db, repo).await.unwrap();
    let cycle = store::create_cycle(
        &app.db,
        &breadboard_core::dto::NewCycle {
            name: "Reconcile default".into(),
            starts_on: chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            ends_on: chrono::NaiveDate::from_ymd_opt(2026, 10, 31).unwrap(),
            project_ids: vec![],
        },
    )
    .await
    .unwrap();
    store::update_cycle_projects(&app.db, cycle.id, &[project.id])
        .await
        .unwrap();

    let mut external = fg_issue(401, "reconcile import", Utc::now());
    external.created_at = Utc::now().to_rfc3339();
    sync::apply_issue(&app.state, repo, &external, SyncSource::ForgejoReconcile)
        .await
        .unwrap();
    let imported = store::find_issue_by_forgejo(&app.db, repo, 401)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(imported.cycle_id, Some(cycle.id));
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
    pr.milestone = Some(FgMilestone {
        title: "v1.0".into(),
    });
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
    pr2.milestone = Some(FgMilestone {
        title: "v1.1".into(),
    });
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
    sync::apply_comment(&app.state, repo, 3, &echo)
        .await
        .unwrap();

    let comments = store::list_comments(&app.db, issue.id).await.unwrap();
    assert_eq!(comments.len(), 1, "echo claimed the local comment, no dup");
    assert_eq!(comments[0].external_id, Some(555));
}

/// The non-race half of direction 1: writeback has already recorded the
/// external_id by the time Forgejo echoes the comment back. The echo must
/// update in place (no duplicate) and must not re-attribute the comment to
/// the webhook's author.
#[tokio::test]
async fn breadboard_comment_echo_after_writeback_recorded() {
    let app = common::spawn().await;
    let repo = "Breadway/bread";
    store::upsert_project_for_repo(&app.db, "BREAD", "bread", repo)
        .await
        .unwrap();
    store::upsert_repo(&app.db, repo, true).await.unwrap();
    sync::apply_issue(
        &app.state,
        repo,
        &fg_issue(3, "issue with comments", Utc::now()),
        SyncSource::ForgejoWebhook,
    )
    .await
    .unwrap();
    let issue = store::get_issue_by_key(&app.db, "BREAD-1").await.unwrap();

    // Local comment by breadboard user 1, then writeback records its Forgejo id.
    let local = store::create_comment(&app.db, issue.id, Some(1), "ship it")
        .await
        .unwrap();
    store::set_comment_external_id(&app.db, local.id, 900)
        .await
        .unwrap();

    // Forgejo echoes it back, attributed to the service account (user 7).
    let echo = FgComment {
        id: 900,
        body: "ship it".into(),
        user: Some(user("breadboard-bot", 7)),
        created_at: Utc::now().to_rfc3339(),
        updated_at: (Utc::now() + Duration::seconds(1)).to_rfc3339(),
    };
    sync::apply_comment(&app.state, repo, 3, &echo)
        .await
        .unwrap();

    let comments = store::list_comments(&app.db, issue.id).await.unwrap();
    assert_eq!(comments.len(), 1, "echo updated in place, no dup");
    assert_eq!(comments[0].external_id, Some(900));
    assert_eq!(
        comments[0].author_id,
        Some(1),
        "author stays the breadboard user"
    );
}

/// Direction 2: a comment that originates in Forgejo is imported exactly once
/// and repeated webhooks for the same comment id are idempotent.
#[tokio::test]
async fn forgejo_origin_comment_imported_once() {
    let app = common::spawn().await;
    let repo = "Breadway/bread";
    store::upsert_project_for_repo(&app.db, "BREAD", "bread", repo)
        .await
        .unwrap();
    store::upsert_repo(&app.db, repo, true).await.unwrap();
    sync::apply_issue(
        &app.state,
        repo,
        &fg_issue(4, "forgejo side chat", Utc::now()),
        SyncSource::ForgejoWebhook,
    )
    .await
    .unwrap();
    let issue = store::get_issue_by_key(&app.db, "BREAD-1").await.unwrap();

    let fg_comment = FgComment {
        id: 1234,
        body: "found it in the logs".into(),
        user: Some(user("otherdev", 88)),
        created_at: Utc::now().to_rfc3339(),
        updated_at: Utc::now().to_rfc3339(),
    };
    sync::apply_comment(&app.state, repo, 4, &fg_comment)
        .await
        .unwrap();
    // Redelivery (Forgejo retry, or a reconcile-style re-apply): still one row.
    sync::apply_comment(&app.state, repo, 4, &fg_comment)
        .await
        .unwrap();

    let comments = store::list_comments(&app.db, issue.id).await.unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].external_id, Some(1234));
    assert!(
        comments[0].author_id.is_some(),
        "attributed to the Forgejo author"
    );
}

/// The edit case: an edited Forgejo comment (same id, new body) updates the
/// existing row rather than creating a second one.
#[tokio::test]
async fn forgejo_comment_edit_updates_in_place() {
    let app = common::spawn().await;
    let repo = "Breadway/bread";
    store::upsert_project_for_repo(&app.db, "BREAD", "bread", repo)
        .await
        .unwrap();
    store::upsert_repo(&app.db, repo, true).await.unwrap();
    sync::apply_issue(
        &app.state,
        repo,
        &fg_issue(5, "edited comment", Utc::now()),
        SyncSource::ForgejoWebhook,
    )
    .await
    .unwrap();
    let issue = store::get_issue_by_key(&app.db, "BREAD-1").await.unwrap();

    let mut c = FgComment {
        id: 42,
        body: "frist".into(),
        user: Some(user("typo", 5)),
        created_at: Utc::now().to_rfc3339(),
        updated_at: Utc::now().to_rfc3339(),
    };
    sync::apply_comment(&app.state, repo, 5, &c).await.unwrap();

    c.body = "first".into();
    c.updated_at = (Utc::now() + Duration::seconds(30)).to_rfc3339();
    sync::apply_comment(&app.state, repo, 5, &c).await.unwrap();

    let comments = store::list_comments(&app.db, issue.id).await.unwrap();
    assert_eq!(comments.len(), 1, "edit updates in place");
    assert_eq!(comments[0].body, "first");
    assert_eq!(comments[0].external_id, Some(42));
}

#[test]
fn status_boundary_mapping() {
    // open keeps a fine-grained status; closed forces done; reopen -> todo.
    assert_eq!(sync::map_status(false, Status::Doing), Status::Doing);
    assert_eq!(sync::map_status(true, Status::Doing), Status::Done);
    assert_eq!(sync::map_status(false, Status::Done), Status::Todo);
    assert_eq!(sync::map_status(true, Status::Done), Status::Done);
}

/// Every `SyncDrift` sitting in the broadcast channel, as
/// `(field, winner, breadboard_value, forgejo_value)`.
fn drain_drift(
    rx: &mut tokio::sync::broadcast::Receiver<BoardEvent>,
) -> Vec<(String, String, String, String)> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let BoardEvent::SyncDrift {
            field,
            winner,
            breadboard_value,
            forgejo_value,
            ..
        } = ev
        {
            out.push((field, winner, breadboard_value, forgejo_value));
        }
    }
    out
}

#[tokio::test]
async fn reconcile_drift_signals_a_missed_webhook() {
    let app = common::spawn().await;
    let repo = "Breadway/bread";
    store::upsert_project_for_repo(&app.db, "BREAD", "bread", repo)
        .await
        .unwrap();
    store::upsert_repo(&app.db, repo, true).await.unwrap();

    // Import via the webhook fast path.
    let fg = fg_issue(7, "OSD on wrong monitor", Utc::now());
    sync::apply_issue(&app.state, repo, &fg, SyncSource::ForgejoWebhook)
        .await
        .unwrap();

    let mut rx = app.state.bus.subscribe();

    // 1. A webhook was missed: Forgejo changed the title and is the newer side.
    //    Reconcile applies it AND emits drift (winner: forgejo).
    let mut changed = fg.clone();
    changed.title = "OSD renders on the wrong monitor".into();
    changed.updated_at = (Utc::now() + Duration::seconds(60)).to_rfc3339();
    sync::apply_issue(&app.state, repo, &changed, SyncSource::ForgejoReconcile)
        .await
        .unwrap();

    assert_eq!(
        drain_drift(&mut rx),
        vec![(
            "title".to_string(),
            "forgejo".to_string(),
            "OSD on wrong monitor".to_string(),
            "OSD renders on the wrong monitor".to_string(),
        )],
        "reconcile applies the missed change and emits one drift event"
    );
    assert_eq!(
        store::get_issue_by_key(&app.db, "BREAD-1")
            .await
            .unwrap()
            .title,
        "OSD renders on the wrong monitor"
    );

    // 2. Drift the other way: a stale Forgejo value seen during reconcile.
    //    breadboard wins (no apply), but the divergence is still signalled.
    let mut stale = fg.clone();
    stale.title = "a title from the past".into();
    stale.updated_at = (Utc::now() - Duration::days(5)).to_rfc3339();
    sync::apply_issue(&app.state, repo, &stale, SyncSource::ForgejoReconcile)
        .await
        .unwrap();

    let drift = drain_drift(&mut rx);
    assert_eq!(drift.len(), 1);
    assert_eq!(drift[0].0, "title");
    assert_eq!(drift[0].1, "breadboard");
    assert_eq!(
        store::get_issue_by_key(&app.db, "BREAD-1")
            .await
            .unwrap()
            .title,
        "OSD renders on the wrong monitor",
        "breadboard's value is kept"
    );

    // 3. The webhook fast path never emits drift, even for an out-of-order
    //    (stale) webhook: that is not a missed webhook.
    let mut ooo = fg.clone();
    ooo.title = "another stale title".into();
    ooo.updated_at = (Utc::now() - Duration::days(3)).to_rfc3339();
    sync::apply_issue(&app.state, repo, &ooo, SyncSource::ForgejoWebhook)
        .await
        .unwrap();
    assert!(
        drain_drift(&mut rx).is_empty(),
        "webhook path stays silent on divergence"
    );
}

// --- PR close/merge target cascade regression tests ---

fn pr_issue(
    number: i64,
    title: &str,
    closed: bool,
    merged: bool,
    body: &str,
    updated: chrono::DateTime<Utc>,
) -> FgIssue {
    FgIssue {
        number,
        title: title.into(),
        body: body.into(),
        state: if closed {
            "closed".into()
        } else {
            "open".into()
        },
        labels: vec![],
        assignees: None,
        milestone: None,
        user: None,
        pull_request: Some(breadboard_server::forgejo::FgPullRequestStub { merged }),
        created_at: "2026-09-01T00:00:00Z".into(),
        updated_at: updated.to_rfc3339(),
    }
}

async fn ensure_repo_and_project(app: &common::TestApp, repo: &str, key: &str) {
    store::upsert_project_for_repo(&app.db, key, "test", repo)
        .await
        .unwrap();
    store::upsert_repo(&app.db, repo, true).await.unwrap();
}

/// Import a non-PR issue via the sync path to allocate a deterministic breadboard key
/// (first issue in a fresh project is KEY-1).
async fn import_target_issue(
    app: &common::TestApp,
    repo: &str,
    number: i64,
    title: &str,
    labels: &[&str],
) -> breadboard_core::Issue {
    let mut fg = fg_issue(number, title, Utc::now());
    fg.labels = labels.iter().map(|n| label(n)).collect();
    sync::apply_issue(&app.state, repo, &fg, SyncSource::ForgejoWebhook)
        .await
        .unwrap();
    // The first issue for this project will be <KEY>-1.
    // We don't know the project key here; caller passes it or we fetch.
    // For these tests we always use a fresh project per test so first target is KEY-1.
    // Return by looking up the just-imported number.
    store::find_issue_by_forgejo(&app.db, repo, number)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn closed_unmerged_pr_does_not_mark_targets_done() {
    let app = common::spawn().await;
    let repo = "Breadway/test";
    ensure_repo_and_project(&app, repo, "TST").await;

    // Create target first so it gets TST-1.
    let target = import_target_issue(&app, repo, 100, "needs verification", &[]).await;
    assert_eq!(target.key, "TST-1");

    // Closed-unmerged PR referencing it.
    let pr = pr_issue(
        10,
        "reject the thing",
        true,
        false,
        "Closes TST-1",
        Utc::now(),
    );
    sync::apply_issue(&app.state, repo, &pr, SyncSource::ForgejoWebhook)
        .await
        .unwrap();

    let t = store::get_issue_by_key(&app.db, "TST-1").await.unwrap();
    assert_ne!(
        t.status,
        Status::Done,
        "closed-unmerged must not cascade done"
    );
}

#[tokio::test]
async fn merged_pr_marks_targets_done() {
    let app = common::spawn().await;
    let repo = "Breadway/test";
    ensure_repo_and_project(&app, repo, "TST").await;

    let target = import_target_issue(&app, repo, 101, "landed work", &[]).await;
    assert_eq!(target.key, "TST-1");

    let pr = pr_issue(11, "land it", true, true, "Closes TST-1", Utc::now());
    sync::apply_issue(&app.state, repo, &pr, SyncSource::ForgejoWebhook)
        .await
        .unwrap();

    let t = store::get_issue_by_key(&app.db, "TST-1").await.unwrap();
    assert_eq!(t.status, Status::Done, "merged PR should cascade done");
}

#[tokio::test]
async fn merged_pr_with_verify_before_done_label_goes_to_review() {
    let app = common::spawn().await;
    let repo = "Breadway/test";
    ensure_repo_and_project(&app, repo, "TST").await;

    let target = import_target_issue(
        &app,
        repo,
        102,
        "needs live verify",
        &["verify-before-done"],
    )
    .await;
    assert_eq!(target.key, "TST-1");

    let pr = pr_issue(
        12,
        "land verified later",
        true,
        true,
        "Closes TST-1",
        Utc::now(),
    );
    sync::apply_issue(&app.state, repo, &pr, SyncSource::ForgejoWebhook)
        .await
        .unwrap();

    let t = store::get_issue_by_key(&app.db, "TST-1").await.unwrap();
    assert_eq!(
        t.status,
        Status::Review,
        "verify-before-done forces review on merge"
    );

    // Verification passed later: an explicit move to done must stick even
    // though the merged PR and the label are unchanged and keep syncing.
    store::update_issue(
        &app.db,
        "TST-1",
        &breadboard_core::dto::IssuePatch {
            status: Some(Status::Done),
            ..Default::default()
        },
        None,
        SyncSource::Breadboard,
    )
    .await
    .unwrap();
    sync::apply_issue(&app.state, repo, &pr, SyncSource::ForgejoWebhook)
        .await
        .unwrap();

    let t = store::get_issue_by_key(&app.db, "TST-1").await.unwrap();
    assert_eq!(
        t.status,
        Status::Done,
        "a verified done is not dragged back to review by later syncs"
    );
}

#[tokio::test]
async fn reopen_pr_does_not_cascade_done() {
    let app = common::spawn().await;
    let repo = "Breadway/test";
    ensure_repo_and_project(&app, repo, "TST").await;

    let target = import_target_issue(&app, repo, 103, "reopen test", &[]).await;
    assert_eq!(target.key, "TST-1");

    // Merge first.
    let pr_done = pr_issue(13, "temp merge", true, true, "Closes TST-1", Utc::now());
    sync::apply_issue(&app.state, repo, &pr_done, SyncSource::ForgejoWebhook)
        .await
        .unwrap();
    let t = store::get_issue_by_key(&app.db, "TST-1").await.unwrap();
    assert_eq!(t.status, Status::Done);

    // Reopen the PR (still references the target).
    let pr_open = pr_issue(
        13,
        "temp merge",
        false,
        false,
        "Closes TST-1",
        Utc::now() + Duration::seconds(2),
    );
    sync::apply_issue(&app.state, repo, &pr_open, SyncSource::ForgejoWebhook)
        .await
        .unwrap();

    let t2 = store::get_issue_by_key(&app.db, "TST-1").await.unwrap();
    // Reopen of PR must not touch the target status (remains done until explicit move).
    assert_eq!(t2.status, Status::Done);
}

/// A new plain issue imported by reconcile used to fail with SQLITE_BUSY
/// ("database is locked") under a concurrent writer: `import_issue` read the
/// cycle defaults before its first write, and a deferred WAL transaction cannot
/// upgrade its snapshot. Holding the write lock while the import runs proves it
/// now waits for the writer instead of failing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconcile_import_waits_for_a_busy_writer() {
    let app = common::spawn().await;
    let repo = "Breadway/test";
    ensure_repo_and_project(&app, repo, "TST").await;

    let mut fg = fg_issue(500, "imported under contention", Utc::now());
    fg.created_at = Utc::now().to_rfc3339();

    let holder = app.db.begin_with("BEGIN IMMEDIATE").await.unwrap();
    let state = app.state.clone();
    let import = tokio::spawn(async move {
        sync::apply_issue(&state, repo, &fg, SyncSource::ForgejoReconcile).await
    });
    // Let the import reach its first write while the lock is held.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    drop(holder);

    let result = tokio::time::timeout(std::time::Duration::from_secs(10), import)
        .await
        .expect("import waited out the busy writer")
        .expect("import task did not panic");
    assert!(
        result.is_ok(),
        "reconcile import failed under contention: {result:?}"
    );
    let imported = store::find_issue_by_forgejo(&app.db, repo, 500)
        .await
        .unwrap()
        .expect("issue imported despite the busy writer");
    assert_eq!(imported.title, "imported under contention");
}

/// A PR can arrive before the issue it closes. Unchanged reconcile passes used
/// to skip `sync_pr_closes` entirely, so the reverse `closed-by` link and the
/// closing cascade stayed missing until some field changed on the PR. The heal
/// pass must restore both without touching unrelated fields.
#[tokio::test]
async fn reconcile_heals_closing_links_when_target_arrives_after_pr() {
    let app = common::spawn().await;
    let repo = "Breadway/test";
    ensure_repo_and_project(&app, repo, "TST").await;

    // Arrival order 1: merged PR referencing a target breadboard has not seen.
    let pr = pr_issue(600, "land it", true, true, "Closes #501", Utc::now());
    sync::apply_issue(&app.state, repo, &pr, SyncSource::ForgejoWebhook)
        .await
        .unwrap();
    let pr_card = store::find_issue_by_forgejo(&app.db, repo, 600)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pr_card.issue_type, IssueType::Pr);

    // Arrival order 2: the missing target is imported afterwards.
    let target_fg = fg_issue(501, "the missing target", Utc::now());
    sync::apply_issue(&app.state, repo, &target_fg, SyncSource::ForgejoWebhook)
        .await
        .unwrap();
    let target = store::find_issue_by_forgejo(&app.db, repo, 501)
        .await
        .unwrap()
        .unwrap();
    assert!(
        target.links.is_empty(),
        "no closing link exists while the target is the newer arrival"
    );

    // Nothing changed on the PR; a reconcile pass must still heal the links
    // and the merged closing cascade idempotently.
    sync::apply_issue(&app.state, repo, &pr, SyncSource::ForgejoReconcile)
        .await
        .unwrap();
    sync::apply_issue(&app.state, repo, &pr, SyncSource::ForgejoReconcile)
        .await
        .unwrap();

    let target = store::find_issue_by_forgejo(&app.db, repo, 501)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        target.status,
        Status::Done,
        "merged PR completes the target"
    );
    assert!(target
        .links
        .iter()
        .any(|l| l.kind == LinkKind::ClosedBy && l.target_id == pr_card.id));
    let pr_card = store::get_issue_by_key(&app.db, &pr_card.key)
        .await
        .unwrap();
    assert!(pr_card
        .links
        .iter()
        .any(|l| l.kind == LinkKind::Closes && l.target_id == target.id));
}

/// Under a busy writer, the closing transition must apply its two links and
/// the status move together rather than leaving a linked but unclosed card.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contended_closing_transition_links_and_status_land_together() {
    let app = common::spawn().await;
    let repo = "Breadway/test";
    ensure_repo_and_project(&app, repo, "TST").await;

    let target = import_target_issue(&app, repo, 700, "close atomically", &[]).await;
    let pr = pr_issue(701, "closing pr", true, true, "no refs", Utc::now());
    sync::apply_issue(&app.state, repo, &pr, SyncSource::ForgejoWebhook)
        .await
        .unwrap();
    let pr_card = store::find_issue_by_forgejo(&app.db, repo, 701)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(target.status, Status::Done);

    let holder = app.db.begin_with("BEGIN IMMEDIATE").await.unwrap();
    let db = app.db.clone();
    let (pr_id, target_id) = (pr_card.id, target.id);
    let task = tokio::spawn(async move {
        store::sync_pr_close_target(
            &db,
            pr_id,
            target_id,
            Some(Status::Done),
            SyncSource::ForgejoReconcile,
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    drop(holder);

    let updated = tokio::time::timeout(std::time::Duration::from_secs(10), task)
        .await
        .expect("closing transition waited out the busy writer")
        .expect("task did not panic")
        .expect("closing transition succeeded")
        .expect("a status change was applied");
    assert_eq!(updated.status, Status::Done);

    let target = store::get_issue_by_key(&app.db, &target.key).await.unwrap();
    assert_eq!(target.status, Status::Done);
    assert!(
        target
            .links
            .iter()
            .any(|l| l.kind == LinkKind::ClosedBy && l.target_id == pr_id),
        "the closed-by link is present with the status change"
    );
    let pr_card = store::get_issue_by_key(&app.db, &pr_card.key)
        .await
        .unwrap();
    assert!(
        pr_card
            .links
            .iter()
            .any(|l| l.kind == LinkKind::Closes && l.target_id == target_id),
        "the closes link is present with the status change"
    );
}

/// A merged PR completes the target; a later abandoned (closed-unmerged) PR
/// that also references it must not revert the accepted completion to review.
#[tokio::test]
async fn abandoned_pr_does_not_revert_done_completed_by_merged_pr() {
    let app = common::spawn().await;
    let repo = "Breadway/test";
    ensure_repo_and_project(&app, repo, "TST").await;

    let target = import_target_issue(&app, repo, 900, "completed by a merged PR", &[]).await;
    assert_eq!(target.key, "TST-1");

    // PR A is merged and completes TST-1.
    let merged = pr_issue(
        910,
        "the accepted fix",
        true,
        true,
        "Closes TST-1",
        Utc::now(),
    );
    sync::apply_issue(&app.state, repo, &merged, SyncSource::ForgejoWebhook)
        .await
        .unwrap();
    assert_eq!(
        store::get_issue_by_key(&app.db, "TST-1")
            .await
            .unwrap()
            .status,
        Status::Done
    );

    // PR B is abandoned and closed unmerged later, also referencing TST-1.
    let abandoned = pr_issue(
        911,
        "the rejected attempt",
        true,
        false,
        "Closes TST-1",
        Utc::now() + Duration::seconds(5),
    );
    sync::apply_issue(&app.state, repo, &abandoned, SyncSource::ForgejoWebhook)
        .await
        .unwrap();

    let after = store::get_issue_by_key(&app.db, "TST-1").await.unwrap();
    assert_eq!(
        after.status,
        Status::Done,
        "an abandoned PR must not revert work completed by a merged PR"
    );
    let pr_b = store::find_issue_by_forgejo(&app.db, repo, 911)
        .await
        .unwrap()
        .unwrap();
    assert!(
        after
            .links
            .iter()
            .any(|l| l.kind == LinkKind::ClosedBy && l.target_id == pr_b.id),
        "the abandoned PR still records its closing link"
    );

    // A later title change on the abandoned PR must not revert it either.
    let mut retitled = abandoned.clone();
    retitled.title = "rejected attempt (retitled)".into();
    retitled.updated_at = (Utc::now() + Duration::seconds(10)).to_rfc3339();
    sync::apply_issue(&app.state, repo, &retitled, SyncSource::ForgejoReconcile)
        .await
        .unwrap();
    assert_eq!(
        store::get_issue_by_key(&app.db, "TST-1")
            .await
            .unwrap()
            .status,
        Status::Done
    );
}

/// The same rule without a merged PR: an abandoned PR never changes a target
/// that a human moved to done.
#[tokio::test]
async fn abandoned_pr_does_not_revert_a_manually_done_target() {
    let app = common::spawn().await;
    let repo = "Breadway/test";
    ensure_repo_and_project(&app, repo, "TST").await;

    let target = import_target_issue(&app, repo, 920, "manually completed", &[]).await;
    store::update_issue(
        &app.db,
        &target.key,
        &breadboard_core::dto::IssuePatch {
            status: Some(Status::Done),
            ..Default::default()
        },
        None,
        SyncSource::Breadboard,
    )
    .await
    .unwrap();

    let abandoned = pr_issue(
        921,
        "abandoned attempt",
        true,
        false,
        "Closes TST-1",
        Utc::now(),
    );
    sync::apply_issue(&app.state, repo, &abandoned, SyncSource::ForgejoWebhook)
        .await
        .unwrap();

    assert_eq!(
        store::get_issue_by_key(&app.db, "TST-1")
            .await
            .unwrap()
            .status,
        Status::Done,
        "only a merged PR's closing transition or an explicit reopen changes done"
    );
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
