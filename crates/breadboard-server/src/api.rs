//! REST handlers for projects, epics, cycles, and issues.
//!
//! Every mutation publishes a [`BoardEvent`] on the bus (→ SSE clients, and
//! milestone 7 → `bread.board.*`). Synced-field changes are logged to
//! `sync_log` by the store layer; Forgejo writeback lands in milestone 4.

use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::http::{header, HeaderValue};
use axum::response::Response;
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

#[derive(Serialize)]
pub struct ReconcileResponse {
    ok: bool,
}

pub async fn reconcile_now(
    State(app): State<AppState>,
    _user: AuthUser,
) -> AppResult<(StatusCode, Json<ReconcileResponse>)> {
    tokio::spawn(async move {
        if let Err(e) = sync::reconcile_once(&app).await {
            tracing::warn!(error = %e, "manual reconcile failed");
        }
    });
    Ok((StatusCode::ACCEPTED, Json(ReconcileResponse { ok: true })))
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
        // Completion follows the issue's board status: a card in Done is done,
        // even if the agent has not heartbeated its lifecycle state yet. This
        // keeps the Agents tiles and the x/y done count saying the same thing.
        if c.board_status == breadboard_core::Status::Done {
            done += 1;
            continue;
        }
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

/// Set or clear the signed-in agent's one-liner. The name is normalised the
/// same way claims are, so it lands on the same profile the Agents tab groups.

// --- chat -------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ChatQuery {
    /// Act as this agent (must belong to the caller) instead of the human.
    #[serde(default)]
    pub agent: Option<String>,
    /// Return only messages with id greater than this.
    #[serde(default)]
    pub after: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// Resolve who is speaking: an owned agent, or the signed-in human.
async fn chat_author(
    app: &AppState,
    user: &AuthUser,
    agent_name: Option<&str>,
) -> AppResult<(String, String, breadboard_core::ChatAuthorKind)> {
    match agent_name.map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => {
            if !store::user_has_agent(&app.db, user.0.id, name).await? {
                return Err(crate::error::AppError::forbidden());
            }
            Ok((
                store::agent_member(name),
                name.trim().to_lowercase(),
                breadboard_core::ChatAuthorKind::Agent,
            ))
        }
        None => Ok((
            store::human_member(&user.0.login),
            user.0.login.clone(),
            breadboard_core::ChatAuthorKind::Human,
        )),
    }
}

async fn resolve_chat_room(app: &AppState, room: Option<&str>) -> AppResult<breadboard_core::ChatRoom> {
    match room.map(str::trim).filter(|s| !s.is_empty()) {
        None => store::chat_room_by_name(&app.db, "lobby")
            .await?
            .ok_or(crate::error::AppError::NotFound),
        Some(id) if id.parse::<i64>().is_ok() => {
            store::get_chat_room(&app.db, id.parse().unwrap()).await
        }
        Some(name) => store::chat_room_by_name(&app.db, name)
            .await?
            .ok_or(crate::error::AppError::NotFound),
    }
}

pub async fn list_chat_rooms(
    State(app): State<AppState>,
    user: AuthUser,
    Query(q): Query<ChatQuery>,
) -> AppResult<Json<Vec<breadboard_core::ChatRoom>>> {
    let (viewer, reader) = match q.agent.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => {
            if !store::user_has_agent(&app.db, user.0.id, name).await? {
                return Err(crate::error::AppError::forbidden());
            }
            let member = store::agent_member(name);
            (Some(member.clone()), Some(member))
        }
        None => (None, Some(store::human_member(&user.0.login))),
    };
    Ok(Json(
        store::chat_rooms(&app.db, viewer.as_deref(), reader.as_deref()).await?,
    ))
}

pub async fn create_chat_room(
    State(app): State<AppState>,
    _user: AuthUser,
    Json(body): Json<breadboard_core::dto::NewChatRoom>,
) -> AppResult<(StatusCode, Json<breadboard_core::ChatRoom>)> {
    let name = breadboard_core::dto::normalize_room_name(&body.name)
        .map_err(crate::error::AppError::BadRequest)?;
    let description = body
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());
    if let Some(d) = description {
        if d.chars().count() > 160 {
            return Err(crate::error::AppError::bad_request(
                "description must be 160 characters or fewer",
            ));
        }
    }
    let room = store::create_chat_room(&app.db, &name, description).await?;
    Ok((StatusCode::CREATED, Json(room)))
}

