//! Review requests as records: the storage, REST handlers, reviewer queue,
//! notification fan-out, and escalation loop.
//!
//! A request ties one issue (and the PR under review) to one human or agent
//! reviewer. Only `approved` clears the outstanding flag, so the same check
//! gates `done` in `store::update_issue` and the merged-PR cascade.

use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use breadboard_core::dto::{NewReviewRequest, ReviewRequestPatch};
use breadboard_core::{
    BoardEvent, ChatAuthorKind, Issue, ReviewRequest as ReviewRequestRecord, ReviewState,
    ReviewerKind, Status, User,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::{notify, store, AppState};

/// How long an unresolved request may sit untouched before one escalation.
/// Kept as a constant so every deployment agrees on the threshold; a request
/// is only escalated once per state change.
pub const REVIEW_ESCALATION_SECS: i64 = 3600;

const SELECT: &str = "SELECT r.id, r.issue_id, i.key AS issue_key, i.title AS issue_title, \
    i.status AS issue_status, r.pr_url, r.pr_number, r.pr_repo, r.requested_by, \
    u.login AS requested_by_login, r.reviewer_id, r.reviewer, r.reviewer_kind, r.state, \
    r.note, r.requested_at, r.updated_at, r.responded_at, r.escalated_at \
    FROM review_requests r \
    JOIN issues i ON i.id = r.issue_id \
    LEFT JOIN users u ON u.id = r.requested_by";

#[derive(sqlx::FromRow)]
struct ReviewRow {
    id: i64,
    issue_id: i64,
    issue_key: String,
    issue_title: String,
    issue_status: String,
    pr_url: Option<String>,
    pr_number: Option<i64>,
    pr_repo: Option<String>,
    requested_by: Option<i64>,
    requested_by_login: Option<String>,
    reviewer_id: Option<i64>,
    reviewer: String,
    reviewer_kind: String,
    state: String,
    note: Option<String>,
    requested_at: String,
    updated_at: String,
    responded_at: Option<String>,
    escalated_at: Option<String>,
}

impl ReviewRow {
    fn into_record(self) -> ReviewRequestRecord {
        ReviewRequestRecord {
            id: self.id,
            issue_id: self.issue_id,
            issue_key: self.issue_key,
            issue_title: self.issue_title,
            issue_status: Status::parse(&self.issue_status).unwrap_or(Status::Backlog),
            pr_url: self.pr_url,
            pr_number: self.pr_number,
            pr_repo: self.pr_repo,
            requested_by: self.requested_by,
            requested_by_login: self.requested_by_login,
            reviewer_id: self.reviewer_id,
            reviewer: self.reviewer,
            reviewer_kind: ReviewerKind::parse(&self.reviewer_kind).unwrap_or(ReviewerKind::Human),
            state: ReviewState::parse(&self.state).unwrap_or(ReviewState::Requested),
            note: self.note,
            requested_at: parse_ts(&self.requested_at),
            updated_at: parse_ts(&self.updated_at),
            responded_at: self.responded_at.as_deref().map(parse_ts),
            escalated_at: self.escalated_at.as_deref().map(parse_ts),
        }
    }
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

fn parse_ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| DateTime::<Utc>::from_timestamp(0, 0).unwrap_or_default())
}

fn normalize_agent(name: &str) -> String {
    name.trim().to_lowercase().replace(' ', "-")
}

fn reviewer_matches(name: &str, candidate: &str) -> bool {
    normalize_agent(name) == normalize_agent(candidate)
}

fn pr_label(req: &ReviewRequestRecord) -> String {
    if let Some(number) = req.pr_number {
        return match &req.pr_repo {
            Some(repo) => format!("{repo}#{number}"),
            None => format!("PR #{number}"),
        };
    }
    req.pr_url
        .clone()
        .unwrap_or_else(|| "the linked PR".to_string())
}

fn outcome_label(state: ReviewState) -> &'static str {
    match state {
        ReviewState::Requested => "Review requested",
        ReviewState::Accepted => "Review accepted",
        ReviewState::ChangesRequested => "Changes requested",
        ReviewState::Approved => "Review approved",
    }
}

