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
    EpicPatch, IssueFilter, IssuePatch, MoveProject, NewCycle, NewEpic, NewIssue, NewProject,
    ProjectPatch,
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
    Ok(Json(epic_view(&app, store::get_epic(&app.db, id).await?).await?))
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
    notify::emit_bread(&app, &BoardEvent::CycleStarted { id: cycle.id, name: cycle.name.clone(), issues });
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
    notify::emit_bread(&app, &BoardEvent::CycleEnded { id: cycle.id, name: cycle.name.clone() });
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
    notify::emit_bread(&app, &BoardEvent::CardCreated { id: issue.id, key: issue.key.clone() });
    if !issue.body.is_empty() {
        notify::mentions(&app, &issue, &issue.body, Some(user.0.id), "the description").await;
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
    let comment = store::create_comment(&app.db, issue.id, Some(user.0.id), body.body.trim()).await?;
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
            &BoardEvent::CardUpdated { id: issue.id, key: issue.key.clone() },
        );
    }
    if patch.body.is_some() && issue.body != before.body {
        notify::mentions(&app, &issue, &issue.body, Some(user.0.id), "the description").await;
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
        body.repo.split('/').next_back().unwrap_or(&body.repo).to_string()
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

pub async fn reconcile_now(
    State(app): State<AppState>,
    _user: AuthUser,
) -> AppResult<StatusCode> {
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

// --- agent claims -----------------------------------------------------------
use breadboard_core::{AgentState, ClaimMode};

/// Stale threshold for the live signal: 5 minutes without a heartbeat.
pub const CLAIM_STALE_SECS: i64 = 300;

fn claim_is_stale(claim: &breadboard_core::AgentClaim) -> bool {
    (chrono::Utc::now() - claim.last_heartbeat).num_seconds() >= CLAIM_STALE_SECS
}

/// Counts for exclusive claims only: an intent is a reservation, not work.
fn exclusive(claims: &[breadboard_core::AgentClaim]) -> Vec<&breadboard_core::AgentClaim> {
    claims.iter().filter(|c| c.mode == ClaimMode::Exclusive).collect()
}

fn agent_line(claims: &[breadboard_core::AgentClaim]) -> String {
    let work = exclusive(claims);
    let queued = claims.len() - work.len();
    let mut done = 0i64;
    let mut active = 0i64;
    let mut review = 0i64;
    let mut verifying = 0i64;
    let mut blocked = 0i64;
    for c in &work {
        match c.state {
            AgentState::Done => done += 1,
            AgentState::Implementing | AgentState::Reworking | AgentState::Planned => active += 1,
            AgentState::AwaitingReview | AgentState::AwaitingCi | AgentState::AwaitingMerge => {
                review += 1
            }
            AgentState::Verifying => verifying += 1,
            AgentState::Blocked | AgentState::NeedsUser => blocked += 1,
        }
    }
    let total = work.len() as i64;
    let mut parts = vec![format!("BB {done}/{total} done")];
    if active > 0 {
        parts.push(format!("{active} active"));
    }
    if review > 0 {
        parts.push(format!("{review} review/CI/merge"));
    }
    if verifying > 0 {
        parts.push(format!("{verifying} verifying"));
    }
    if blocked > 0 {
        parts.push(format!("{blocked} blocked"));
    }
    if queued > 0 {
        parts.push(format!("{queued} queued"));
    }
    parts.push(format!("{} remaining", total - done));
    parts.join(", ")
}

pub async fn list_agents(
    State(app): State<AppState>,
    _user: AuthUser,
) -> AppResult<Json<Vec<breadboard_core::AgentClaim>>> {
    Ok(Json(store::list_claims(&app.db).await?))
}

pub async fn agent_status(
    State(app): State<AppState>,
    _user: AuthUser,
) -> AppResult<Json<breadboard_core::dto::AgentStatusSummary>> {
    let claims = store::list_claims(&app.db).await?;
    let work = exclusive(&claims);
    let queued = (claims.len() - work.len()) as i64;
    let stale = work.iter().filter(|c| claim_is_stale(c)).count() as i64;
    let live = work.len() as i64 - stale;
    let line = agent_line(&claims);
    let mut done = 0i64;
    let mut active = 0i64;
    let mut review = 0i64;
    let mut verifying = 0i64;
    let mut blocked = 0i64;
    for c in &work {
        match c.state {
            AgentState::Done => done += 1,
            AgentState::Implementing | AgentState::Reworking | AgentState::Planned => active += 1,
            AgentState::AwaitingReview | AgentState::AwaitingCi | AgentState::AwaitingMerge => {
                review += 1
            }
            AgentState::Verifying => verifying += 1,
            AgentState::Blocked | AgentState::NeedsUser => blocked += 1,
        }
    }
    let total = work.len() as i64;
    Ok(Json(breadboard_core::dto::AgentStatusSummary {
        total,
        live,
        stale,
        queued,
        done,
        active,
        review,
        verifying,
        blocked,
        remaining: total - done,
        line,
        claims,
    }))
}

/// Claim an issue. `exclusive` (default) is first-wins and moves a
/// backlog/todo card to `doing`, assigning it to the claimer. `intent` is a
/// soft reservation: it never blocks another agent and never moves the card.
pub async fn claim_issue(
    State(app): State<AppState>,
    user: AuthUser,
    Path(key): Path<String>,
    Json(body): Json<breadboard_core::dto::ClaimUpsert>,
) -> AppResult<(StatusCode, Json<breadboard_core::AgentClaim>)> {
    let issue = store::get_issue_by_key(&app.db, &key).await?;
    if issue.issue_type == breadboard_core::IssueType::Pr {
        return Err(crate::error::AppError::bad_request(
            "cannot claim a PR card",
        ));
    }
    let mode = body.mode.unwrap_or(ClaimMode::Exclusive);
    let claim = store::insert_claim(&app.db, &issue, user.0.id, &body).await?;
    let audit = store::log_claim_event(
        &app.db,
        issue.id,
        "agent_claim",
        None,
        Some(&format!("{} ({})", claim.agent_name, claim.mode.as_str())),
        Some(user.0.id),
    )
    .await;
    if let Err(e) = audit {
        tracing::warn!(error = %e, key = %key, "claim audit write failed");
    }

    // Side-effect only for exclusive work: backlog/todo → doing + assign.
    if mode == ClaimMode::Exclusive
        && (matches!(
            issue.status,
            breadboard_core::Status::Backlog | breadboard_core::Status::Todo
        ) || issue.assignee_id.is_none())
    {
        let patch = breadboard_core::dto::IssuePatch {
            status: if matches!(
                issue.status,
                breadboard_core::Status::Backlog | breadboard_core::Status::Todo
            ) {
                Some(breadboard_core::Status::Doing)
            } else {
                None
            },
            assignee_id: if issue.assignee_id.is_none() {
                Some(Some(user.0.id))
            } else {
                None
            },
            ..Default::default()
        };
        if !patch.is_empty() {
            let before = issue.clone();
            let updated = store::update_issue(
                &app.db,
                &key,
                &patch,
                Some(user.0.id),
                SyncSource::Breadboard,
            )
            .await?;
            if updated.status != before.status {
                app.bus.publish(BoardEvent::CardMoved {
                    id: updated.id,
                    key: updated.key.clone(),
                    from: before.status,
                    to: updated.status,
                });
            }
            if updated.assignee_id != before.assignee_id {
                app.bus.publish(BoardEvent::CardAssigned {
                    id: updated.id,
                    key: updated.key.clone(),
                    assignee_id: updated.assignee_id,
                });
            }
            store::refresh_claim_board_status(&app.db, issue.id, updated.status).await?;
            sync::writeback(&app, &before, &updated).await;
        }
    }
    let claim = store::get_claim_by_id(&app.db, claim.id)
        .await?
        .unwrap_or(claim);
    app.bus.publish(BoardEvent::AgentClaimed {
        id: claim.issue_id,
        key: claim.key.clone(),
        agent: claim.agent_name.clone(),
        mode: claim.mode.as_str().to_string(),
        session_id: claim.session_id.clone(),
        state: claim.state.as_str().to_string(),
    });
    notify::emit_bread(
        &app,
        &BoardEvent::AgentClaimed {
            id: claim.issue_id,
            key: claim.key.clone(),
            agent: claim.agent_name.clone(),
            mode: claim.mode.as_str().to_string(),
            session_id: claim.session_id.clone(),
            state: claim.state.as_str().to_string(),
        },
    );
    Ok((StatusCode::CREATED, Json(claim)))
}

/// Pick the caller's claim on this issue. `agent` (query, optional) and
/// `session_id` (body, optional) disambiguate when one user owns several.
async fn resolve_own_claim(
    app: &AppState,
    issue_id: i64,
    user_id: i64,
    agent: Option<&str>,
    session_id: Option<&str>,
) -> AppResult<breadboard_core::AgentClaim> {
    let mut claims = store::find_user_claims(&app.db, issue_id, user_id, agent).await?;
    if claims.len() > 1 {
        if let Some(session) = session_id {
            claims.retain(|c| c.session_id.as_deref() == Some(session));
        }
    }
    match claims.len() {
        0 => Err(crate::error::AppError::NotFound),
        1 => Ok(claims.remove(0)),
        _ => Err(crate::error::AppError::Conflict(
            "this user has several claims on the issue: pass agent_name or session_id".into(),
        )),
    }
}

#[derive(Deserialize)]
pub struct ClaimQuery {
    /// Agent name, when the caller owns more than one claim on the issue.
    #[serde(default)]
    pub agent: Option<String>,
}

/// Heartbeat / field update. Only the claiming user may update; when both
/// sides carry a `session_id` they must match (prevents a second agent with
/// the same login from hijacking the claim). Setting a new `pr_url` also
/// comments the PR link on the issue so it syncs to Forgejo.
pub async fn heartbeat_claim(
    State(app): State<AppState>,
    user: AuthUser,
    Path(key): Path<String>,
    Query(q): Query<ClaimQuery>,
    Json(body): Json<breadboard_core::dto::ClaimUpdate>,
) -> AppResult<Json<breadboard_core::AgentClaim>> {
    let issue = store::get_issue_by_key(&app.db, &key).await?;
    let session = body
        .session_id
        .as_ref()
        .and_then(|v| v.as_deref());
    let existing = resolve_own_claim(&app, issue.id, user.0.id, q.agent.as_deref(), session).await?;
    if let (Some(Some(want_inner)), Some(have)) = (&body.session_id, &existing.session_id) {
        // Outer Some = field present, inner Some = set (not clear). A present
        // session that disagrees with the active claim is a 409, not a hijack.
        if want_inner.as_str() != have {
            return Err(crate::error::AppError::Conflict(
                "session_id does not match the active claim".into(),
            ));
        }
    }
    let claim = store::update_claim(&app.db, existing.id, &body).await?;

    // Audit the meaningful transitions, not every heartbeat.
    if existing.state != claim.state {
        let _ = store::log_claim_event(
            &app.db,
            issue.id,
            "agent_state",
            Some(existing.state.as_str()),
            Some(claim.state.as_str()),
            Some(user.0.id),
        )
        .await;
    }
    if claim.pr_url.is_some() && claim.pr_url != existing.pr_url {
        let url = claim.pr_url.clone().unwrap_or_default();
        let _ = store::log_claim_event(
            &app.db,
            issue.id,
            "agent_pr",
            existing.pr_url.as_deref(),
            Some(&url),
            Some(user.0.id),
        )
        .await;
        // Comment once per URL so the PR link reaches the Forgejo thread
        // without spamming it on every heartbeat.
        let already = store::list_comments(&app.db, issue.id)
            .await
            .unwrap_or_default()
            .iter()
            .any(|c| c.body.contains(&url));
        if !already {
            if let Ok(comment) = store::create_comment(
                &app.db,
                issue.id,
                Some(user.0.id),
                &format!("Pull request: {url}"),
            )
            .await
            {
                sync::writeback_comment(&app, &issue, comment.id, &comment.body).await;
            }
        }
    }

    app.bus.publish(BoardEvent::AgentUpdated {
        id: claim.issue_id,
        key: claim.key.clone(),
        agent: claim.agent_name.clone(),
        mode: claim.mode.as_str().to_string(),
        session_id: claim.session_id.clone(),
        state: claim.state.as_str().to_string(),
    });
    Ok(Json(claim))
}

/// Release the caller's claim on an issue. `?agent=` disambiguates when the
/// caller holds more than one. The card keeps its status; only the tint goes
/// away. Intents are released the same way.
pub async fn release_claim(
    State(app): State<AppState>,
    user: AuthUser,
    Path(key): Path<String>,
    Query(q): Query<ClaimQuery>,
) -> AppResult<StatusCode> {
    let issue = store::get_issue_by_key(&app.db, &key).await?;
    let claim = resolve_own_claim(&app, issue.id, user.0.id, q.agent.as_deref(), None).await?;
    store::delete_claim_by_id(&app.db, claim.id).await?;
    let _ = store::log_claim_event(
        &app.db,
        issue.id,
        "agent_release",
        Some(&format!("{} ({})", claim.agent_name, claim.mode.as_str())),
        None,
        Some(user.0.id),
    )
    .await;
    app.bus.publish(BoardEvent::AgentReleased {
        id: issue.id,
        key: issue.key.clone(),
    });
    notify::emit_bread(
        &app,
        &BoardEvent::AgentReleased {
            id: issue.id,
            key: issue.key.clone(),
        },
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Release one claim by id (the Agents page uses this so each row is
/// independently releasable). Only the owner may release.
pub async fn release_claim_by_id(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let claim = store::get_claim_by_id(&app.db, id)
        .await?
        .ok_or(crate::error::AppError::NotFound)?;
    if claim.claimed_by != user.0.id {
        return Err(crate::error::AppError::forbidden());
    }
    store::delete_claim_by_id(&app.db, id).await?;
    let _ = store::log_claim_event(
        &app.db,
        claim.issue_id,
        "agent_release",
        Some(&format!("{} ({})", claim.agent_name, claim.mode.as_str())),
        None,
        Some(user.0.id),
    )
    .await;
    app.bus.publish(BoardEvent::AgentReleased {
        id: claim.issue_id,
        key: claim.key.clone(),
    });
    notify::emit_bread(
        &app,
        &BoardEvent::AgentReleased {
            id: claim.issue_id,
            key: claim.key.clone(),
        },
    );
    Ok(StatusCode::NO_CONTENT)
}