pub async fn chat_messages(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Query(q): Query<ChatQuery>,
) -> AppResult<Json<Vec<breadboard_core::ChatMessage>>> {
    let room = store::get_chat_room(&app.db, id).await?;
    if let Some(name) = q.agent.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if !store::user_has_agent(&app.db, user.0.id, name).await? {
            return Err(crate::error::AppError::forbidden());
        }
        if room.kind == breadboard_core::ChatRoomKind::Dm
            && !store::chat_is_member(&app.db, room.id, &store::agent_member(name)).await?
        {
            return Err(crate::error::AppError::forbidden());
        }
    }
    let reader = match q.agent.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => store::agent_member(name),
        None => store::human_member(&user.0.login),
    };
    let messages = store::chat_messages(
        &app.db,
        room.id,
        q.after.unwrap_or(0),
        q.limit.unwrap_or(200).clamp(1, 500),
    )
    .await?;
    if let Some(last) = messages.last() {
        store::chat_mark_read(&app.db, room.id, &reader, last.id).await?;
    }
    Ok(Json(messages))
}

pub async fn open_dm(
    State(app): State<AppState>,
    user: AuthUser,
    Json(body): Json<breadboard_core::dto::OpenDm>,
) -> AppResult<Json<breadboard_core::ChatRoom>> {
    let (author, _, _) =
        chat_author(&app, &user, body.agent_name.as_deref()).await?;
    let peer = store::agent_member(&body.with);
    if peer == author {
        return Err(crate::error::AppError::bad_request(
            "cannot open a DM with yourself",
        ));
    }
    if peer == store::agent_member("") {
        return Err(crate::error::AppError::bad_request("agent name is required"));
    }
    Ok(Json(store::find_or_create_dm(&app.db, &author, &peer).await?))
}


// --- chat uploads -----------------------------------------------------------

const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

fn safe_display_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("file");
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control() && *c != '"')
        .take(120)
        .collect();
    if cleaned.trim().is_empty() {
        "file".into()
    } else {
        cleaned
    }
}

fn guess_mime(filename: &str, provided: Option<&str>) -> String {
    if let Some(m) = provided.map(str::trim).filter(|m| !m.is_empty() && *m != "application/octet-stream") {
        return m.to_string();
    }
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "json" => "application/json",
        "zip" => "application/zip",
        "log" => "text/plain",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn storage_ext(filename: &str) -> String {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    if !ext.is_empty() && ext.len() <= 10 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        ext
    } else {
        "bin".into()
    }
}

async fn save_upload(
    app: &AppState,
    user_id: i64,
    filename: &str,
    mime: Option<&str>,
    bytes: &[u8],
) -> AppResult<Json<breadboard_core::ChatUpload>> {
    if bytes.is_empty() {
        return Err(AppError::bad_request("file is empty"));
    }
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(AppError::bad_request("file is larger than 10 MB"));
    }
    let display = safe_display_name(filename);
    let mime = guess_mime(&display, mime);
    let stored = format!("{}.{}", crate::auth::random_token(), storage_ext(&display));
    let dir = app.config.uploads_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("creating upload dir: {e}")))?;
    tokio::fs::write(dir.join(&stored), bytes)
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("writing upload: {e}")))?;
    let upload = store::insert_chat_upload(
        &app.db,
        user_id,
        &display,
        &mime,
        bytes.len() as i64,
        &stored,
    )
    .await?;
    Ok(Json(upload))
}