/// The owner user for an agent name, if any. Mirrors `store::agent_owners`
/// without making the store helper public.
async fn agent_owner(db: &SqlitePool, agent_name: &str) -> AppResult<Option<i64>> {
    let name = normalize_agent(agent_name);
    Ok(sqlx::query_scalar(
        "SELECT user_id FROM (
            SELECT user_id FROM agent_profiles WHERE lower(agent_name) = ?1
            UNION
            SELECT claimed_by AS user_id FROM agent_claims WHERE lower(agent_name) = ?1
            UNION
            SELECT user_id FROM agent_sessions WHERE lower(agent_name) = ?1 AND user_id IS NOT NULL
         ) ORDER BY user_id LIMIT 1",
    )
    .bind(&name)
    .fetch_optional(db)
    .await?)
}

pub async fn list_for_issue(db: &SqlitePool, issue_id: i64) -> AppResult<Vec<ReviewRequestRecord>> {
    let sql = format!("{SELECT} WHERE r.issue_id = ?1 ORDER BY r.requested_at DESC, r.id DESC");
    let rows: Vec<ReviewRow> = sqlx::query_as(&sql).bind(issue_id).fetch_all(db).await?;
    Ok(rows.into_iter().map(ReviewRow::into_record).collect())
}

pub async fn list_for_reviewer(
    db: &SqlitePool,
    reviewer: &str,
    outstanding_only: bool,
) -> AppResult<Vec<ReviewRequestRecord>> {
    let sql = if outstanding_only {
        format!(
            "{SELECT} WHERE lower(r.reviewer) = lower(?1) \
             AND r.state IN ('requested', 'accepted', 'changes_requested') \
             ORDER BY r.requested_at ASC, r.id ASC"
        )
    } else {
        format!("{SELECT} WHERE lower(r.reviewer) = lower(?1) ORDER BY r.requested_at DESC, r.id DESC LIMIT 500")
    };
    let rows: Vec<ReviewRow> = sqlx::query_as(&sql)
        .bind(reviewer.trim())
        .fetch_all(db)
        .await?;
    Ok(rows.into_iter().map(ReviewRow::into_record).collect())
}

pub async fn list_outstanding(db: &SqlitePool) -> AppResult<Vec<ReviewRequestRecord>> {
    let sql = format!(
        "{SELECT} WHERE r.state IN ('requested', 'accepted', 'changes_requested') \
         ORDER BY r.requested_at ASC, r.id ASC LIMIT 500"
    );
    let rows: Vec<ReviewRow> = sqlx::query_as(&sql).fetch_all(db).await?;
    Ok(rows.into_iter().map(ReviewRow::into_record).collect())
}

pub async fn get(db: &SqlitePool, id: i64) -> AppResult<ReviewRequestRecord> {
    let sql = format!("{SELECT} WHERE r.id = ?1");
    let row: Option<ReviewRow> = sqlx::query_as(&sql).bind(id).fetch_optional(db).await?;
    row.map(ReviewRow::into_record).ok_or(AppError::NotFound)
}

/// Whether an unresolved request still blocks this issue from being done.
pub async fn has_outstanding(db: &SqlitePool, issue_id: i64) -> AppResult<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM review_requests \
         WHERE issue_id = ?1 AND state IN ('requested', 'accepted', 'changes_requested')",
    )
    .bind(issue_id)
    .fetch_one(db)
    .await?;
    Ok(count > 0)
}

pub async fn first_outstanding(
    db: &SqlitePool,
    issue_id: i64,
) -> AppResult<Option<ReviewRequestRecord>> {
    let sql = format!(
        "{SELECT} WHERE r.issue_id = ?1 \
         AND r.state IN ('requested', 'accepted', 'changes_requested') \
         ORDER BY r.requested_at ASC, r.id ASC LIMIT 1"
    );
    let row: Option<ReviewRow> = sqlx::query_as(&sql)
        .bind(issue_id)
        .fetch_optional(db)
        .await?;
    Ok(row.map(ReviewRow::into_record))
}

