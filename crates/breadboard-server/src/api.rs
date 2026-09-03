//! REST handlers for projects, epics, cycles, and issues.
//!
//! Every mutation publishes a [`BoardEvent`] on the bus (→ SSE clients, and
//! milestone 7 → `bread.board.*`). Synced-field changes are logged to
//! `sync_log` by the store layer; Forgejo writeback lands in milestone 4.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use breadboard_core::{BoardEvent, Comment, Cycle, Epic, Issue, Project, SyncSource};
use serde::Serialize;

use crate::auth::AuthUser;
use crate::dto::{IssueFilter, IssuePatch, NewCycle, NewEpic, NewIssue, NewProject};
use crate::error::AppResult;
use crate::{store, AppState};

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

// --- epics ----------------------------------------------------------------

pub async fn list_epics(
    State(app): State<AppState>,
    _user: AuthUser,
) -> AppResult<Json<Vec<Epic>>> {
    Ok(Json(store::list_epics(&app.db).await?))
}

pub async fn get_epic(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Epic>> {
    Ok(Json(store::get_epic(&app.db, id).await?))
}

pub async fn create_epic(
    State(app): State<AppState>,
    _user: AuthUser,
    Json(body): Json<NewEpic>,
) -> AppResult<(StatusCode, Json<Epic>)> {
    let epic = store::create_epic(&app.db, &body).await?;
    Ok((StatusCode::CREATED, Json(epic)))
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
    }
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
