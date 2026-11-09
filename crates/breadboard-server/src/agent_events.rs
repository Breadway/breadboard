//! Per-agent activity timeline: recording, querying and retention.
//!
//! Events are written by the code path that performs the action (a claim, a
//! state update, a chat post, an observed PR merge), never reconstructed by a
//! poller. Store hooks call [`record_best_effort`] so a timeline write can
//! never turn a successful claim or chat message into an error.
//!
//! `GET /api/agents/{name}/events` returns newest-first, cursor-paginated
//! entries filtered by kind and time range.

use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::{FromRow, SqlitePool};

use breadboard_core::{AgentEvent, AgentEventKind, AgentEventPage};

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::AppState;

/// How long a timeline row is kept. The feed exists to answer "what did this
/// agent do yesterday", so a quarter is generous without becoming an archive.
pub const AGENT_EVENT_RETENTION_DAYS: i64 = 90;

/// Default page size for the activity feed.
const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

/// One event to insert. Construct with [`AgentEventInsert::new`] and chain the
/// optional links.
#[derive(Debug, Clone)]
pub struct AgentEventInsert {
    pub agent_name: String,
    pub kind: AgentEventKind,
    pub issue_id: Option<i64>,
    pub issue_key: Option<String>,
    pub pr_url: Option<String>,
    pub pr_number: Option<i64>,
    pub room_id: Option<i64>,
    pub message_id: Option<i64>,
    pub summary: Option<String>,
    pub detail: Option<String>,
    pub actor_id: Option<i64>,
}

impl AgentEventInsert {
    pub fn new(agent_name: impl Into<String>, kind: AgentEventKind) -> Self {
        Self {
            agent_name: agent_name.into(),
            kind,
            issue_id: None,
            issue_key: None,
            pr_url: None,
            pr_number: None,
            room_id: None,
            message_id: None,
            summary: None,
            detail: None,
            actor_id: None,
        }
    }

    pub fn issue(mut self, id: i64, key: &str) -> Self {
        self.issue_id = Some(id);
        self.issue_key = Some(key.to_string());
        self
    }

    pub fn pr(mut self, url: Option<&str>, number: Option<i64>) -> Self {
        self.pr_url = url.map(str::to_string);
        self.pr_number = number;
        self
    }