pub async fn create(
    db: &SqlitePool,
    issue: &Issue,
    new: &NewReviewRequest,
    requester: &User,
) -> AppResult<ReviewRequestRecord> {
    let reviewer = new.reviewer.trim().to_string();
    let kind = match new.reviewer_kind {
        Some(kind) => kind,
        None => {
            if store::find_user_by_login(db, &reviewer).await?.is_some() {
                ReviewerKind::Human
            } else if store::known_agent_names(db)
                .await?
                .contains(&normalize_agent(&reviewer))
            {
                ReviewerKind::Agent
            } else {
                ReviewerKind::Human
            }
        }
    };
    let reviewer_id = match kind {
        ReviewerKind::Human => store::find_user_by_login(db, &reviewer).await?,
        ReviewerKind::Agent => agent_owner(db, &reviewer).await?,
    };

    let ts = now_rfc3339();
    let id: Result<i64, sqlx::Error> = sqlx::query_scalar(
        "INSERT INTO review_requests
            (issue_id, pr_url, pr_number, pr_repo, requested_by, reviewer_id, reviewer,
             reviewer_kind, state, note, requested_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'requested', ?9, ?10, ?10)
         RETURNING id",
    )
    .bind(issue.id)
    .bind(&new.pr_url)
    .bind(new.pr_number)
    .bind(&new.pr_repo)
    .bind(requester.id)
    .bind(reviewer_id)
    .bind(&reviewer)
    .bind(kind.as_str())
    .bind(new.note.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(&ts)
    .fetch_one(db)
    .await;

    match id {
        Ok(id) => get(db, id).await,
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            Err(AppError::Conflict(format!(
                "{reviewer} already has an open review request on {}",
                issue.key
            )))
        }
        Err(e) => Err(e.into()),
    }
}

pub async fn update_state(
    db: &SqlitePool,
    id: i64,
    patch: &ReviewRequestPatch,
    actor: &User,
) -> AppResult<ReviewRequestRecord> {
    let current = get(db, id).await?;

    let actor_is_requester = current.requested_by == Some(actor.id);
    let actor_is_reviewer = current.reviewer_id == Some(actor.id);
    let actor_is_named_agent = current.reviewer_kind == ReviewerKind::Agent
        && patch
            .agent_name
            .as_deref()
            .is_some_and(|name| reviewer_matches(name, &current.reviewer));
    if !(actor_is_requester || actor_is_reviewer || actor_is_named_agent) {
        return Err(AppError::forbidden_msg(
            "only the requester or the named reviewer can update this request",
        ));
    }

    if let Some(next) = patch.state {
        if current.state == ReviewState::Approved && next != ReviewState::Approved {
            return Err(AppError::Conflict(
                "an approved review request is closed; file a new request instead".into(),
            ));
        }
    }

    let ts = now_rfc3339();
    let mut tx = crate::db::begin_write(db).await?;
    let mut set_state = false;
    if let Some(next) = patch.state {
        if next != current.state {
            set_state = true;
            sqlx::query(
                "UPDATE review_requests
                    SET state = ?2, responded_at = ?3, escalated_at = NULL, updated_at = ?4
                  WHERE id = ?1",
            )
            .bind(id)
            .bind(next.as_str())
            .bind(if next == ReviewState::Requested {
                None
            } else {
                Some(ts.clone())
            })
            .bind(&ts)
            .execute(&mut *tx)
            .await?;
        }
    }
    if let Some(note) = &patch.note {
        let note = note.trim();
        sqlx::query("UPDATE review_requests SET note = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(id)
            .bind(if note.is_empty() { None } else { Some(note) })
            .bind(&ts)
            .execute(&mut *tx)
            .await?;
    } else if !set_state {
        // A note-only patch that changed nothing still bumps the clock.
        sqlx::query("UPDATE review_requests SET updated_at = ?2 WHERE id = ?1")
            .bind(id)
            .bind(&ts)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    let after = get(db, id).await?;
    Ok(after)
}

/// Mark every unresolved request that has not changed state for the threshold
/// as escalated. Returns only the rows this call actually escalated, so the
/// caller can notify once.
pub async fn escalate_stale(
    db: &SqlitePool,
    now: DateTime<Utc>,
) -> AppResult<Vec<ReviewRequestRecord>> {
    let cutoff = (now - chrono::Duration::seconds(REVIEW_ESCALATION_SECS)).to_rfc3339();
    let ids: Vec<i64> = sqlx::query_scalar(
        "UPDATE review_requests
            SET escalated_at = ?1
          WHERE state IN ('requested', 'accepted', 'changes_requested')
            AND escalated_at IS NULL
            AND updated_at <= ?2
        RETURNING id",
    )
    .bind(now.to_rfc3339())
    .bind(cutoff)
    .fetch_all(db)
    .await?;

    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(get(db, id).await?);
    }
    Ok(out)
}

/// Escalation loop: at most once per request per untouched window.
pub fn spawn_escalation(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(300));
        loop {
            tick.tick().await;
            match escalate_stale(&state.db, Utc::now()).await {
                Ok(requests) => {
                    for request in requests {
                        notify_escalated(&state, &request).await;
                        publish(&state, &request, true);
                    }
                }
                Err(error) => tracing::warn!(%error, "review escalation pass failed"),
            }
        }
    });
}

