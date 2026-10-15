//! REST handlers for projects, epics, cycles, and issues.
//!
//! Every mutation publishes a [`BoardEvent`] on the bus (→ SSE clients, and
//! milestone 7 → `bread.board.*`). Synced-field changes are logged to
//! `sync_log` by the store layer; Forgejo writeback lands in milestone 4.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use breadboard_core::{BoardEvent, Comment, Cycle, Epic, Issue, Project, SyncSource};
use serde::{Deserialize, Serialize};

use crate::auth::AuthUser;
use crate::dto::{
    CyclePatch, EpicPatch, IssueFilter, IssuePatch, MoveProject, NewCycle, NewEpic, NewIssue,
    NewProject, ProjectPatch,
};
use crate::error::{AppError, AppResult};
use crate::{notify, store, sync, AppState};

// --- projects --------------------------------------------------------------

pub async fn list_projects(
    State(app): State<AppState>,
    _user: AuthUser,
) -> AppResult<Json<Vec<Project>>> {
    Ok(Json(store::list_projects(&app.db).await?))
}

pub async fn get_project(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Project>> {
    Ok(Json(store::get_project(&app.db, id).await?))
}

pub async fn create_project(
    State(app): State<AppState>,
    _user: AuthUser,
    Json(body): Json<NewProject>,
) -> AppResult<(StatusCode, Json<Project>)> {
    let project = store::create_project(&app.db, &body).await?;
    Ok((StatusCode::CREATED, Json(project)))
}

pub async fn update_project(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(id): Path<i64>,
    Json(patch): Json<ProjectPatch>,
) -> AppResult<Json<Project>> {
    Ok(Json(store::update_project(&app.db, id, &patch).await?))
}

// --- epics ----------------------------------------------------------------

#[derive(Serialize)]
pub struct EpicView {
    #[serde(flatten)]
    epic: Epic,
    /// Child-issue counts, keyed by status.
    progress: std::collections::BTreeMap<String, i64>,
    total: i64,
}

async fn epic_view(app: &AppState, epic: Epic) -> AppResult<EpicView> {
    let counts = store::epic_progress(&app.db, epic.id).await?;
    let total = counts.iter().map(|(_, n)| n).sum();
    Ok(EpicView {
        epic,
        progress: counts.into_iter().collect(),
        total,
    })
}

pub async fn list_epics(
    State(app): State<AppState>,
    _user: AuthUser,
) -> AppResult<Json<Vec<EpicView>>> {
    let mut out = Vec::new();
    for e in store::list_epics(&app.db).await? {
        out.push(epic_view(&app, e).await?);
    }
    Ok(Json(out))
}

pub async fn get_epic(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<EpicView>> {
    Ok(Json(
        epic_view(&app, store::get_epic(&app.db, id).await?).await?,
    ))
}

pub async fn create_epic(
    State(app): State<AppState>,
    _user: AuthUser,
    Json(body): Json<NewEpic>,
) -> AppResult<(StatusCode, Json<Epic>)> {
    let epic = store::create_epic(&app.db, &body).await?;
    Ok((StatusCode::CREATED, Json(epic)))
}

pub async fn update_epic(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(id): Path<i64>,
    Json(patch): Json<EpicPatch>,
) -> AppResult<Json<Epic>> {
    Ok(Json(store::update_epic(&app.db, id, &patch).await?))
}

pub async fn delete_epic(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    store::delete_epic(&app.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- cycles -------------------------------------------------------------

pub async fn list_cycles(
    State(app): State<AppState>,
    _user: AuthUser,
) -> AppResult<Json<Vec<Cycle>>> {
    Ok(Json(store::list_cycles(&app.db).await?))
}

pub async fn create_cycle(
    State(app): State<AppState>,
    _user: AuthUser,
    Json(body): Json<NewCycle>,
) -> AppResult<(StatusCode, Json<Cycle>)> {
    let cycle = store::create_cycle(&app.db, &body).await?;
    Ok((StatusCode::CREATED, Json(cycle)))
}

pub async fn update_cycle(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<CyclePatch>,
) -> AppResult<Json<Cycle>> {
    let cycle = match body.project_ids {
        Some(ids) => store::update_cycle_projects(&app.db, id, &ids).await?,
        None => store::get_cycle(&app.db, id).await?,
    };
    Ok(Json(cycle))
}

pub async fn start_cycle(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Cycle>> {
    let cycle = store::set_cycle_closed(&app.db, id, false).await?;
    let issues = store::count_cycle_issues(&app.db, id).await?;
    app.bus.publish(BoardEvent::CycleStarted {
        id: cycle.id,
        name: cycle.name.clone(),
        issues,
    });
    notify::emit_bread(
        &app,
        &BoardEvent::CycleStarted {
            id: cycle.id,
            name: cycle.name.clone(),
            issues,
        },
    );
    notify::cycle(&app, cycle.id, &cycle.name, true).await;
    Ok(Json(cycle))
}

pub async fn end_cycle(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Cycle>> {
    let cycle = store::set_cycle_closed(&app.db, id, true).await?;
    app.bus.publish(BoardEvent::CycleEnded {
        id: cycle.id,
        name: cycle.name.clone(),
    });
    notify::emit_bread(
        &app,
        &BoardEvent::CycleEnded {
            id: cycle.id,
            name: cycle.name.clone(),
        },
    );
    notify::cycle(&app, cycle.id, &cycle.name, false).await;
    Ok(Json(cycle))
}

// --- issues -----------------------------------------------------------

#[derive(Serialize)]
pub struct IssueDetail {
    #[serde(flatten)]
    issue: Issue,
    comments: Vec<Comment>,
}

pub async fn list_issues(
    State(app): State<AppState>,
    user: AuthUser,
    Query(mut filter): Query<IssueFilter>,
) -> AppResult<Json<Vec<Issue>>> {
    if filter.mine {
        filter.assignee_id = Some(user.0.id);
    }
    Ok(Json(store::list_issues(&app.db, &filter).await?))
}

pub async fn create_issue(
    State(app): State<AppState>,
    user: AuthUser,
    Json(body): Json<NewIssue>,
) -> AppResult<(StatusCode, Json<Issue>)> {
    let issue = store::create_issue(&app.db, &body, Some(user.0.id)).await?;
    // Push to Forgejo when the project maps to an enabled repo (no-op otherwise).
    let issue = sync::writeback_new_issue(&app, issue).await;
    app.bus.publish(BoardEvent::CardCreated {
        id: issue.id,
        key: issue.key.clone(),
    });
    if let Some(assignee_id) = issue.assignee_id {
        app.bus.publish(BoardEvent::CardAssigned {
            id: issue.id,
            key: issue.key.clone(),
            assignee_id: Some(assignee_id),
        });
        notify::assigned(&app, &issue, assignee_id, Some(user.0.id)).await;
    }
    notify::emit_bread(
        &app,
        &BoardEvent::CardCreated {
            id: issue.id,
            key: issue.key.clone(),
        },
    );
    if !issue.body.is_empty() {
        notify::mentions(
            &app,
            &issue,
            &issue.body,
            Some(user.0.id),
            "the description",
        )
        .await;
    }
    Ok((StatusCode::CREATED, Json(issue)))
}

pub async fn get_issue(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(key): Path<String>,
) -> AppResult<Json<IssueDetail>> {
    let issue = store::get_issue_by_key(&app.db, &key).await?;
    let comments = store::list_comments(&app.db, issue.id).await?;
    Ok(Json(IssueDetail { issue, comments }))
}

#[derive(Deserialize)]
pub struct NewComment {
    body: String,
}

pub async fn add_comment(
    State(app): State<AppState>,
    user: AuthUser,
    Path(key): Path<String>,
    Json(body): Json<NewComment>,
) -> AppResult<(StatusCode, Json<Comment>)> {
    if body.body.trim().is_empty() {
        return Err(crate::error::AppError::bad_request("comment body is empty"));
    }
    let issue = store::get_issue_by_key(&app.db, &key).await?;
    let comment =
        store::create_comment(&app.db, issue.id, Some(user.0.id), body.body.trim()).await?;
    app.bus.publish(BoardEvent::CardUpdated {
        id: issue.id,
        key: issue.key.clone(),
    });
    notify::mentions(&app, &issue, &comment.body, Some(user.0.id), "a comment").await;
    sync::writeback_comment(&app, &issue, comment.id, &comment.body).await;
    // Re-read so the response carries the external_id if writeback set one.
    let stored = store::list_comments(&app.db, issue.id)
        .await?
        .into_iter()
        .find(|c| c.id == comment.id)
        .unwrap_or(comment);
    Ok((StatusCode::CREATED, Json(stored)))
}

pub async fn issue_activity(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(key): Path<String>,
) -> AppResult<Json<Vec<store::ActivityEntry>>> {
    let issue = store::get_issue_by_key(&app.db, &key).await?;
    Ok(Json(store::issue_activity(&app.db, issue.id).await?))
}

pub async fn patch_issue(
    State(app): State<AppState>,
    user: AuthUser,
    Path(key): Path<String>,
    Json(patch): Json<IssuePatch>,
) -> AppResult<Json<Issue>> {
    let before = store::get_issue_by_key(&app.db, &key).await?;
    let issue = store::update_issue(
        &app.db,
        &key,
        &patch,
        Some(user.0.id),
        SyncSource::Breadboard,
    )
    .await?;

    if issue.updated_at != before.updated_at {
        app.bus.publish(BoardEvent::CardUpdated {
            id: issue.id,
            key: issue.key.clone(),
        });
    }
    if issue.status != before.status {
        app.bus.publish(BoardEvent::CardMoved {
            id: issue.id,
            key: issue.key.clone(),
            from: before.status,
            to: issue.status,
        });
    }
    if issue.assignee_id != before.assignee_id {
        app.bus.publish(BoardEvent::CardAssigned {
            id: issue.id,
            key: issue.key.clone(),
            assignee_id: issue.assignee_id,
        });
        if let Some(a) = issue.assignee_id {
            notify::assigned(&app, &issue, a, Some(user.0.id)).await;
        }
    }
    if issue.updated_at != before.updated_at {
        notify::emit_bread(
            &app,
            &BoardEvent::CardUpdated {
                id: issue.id,
                key: issue.key.clone(),
            },
        );
    }
    if patch.body.is_some() && issue.body != before.body {
        notify::mentions(
            &app,
            &issue,
            &issue.body,
            Some(user.0.id),
            "the description",
        )
        .await;
    }

    // Push synced-field changes back to Forgejo (no-op if not linked/enabled).
    sync::writeback(&app, &before, &issue).await;

    Ok(Json(issue))
}

pub async fn move_issue_project(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(key): Path<String>,
    Json(body): Json<MoveProject>,
) -> AppResult<Json<Issue>> {
    let project = store::find_project(&app.db, &body.project).await?;
    let issue = sync::move_project(&app, &key, &project).await?;
    app.bus.publish(BoardEvent::CardUpdated {
        id: issue.id,
        key: issue.key.clone(),
    });
    Ok(Json(issue))
}

pub async fn delete_issue(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(key): Path<String>,
) -> AppResult<StatusCode> {
    let issue = store::get_issue_by_key(&app.db, &key).await?;
    store::delete_issue(&app.db, &key).await?;
    app.bus.publish(BoardEvent::CardDeleted {
        id: issue.id,
        key: issue.key,
    });
    Ok(StatusCode::NO_CONTENT)
}

// --- synced repos ---------------------------------------------------------

#[derive(Serialize)]
pub struct RepoView {
    repo: String,
    project_key: Option<String>,
    enabled: bool,
    last_synced_at: Option<String>,
}

pub async fn list_repos(
    State(app): State<AppState>,
    _user: AuthUser,
) -> AppResult<Json<Vec<RepoView>>> {
    let rows = store::list_repos(&app.db).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| RepoView {
                repo: r.repo,
                project_key: r.project_key,
                enabled: r.enabled,
                last_synced_at: r.last_synced_at,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
pub struct AddRepo {
    /// `owner/name` on the configured Forgejo instance.
    repo: String,
    /// Project key to file this repo's issues under (created if absent).
    key: String,
    #[serde(default)]
    name: Option<String>,
}

pub async fn add_repo(
    State(app): State<AppState>,
    _user: AuthUser,
    Json(body): Json<AddRepo>,
) -> AppResult<(StatusCode, Json<RepoView>)> {
    if crate::forgejo::split_repo(&body.repo).is_err() {
        return Err(AppError::bad_request("repo must be `owner/name`"));
    }
    let name = body.name.unwrap_or_else(|| {
        body.repo
            .split('/')
            .next_back()
            .unwrap_or(&body.repo)
            .to_string()
    });
    let project = store::upsert_project_for_repo(&app.db, &body.key, &name, &body.repo).await?;
    store::upsert_repo(&app.db, &body.repo, true).await?;

    // Kick an initial import in the background.
    let state = app.clone();
    let repo = body.repo.clone();
    tokio::spawn(async move {
        if let Err(e) = sync::reconcile_once(&state).await {
            tracing::warn!(%repo, error = %e, "initial repo import failed");
        }
    });

    Ok((
        StatusCode::CREATED,
        Json(RepoView {
            repo: body.repo,
            project_key: Some(project.key),
            enabled: true,
            last_synced_at: None,
        }),
    ))
}

pub async fn remove_repo(
    State(app): State<AppState>,
    _user: AuthUser,
    Path((owner, name)): Path<(String, String)>,
) -> AppResult<StatusCode> {
    let repo = format!("{owner}/{name}");
    store::upsert_repo(&app.db, &repo, false).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn reconcile_now(State(app): State<AppState>, _user: AuthUser) -> AppResult<StatusCode> {
    tokio::spawn(async move {
        if let Err(e) = sync::reconcile_once(&app).await {
            tracing::warn!(error = %e, "manual reconcile failed");
        }
    });
    Ok(StatusCode::ACCEPTED)
}

// --- notifications ------------------------------------------------------

#[derive(Deserialize)]
pub struct NotifQuery {
    #[serde(default)]
    unread: bool,
}

#[derive(Serialize)]
pub struct Inbox {
    unread: i64,
    notifications: Vec<breadboard_core::Notification>,
}

pub async fn list_notifications(
    State(app): State<AppState>,
    user: AuthUser,
    Query(q): Query<NotifQuery>,
) -> AppResult<Json<Inbox>> {
    Ok(Json(Inbox {
        unread: store::unread_count(&app.db, user.0.id).await?,
        notifications: store::list_notifications(&app.db, user.0.id, q.unread).await?,
    }))
}

pub async fn mark_notification_read(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    store::mark_notification_read(&app.db, user.0.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn mark_all_notifications_read(
    State(app): State<AppState>,
    user: AuthUser,
) -> AppResult<StatusCode> {
    store::mark_all_read(&app.db, user.0.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn import_markdown(
    State(app): State<AppState>,
    _user: AuthUser,
    Json(req): Json<crate::import::ImportRequest>,
) -> AppResult<Json<crate::import::ImportResult>> {
    Ok(Json(crate::import::run(&app, &req).await?))
}

pub async fn get_notification_prefs(
    State(app): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<store::NotificationPrefs>> {
    Ok(Json(store::get_prefs(&app.db, user.0.id).await?))
}

pub async fn set_notification_prefs(
    State(app): State<AppState>,
    user: AuthUser,
    Json(prefs): Json<store::NotificationPrefs>,
) -> AppResult<Json<store::NotificationPrefs>> {
    Ok(Json(store::set_prefs(&app.db, user.0.id, prefs).await?))
}