pub async fn upload_chat_file(
    State(app): State<AppState>,
    user: AuthUser,
    mut multipart: Multipart,
) -> AppResult<(StatusCode, Json<breadboard_core::ChatUpload>)> {
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::bad_request(format!("bad multipart: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().unwrap_or("file").to_string();
        let mime = field.content_type().map(str::to_string);
        let bytes = field
            .bytes()
            .await
            .map_err(|e| AppError::bad_request(format!("reading file: {e}")))?;
        let saved = save_upload(&app, user.0.id, &filename, mime.as_deref(), &bytes).await?;
        return Ok((StatusCode::CREATED, saved));
    }
    Err(AppError::bad_request("no file field"))
}

#[derive(Deserialize)]
pub struct UploadBase64 {
    pub filename: String,
    #[serde(default)]
    pub mime: Option<String>,
    pub data_base64: String,
}

/// Base64 variant for MCP/CLI clients that cannot send multipart.
pub async fn upload_chat_file_base64(
    State(app): State<AppState>,
    user: AuthUser,
    Json(body): Json<UploadBase64>,
) -> AppResult<(StatusCode, Json<breadboard_core::ChatUpload>)> {
    use base64::Engine as _;
    let raw = body
        .data_base64
        .split_once("base64,")
        .map(|(_, rest)| rest)
        .unwrap_or(&body.data_base64);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .map_err(|_| AppError::bad_request("data_base64 is not valid base64"))?;
    let saved = save_upload(
        &app,
        user.0.id,
        &body.filename,
        body.mime.as_deref(),
        &bytes,
    )
    .await?;
    Ok((StatusCode::CREATED, saved))
}

pub async fn get_chat_upload(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let stored = store::get_chat_upload(&app.db, id).await?;
    let path = app.config.uploads_dir().join(&stored.path);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let inline = matches!(
        stored.upload.mime.as_str(),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    );
    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, &stored.upload.mime)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "private, max-age=86400");
    if !inline {
        let name = safe_display_name(&stored.upload.filename);
        builder = builder.header(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")).map_err(|_| {
                AppError::bad_request("invalid filename")
            })?,
        );
    }
    builder
        .body(Body::from(bytes))
        .map_err(|e| AppError::Other(anyhow::anyhow!("building upload response: {e}")))
}
pub async fn post_chat_message(
    State(app): State<AppState>,
    user: AuthUser,
    Json(body): Json<breadboard_core::dto::NewChatMessage>,
) -> AppResult<(StatusCode, Json<breadboard_core::ChatMessage>)> {
    let text = body.body.trim();
    if text.is_empty() && body.uploads.is_empty() {
        return Err(crate::error::AppError::bad_request("message is empty"));
    }
    if text.chars().count() > 2000 {
        return Err(crate::error::AppError::bad_request(
            "message must be 2000 characters or fewer",
        ));
    }
    let (author, display, kind) = chat_author(&app, &user, body.agent_name.as_deref()).await?;

    let room = if let Some(to) = body.to.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let peer = store::agent_member(to);
        if peer == author {
            return Err(crate::error::AppError::bad_request(
                "cannot DM yourself",
            ));
        }
        store::find_or_create_dm(&app.db, &author, &peer).await?
    } else {
        resolve_chat_room(&app, body.room.as_deref()).await?
    };

    if kind == breadboard_core::ChatAuthorKind::Agent
        && room.kind == breadboard_core::ChatRoomKind::Dm
        && !store::chat_is_member(&app.db, room.id, &author).await?
    {
        return Err(crate::error::AppError::forbidden());
    }

    let message = store::post_chat_message(
        &app.db,
        room.id,
        &author,
        &display,
        kind,
        text,
        &body.uploads,
        user.0.id,
    )
    .await?;
    app.bus.publish(BoardEvent::ChatMessage {
        room_id: room.id,
        id: message.id,
    });
    notify::emit_bread(
        &app,
        &BoardEvent::ChatMessage {
            room_id: room.id,
            id: message.id,
        },
    );
    Ok((StatusCode::CREATED, Json(message)))
}
pub async fn set_agent_message(
    State(app): State<AppState>,
    user: AuthUser,
    Json(body): Json<breadboard_core::dto::AgentMessage>,
) -> AppResult<Json<serde_json::Value>> {
    let agent_name = body.agent_name.trim();
    if agent_name.is_empty() {
        return Err(crate::error::AppError::bad_request("agent_name is required"));
    }
    let message = breadboard_core::dto::normalize_agent_message(&body.message)
        .map_err(crate::error::AppError::BadRequest)?;
    store::set_agent_message(&app.db, user.0.id, agent_name, message.as_deref()).await?;
    Ok(Json(serde_json::json!({
        "agent_name": agent_name.to_lowercase(),
        "message": message,
    })))
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
        if c.board_status == breadboard_core::Status::Done {
            done += 1;
            continue;
        }
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
    mode: Option<ClaimMode>,
    session_id: Option<&str>,
) -> AppResult<breadboard_core::AgentClaim> {
    let mut claims = store::find_user_claims(&app.db, issue_id, user_id, agent).await?;
    if let Some(mode) = mode {
        claims.retain(|c| c.mode == mode);
    }
    // Prefer the exclusive claim, then the freshest one. Never fail just
    // because one agent also has an intent on the issue it is working.
    claims.sort_by(|a, b| {
        (a.mode == ClaimMode::Intent)
            .cmp(&(b.mode == ClaimMode::Intent))
            .then_with(|| b.last_heartbeat.cmp(&a.last_heartbeat))
    });
    if let Some(session) = session_id {
        if let Some(matched) = claims.iter().find(|c| c.session_id.as_deref() == Some(session)) {
            return Ok(matched.clone());
        }
    }
    claims
        .into_iter()
        .next()
        .ok_or(crate::error::AppError::NotFound)
}

#[derive(Deserialize)]
pub struct ClaimQuery {
    /// Agent name, when the caller owns more than one claim on the issue.
    #[serde(default)]
    pub agent: Option<String>,
    /// `exclusive` or `intent`, when the same agent holds both.
    #[serde(default)]
    pub mode: Option<ClaimMode>,
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
    if let Some(Some(url)) = &body.pr_url {
        if !breadboard_core::dto::is_safe_http_url(url) {
            return Err(crate::error::AppError::bad_request(
                "pr_url must be an absolute http(s) URL",
            ));
        }
    }
    let session = body
        .session_id
        .as_ref()
        .and_then(|v| v.as_deref());
    let existing = resolve_own_claim(
        &app,
        issue.id,
        user.0.id,
        q.agent.as_deref(),
        q.mode,
        session,
    )
    .await?;
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
    let claim =
        resolve_own_claim(&app, issue.id, user.0.id, q.agent.as_deref(), q.mode, None).await?;
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