fn publish(app: &AppState, request: &ReviewRequestRecord, escalated: bool) {
    let event = BoardEvent::ReviewUpdated {
        id: request.id,
        issue_id: request.issue_id,
        key: request.issue_key.clone(),
        reviewer: request.reviewer.clone(),
        state: request.state.as_str().to_string(),
        escalated,
    };
    app.bus.publish(event.clone());
    notify::emit_bread(app, &event);
}

async fn notify_reviewer(
    app: &AppState,
    request: &ReviewRequestRecord,
    issue: &Issue,
    requester: &User,
    body: String,
) {
    match request.reviewer_kind {
        ReviewerKind::Human => {
            if let Some(user_id) = request.reviewer_id {
                notify::review(app, user_id, issue, body).await;
            }
        }
        ReviewerKind::Agent => {
            if let Err(error) = notify_agent(app, &request.reviewer, requester, issue, &body).await
            {
                tracing::warn!(%error, reviewer = %request.reviewer, "agent review notification failed");
            }
        }
    }
}

/// Agents are woken through the existing chat push path: a DM from the
/// requester lands in the agent's `chat_pushes` inbox.
async fn notify_agent(
    app: &AppState,
    agent_name: &str,
    from: &User,
    _issue: &Issue,
    body: &str,
) -> AppResult<()> {
    let author = store::human_member(&from.login);
    let peer = store::agent_member(agent_name);
    let room = store::find_or_create_dm(&app.db, &author, &peer).await?;
    let message = store::post_chat_message(
        &app.db,
        room.id,
        &author,
        &from.display_name,
        ChatAuthorKind::Human,
        body,
        &[],
        from.id,
    )
    .await?;
    store::create_chat_pushes(&app.db, &room, &message, &author).await?;
    app.bus.publish(BoardEvent::ChatMessage {
        room_id: room.id,
        id: message.id,
    });
    Ok(())
}

async fn notify_created(
    app: &AppState,
    request: &ReviewRequestRecord,
    issue: &Issue,
    requester: &User,
) {
    let body = format!(
        "Review requested on {} ({}): please review it and record an outcome.",
        issue.key,
        pr_label(request)
    );
    notify_reviewer(app, request, issue, requester, body).await;
}

async fn notify_outcome(app: &AppState, request: &ReviewRequestRecord, actor: &User) {
    // Tell the requester when the reviewer acted, and tell the reviewer when
    // the requester changed the record. Skip self-notifications.
    let Ok(issue) = store::get_issue(&app.db, request.issue_id).await else {
        return;
    };
    let body = match &request.note {
        Some(note) if !note.trim().is_empty() => format!(
            "{} on {} ({}): {}",
            outcome_label(request.state),
            request.issue_key,
            pr_label(request),
            note.trim()
        ),
        _ => format!(
            "{} on {} ({})",
            outcome_label(request.state),
            request.issue_key,
            pr_label(request)
        ),
    };
    if let Some(requester_id) = request.requested_by {
        if requester_id != actor.id {
            notify::review(app, requester_id, &issue, body.clone()).await;
        }
    }
    if request.reviewer_kind == ReviewerKind::Agent
        && request.reviewer_id.is_some_and(|id| id != actor.id)
    {
        if let Some(user_id) = request.reviewer_id {
            notify::review(app, user_id, &issue, body).await;
        }
    }
}