    pub fn chat(mut self, room_id: i64, message_id: Option<i64>) -> Self {
        self.room_id = Some(room_id);
        self.message_id = message_id;
        self
    }

    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn actor(mut self, actor_id: Option<i64>) -> Self {
        self.actor_id = actor_id;
        self
    }
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

/// Append one event. The caller owns the action; a returned error is a real
/// write failure, which store hooks downgrade with [`record_best_effort`].
pub async fn record(db: &SqlitePool, event: AgentEventInsert) -> AppResult<i64> {
    let agent_name = crate::names::normalize(&event.agent_name);
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO agent_events
            (agent_name, kind, issue_id, issue_key, pr_url, pr_number, room_id,
             message_id, summary, detail, actor_id, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         RETURNING id",
    )
    .bind(&agent_name)
    .bind(event.kind.as_str())
    .bind(event.issue_id)
    .bind(event.issue_key)
    .bind(event.pr_url)
    .bind(event.pr_number)
    .bind(event.room_id)
    .bind(event.message_id)
    .bind(event.summary)
    .bind(event.detail)
    .bind(event.actor_id)
    .bind(now_rfc3339())
    .fetch_one(db)
    .await?;
    Ok(id)
}

/// Record without failing the action that produced the event. Used from claim,
/// session and chat paths where the primary write already succeeded.
pub async fn record_best_effort(db: &SqlitePool, event: AgentEventInsert) {
    let kind = event.kind;
    let agent = event.agent_name.clone();
    if let Err(error) = record(db, event).await {
        tracing::warn!(agent = %agent, kind = %kind.as_str(), %error, "agent event write failed");
    }
}

#[derive(Debug, FromRow)]
struct AgentEventRow {
    id: i64,
    agent_name: String,
    kind: String,
    issue_id: Option<i64>,
    issue_key: Option<String>,
    pr_url: Option<String>,
    pr_number: Option<i64>,
    room_id: Option<i64>,
    message_id: Option<i64>,
    summary: Option<String>,
    detail: Option<String>,
    actor: Option<String>,
    at: String,
}

impl From<AgentEventRow> for AgentEvent {
    fn from(r: AgentEventRow) -> Self {
        AgentEvent {
            id: r.id,
            agent_name: r.agent_name,
            kind: AgentEventKind::parse(&r.kind).unwrap_or(AgentEventKind::StateChange),
            issue_id: r.issue_id,
            issue_key: r.issue_key,
            pr_url: r.pr_url,
            pr_number: r.pr_number,
            room_id: r.room_id,
            message_id: r.message_id,
            summary: r.summary.unwrap_or_default(),
            detail: r.detail,
            actor: r.actor,
            at: DateTime::parse_from_rfc3339(&r.at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        }
    }
}

/// Query parameters for one page of the timeline.
#[derive(Debug, Default, Deserialize)]
pub struct AgentEventQuery {
    /// Comma-separated kinds, e.g. `claimed,pr_opened`. Empty or absent = all.
    #[serde(default)]
    pub kind: Option<String>,
    /// Inclusive lower bound, RFC 3339.
    #[serde(default)]
    pub since: Option<String>,
    /// Inclusive upper bound, RFC 3339.
    #[serde(default)]
    pub until: Option<String>,
    /// Page size, 1-200 (default 50).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Cursor: return events with `id` lower than this (the `next_before` of
    /// the previous page).
    #[serde(default)]
    pub before: Option<i64>,
}

fn parse_bound(value: Option<&str>, label: &str) -> AppResult<Option<String>> {
    let Some(raw) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let parsed = DateTime::parse_from_rfc3339(raw)
        .map_err(|_| AppError::bad_request(format!("{label} must be an RFC 3339 timestamp")))?;
    Ok(Some(parsed.with_timezone(&Utc).to_rfc3339()))
}

fn parse_kinds(value: Option<&str>) -> AppResult<String> {
    let Some(raw) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(String::new());
    };
    let mut kinds = Vec::new();
    for part in raw.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let kind = AgentEventKind::parse(part).ok_or_else(|| {
            AppError::bad_request(format!(
                "unknown event kind '{part}'; expected one of: {}",
                AgentEventKind::ALL
                    .iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    Ok(kinds
        .iter()
        .map(|k| k.as_str())
        .collect::<Vec<_>>()
        .join(","))
}

/// One newest-first page for `name`.
pub async fn list_agent_events(
    db: &SqlitePool,
    name: &str,
    query: &AgentEventQuery,
) -> AppResult<AgentEventPage> {
    let name = crate::names::normalize(name);
    let since = parse_bound(query.since.as_deref(), "since")?;
    let until = parse_bound(query.until.as_deref(), "until")?;
    let kinds = parse_kinds(query.kind.as_deref())?;
    if let (Some(since), Some(until)) = (&since, &until) {
        if since > until {
            return Err(AppError::bad_request("since must not be after until"));
        }
    }
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let before = query.before.unwrap_or(0).max(0);

    let rows: Vec<AgentEventRow> = sqlx::query_as(
        "SELECT e.id, e.agent_name, e.kind, e.issue_id, e.issue_key, e.pr_url,
                e.pr_number, e.room_id, e.message_id, e.summary, e.detail,
                u.login AS actor, e.at
           FROM agent_events e
           LEFT JOIN users u ON u.id = e.actor_id
          WHERE e.agent_name = ?1
            AND (?2 = '' OR e.at >= ?2)
            AND (?3 = '' OR e.at <= ?3)
            AND (?4 = '' OR instr(','||?4||',', ','||e.kind||',') > 0)
            AND (?5 = 0 OR e.id < ?5)
          ORDER BY e.at DESC, e.id DESC
          LIMIT ?6",
    )
    .bind(&name)
    .bind(since.unwrap_or_default())
    .bind(until.unwrap_or_default())
    .bind(&kinds)
    .bind(before)
    .bind(limit + 1)
    .fetch_all(db)
    .await?;

    let has_more = rows.len() as i64 > limit;
    let mut events: Vec<AgentEvent> = rows
        .into_iter()
        .take(limit as usize)
        .map(Into::into)
        .collect();
    let next_before = if has_more {
        events.last().map(|event| event.id)
    } else {
        None
    };
    // `events` is newest first; keep that order, it is what the panel shows.
    events.shrink_to_fit();
    Ok(AgentEventPage {
        events,
        has_more,
        next_before,
    })
}

/// Delete rows older than the retention window. Runs at startup and daily.
pub async fn prune_agent_events(db: &SqlitePool, retain_days: i64) -> AppResult<u64> {
    let cutoff = (Utc::now() - chrono::Duration::days(retain_days.max(1))).to_rfc3339();
    let deleted = sqlx::query("DELETE FROM agent_events WHERE at < ?1")
        .bind(cutoff)
        .execute(db)
        .await?
        .rows_affected();
    Ok(deleted)
}

/// Apply the retention window now and then once a day.
pub fn spawn_pruner(state: AppState) {
    tokio::spawn(async move {
        prune_once(&state, "startup").await;
        let mut tick = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
        tick.tick().await; // the first tick is immediate; startup already ran
        loop {
            tick.tick().await;
            prune_once(&state, "daily").await;
        }
    });
}

async fn prune_once(state: &AppState, reason: &str) {
    match prune_agent_events(&state.db, AGENT_EVENT_RETENTION_DAYS).await {
        Ok(0) => {}
        Ok(deleted) => tracing::info!(deleted, reason, "pruned agent events"),
        Err(error) => tracing::warn!(%error, reason, "pruning agent events failed"),
    }
}

/// `GET /api/agents/{name}/events`.
pub async fn agent_events(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(name): Path<String>,
    Query(query): Query<AgentEventQuery>,
) -> AppResult<Json<AgentEventPage>> {
    Ok(Json(list_agent_events(&app.db, &name, &query).await?))
}

/// Routes owned by this module, merged by `http::router`.
pub fn routes() -> Router<AppState> {
    Router::new().route("/agents/{name}/events", get(agent_events))
}