async fn notify_escalated(app: &AppState, request: &ReviewRequestRecord) {
    let Ok(issue) = store::get_issue(&app.db, request.issue_id).await else {
        return;
    };
    let minutes = REVIEW_ESCALATION_SECS / 60;
    let body = format!(
        "Review request on {} ({}) has been waiting over {minutes} minutes: {} is still {}.",
        request.issue_key,
        pr_label(request),
        request.reviewer,
        request.state.as_str()
    );
    match request.reviewer_kind {
        ReviewerKind::Human => {
            if let Some(user_id) = request.reviewer_id {
                notify::review(app, user_id, &issue, body.clone()).await;
            }
        }
        ReviewerKind::Agent => {
            let requester = match request.requested_by {
                Some(id) => sqlx::query_as::<_, (i64, String, String)>(
                    "SELECT id, login, display_name FROM users WHERE id = ?1",
                )
                .bind(id)
                .fetch_optional(&app.db)
                .await
                .ok()
                .flatten()
                .map(|(id, login, display_name)| User {
                    id,
                    forgejo_id: 0,
                    login,
                    display_name,
                    avatar_url: None,
                }),
                None => None,
            };
            if let Some(requester) = requester {
                if let Err(error) =
                    notify_agent(app, &request.reviewer, &requester, &issue, &body).await
                {
                    tracing::warn!(%error, "agent escalation notification failed");
                }
            }
        }
    }
    if let Some(requester_id) = request.requested_by {
        notify::review(app, requester_id, &issue, body).await;
    }
}

// --- REST handlers --------------------------------------------------------

#[derive(Deserialize)]
pub struct ReviewQuery {
    pub reviewer: Option<String>,
    pub outstanding: Option<bool>,
    pub issue: Option<String>,
    pub limit: Option<u32>,
}

pub async fn list_issue_reviews(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(key): Path<String>,
) -> AppResult<Json<Vec<ReviewRequestRecord>>> {
    let issue = store::get_issue_by_key(&app.db, &key).await?;
    Ok(Json(list_for_issue(&app.db, issue.id).await?))
}

pub async fn create_review_request(
    State(app): State<AppState>,
    user: AuthUser,
    Path(key): Path<String>,
    Json(body): Json<NewReviewRequest>,
) -> AppResult<(StatusCode, Json<ReviewRequestRecord>)> {
    body.validate().map_err(AppError::bad_request)?;
    let issue = store::get_issue_by_key(&app.db, &key).await?;
    let request = create(&app.db, &issue, &body, &user.0).await?;
    notify_created(&app, &request, &issue, &user.0).await;
    publish(&app, &request, false);
    Ok((StatusCode::CREATED, Json(request)))
}

pub async fn list_reviews(
    State(app): State<AppState>,
    _user: AuthUser,
    Query(query): Query<ReviewQuery>,
) -> AppResult<Json<Vec<ReviewRequestRecord>>> {
    if let Some(key) = query.issue.as_deref().filter(|s| !s.trim().is_empty()) {
        let issue = store::get_issue_by_key(&app.db, key).await?;
        return Ok(Json(list_for_issue(&app.db, issue.id).await?));
    }
    let outstanding_only = query.outstanding.unwrap_or(false);
    if let Some(reviewer) = query.reviewer.as_deref().filter(|s| !s.trim().is_empty()) {
        return Ok(Json(
            list_for_reviewer(&app.db, reviewer, outstanding_only).await?,
        ));
    }
    let mut rows = list_outstanding(&app.db).await?;
    if let Some(limit) = query.limit {
        rows.truncate(limit.min(500) as usize);
    }
    Ok(Json(rows))
}

pub async fn update_review_request(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(patch): Json<ReviewRequestPatch>,
) -> AppResult<Json<ReviewRequestRecord>> {
    patch.validate().map_err(AppError::bad_request)?;
    let request = update_state(&app.db, id, &patch, &user.0).await?;
    notify_outcome(&app, &request, &user.0).await;
    publish(&app, &request, false);
    Ok(Json(request))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_outstanding_rules() {
        assert!(ReviewState::Requested.is_outstanding());
        assert!(ReviewState::Accepted.is_outstanding());
        assert!(ReviewState::ChangesRequested.is_outstanding());
        assert!(!ReviewState::Approved.is_outstanding());
    }

    #[test]
    fn agent_names_match_across_separators() {
        assert!(reviewer_matches("Brazen Croissant", "brazen-croissant"));
        assert!(reviewer_matches(" Zesty Rye ", "zesty rye"));
        assert!(!reviewer_matches("Zesty Rye", "zesty-rye-2"));
    }
}
