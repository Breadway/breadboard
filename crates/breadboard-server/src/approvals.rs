//! Approval asks: storage, the state machine, REST handlers and the
//! operator-only decision gate.
//!
//! Product rules enforced here, not in the UI:
//! - an agent creates, revises, comments on and withdraws its own ask,
//! - only a human operator records a decision; any request that presents an
//!   agent identity is refused, and `approvals.operators` can pin the allowed
//!   logins,
//! - a decision is one-shot: only a pending ask can be decided,
//! - request changes returns the same row to the owning agent; resubmitting
//!   increments `revision` and puts it back in `pending`,
//! - send info / instructions closes the ask as `answered`,
//! - a pending ask past `expires_at` becomes `expired` and cannot be decided.

use std::collections::{BTreeMap, HashMap};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use breadboard_core::dto::{
    ApprovalAttachmentSpec, ApprovalComment, ApprovalDecisionInput, ApprovalRevision,
    ApprovalWithdrawal, MaintenanceWindowPatch, NewApprovalAsk, NewFreezePeriod,
    NewMaintenanceWindow,
};
use breadboard_core::{
    ApprovalActorKind, ApprovalAsk, ApprovalAttachment, ApprovalCounts, ApprovalDecision,
    ApprovalEvent, ApprovalEventKind, ApprovalKind, ApprovalStatus, ApprovalUrgency, BoardEvent,
    FreezePeriod, Issue, MaintenanceStatus, MaintenanceWindow, User,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::artifacts::{self, UploadInput};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::{store, AppState};

/// List page size. The inbox never needs the whole table.
const DEFAULT_LIMIT: i64 = 200;
const MAX_LIMIT: i64 = 500;

const SELECT: &str = "SELECT a.id, a.project_id, p.key AS project_key, p.name AS project_name, \
    a.issue_id, i.key AS issue_key, i.title AS issue_title, i.status AS issue_status, \
    a.agent_name, a.session_id, a.kind, a.urgency, a.title, a.summary, a.body_md, \
    a.environment, a.status, a.revision, a.checklist_json, a.risk_md, a.rollback_md, \
    a.expires_at, a.deadline, a.decided_by, a.decided_by_login, a.decided_at, \
    a.decision_note_md, a.created_at, a.updated_at \
    FROM approval_asks a \
    LEFT JOIN projects p ON p.id = a.project_id \
    LEFT JOIN issues i ON i.id = a.issue_id";

#[derive(sqlx::FromRow)]
struct AskRow {
    id: i64,
    project_id: Option<i64>,
    project_key: Option<String>,
    project_name: Option<String>,
    issue_id: Option<i64>,
    issue_key: Option<String>,
    issue_title: Option<String>,
    issue_status: Option<String>,
    agent_name: String,
    session_id: Option<String>,
    kind: String,
    urgency: String,
    title: String,
    summary: String,
    body_md: String,
    environment: Option<String>,
    status: String,
    revision: i64,
    checklist_json: String,
    risk_md: Option<String>,
    rollback_md: Option<String>,
    expires_at: Option<String>,
    deadline: Option<String>,
    decided_by: Option<i64>,
    decided_by_login: Option<String>,
    decided_at: Option<String>,
    decision_note_md: Option<String>,
    created_at: String,
    updated_at: String,
}

impl AskRow {
    fn into_ask(self) -> ApprovalAsk {
        ApprovalAsk {
            id: self.id,
            project_id: self.project_id,
            project_key: self.project_key,
            project_name: self.project_name,
            issue_id: self.issue_id,
            issue_key: self.issue_key,
            issue_title: self.issue_title,
            issue_status: self
                .issue_status
                .as_deref()
                .and_then(breadboard_core::Status::parse),
            agent_name: self.agent_name,
            session_id: self.session_id,
            kind: ApprovalKind::parse(&self.kind).unwrap_or(ApprovalKind::Approval),
            urgency: ApprovalUrgency::parse(&self.urgency).unwrap_or(ApprovalUrgency::Normal),
            title: self.title,
            summary: self.summary,
            body_md: self.body_md,
            environment: self.environment,
            status: ApprovalStatus::parse(&self.status).unwrap_or(ApprovalStatus::Pending),
            revision: self.revision,
            checklist: parse_string_list(&self.checklist_json),
            risk_md: self.risk_md,
            rollback_md: self.rollback_md,
            expires_at: self.expires_at.as_deref().map(parse_ts),
            deadline: self.deadline,
            unread: false,
            decided_by: self.decided_by,
            decided_by_login: self.decided_by_login,
            decided_at: self.decided_at.as_deref().map(parse_ts),
            decision_note_md: self.decision_note_md,
            created_at: parse_ts(&self.created_at),
            updated_at: parse_ts(&self.updated_at),
            events: Vec::new(),
            attachments: Vec::new(),
            window: None,
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

fn agent_matches(a: &str, b: &str) -> bool {
    normalize_agent(a) == normalize_agent(b)
}

fn parse_string_list(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

fn list_json(items: &[String]) -> String {
    serde_json::to_string(items).unwrap_or_else(|_| "[]".into())
}

// --- agent and operator identity -----------------------------------------

/// Resolve an agent identity the caller actually owns. Mirrors the artifact
/// uploader rule so a shared human session cannot speak for someone else's
/// agent.
async fn resolve_agent(
    app: &AppState,
    user: &User,
    agent_name: &str,
    session_id: Option<&str>,
) -> AppResult<(String, Option<String>)> {
    let session_id = session_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let name = agent_name.trim();
    if name.is_empty() {
        return Err(AppError::bad_request("agent_name is required"));
    }
    let normalized =
        store::ensure_agent_writable(&app.db, user.id, name, session_id.as_deref()).await?;
    if !store::user_has_agent(&app.db, user.id, &normalized).await? {
        return Err(AppError::forbidden_msg(
            "agent_name does not belong to this user",
        ));
    }
    Ok((normalized, session_id))
}

/// Decisions are the operator's. `approvals.operators` pins the allowed logins;
/// an empty list means any authenticated human may decide.
pub fn is_operator(app: &AppState, user: &User) -> bool {
    let operators = &app.config.approvals.operators;
    if operators.is_empty() {
        return true;
    }
    operators
        .iter()
        .any(|login| login.trim().eq_ignore_ascii_case(&user.login))
}

fn require_operator(app: &AppState, user: &User, agent_name: Option<&str>) -> AppResult<()> {
    if agent_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .is_some()
    {
        return Err(AppError::forbidden_msg(
            "approval decisions are operator-only; an agent cannot decide its own ask",
        ));
    }
    if !is_operator(app, user) {
        return Err(AppError::forbidden_msg(
            "only a configured operator can decide approval asks",
        ));
    }
    Ok(())
}

// --- events ---------------------------------------------------------------

async fn insert_event(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    ask_id: i64,
    kind: ApprovalEventKind,
    actor_kind: ApprovalActorKind,
    actor_name: &str,
    body_md: &str,
    meta: &BTreeMap<String, String>,
) -> AppResult<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO approval_events
            (ask_id, kind, actor_kind, actor_name, body_md, created_at, meta_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         RETURNING id",
    )
    .bind(ask_id)
    .bind(kind.as_str())
    .bind(actor_kind.as_str())
    .bind(actor_name)
    .bind(body_md)
    .bind(now_rfc3339())
    .bind(serde_json::to_string(meta).unwrap_or_else(|_| "{}".into()))
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

#[derive(sqlx::FromRow)]
struct EventRow {
    id: i64,
    ask_id: i64,
    kind: String,
    actor_kind: String,
    actor_name: String,
    body_md: String,
    created_at: String,
    meta_json: String,
}

impl From<EventRow> for ApprovalEvent {
    fn from(row: EventRow) -> Self {
        ApprovalEvent {
            id: row.id,
            ask_id: row.ask_id,
            kind: ApprovalEventKind::parse(&row.kind).unwrap_or(ApprovalEventKind::Comment),
            actor_kind: ApprovalActorKind::parse(&row.actor_kind)
                .unwrap_or(ApprovalActorKind::Human),
            actor_name: row.actor_name,
            body_md: row.body_md,
            created_at: parse_ts(&row.created_at),
            meta: serde_json::from_str::<BTreeMap<String, String>>(&row.meta_json)
                .unwrap_or_default(),
        }
    }
}

async fn events_for(db: &SqlitePool, ids: &[i64]) -> AppResult<Vec<ApprovalEvent>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
        "SELECT id, ask_id, kind, actor_kind, actor_name, body_md, created_at, meta_json \
         FROM approval_events WHERE ask_id IN (",
    );
    let mut separated = qb.separated(", ");
    for id in ids {
        separated.push_bind(*id);
    }
    separated.push_unseparated(") ORDER BY ask_id, id");
    let rows: Vec<EventRow> = qb.build_query_as().fetch_all(db).await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

// --- attachments ----------------------------------------------------------

#[derive(sqlx::FromRow)]
struct AttachmentRow {
    id: i64,
    ask_id: i64,
    artifact_id: i64,
    caption: String,
    sort: i64,
    filename: String,
    mime: String,
    size: i64,
    created_at: String,
}

impl From<AttachmentRow> for ApprovalAttachment {
    fn from(row: AttachmentRow) -> Self {
        Self {
            id: row.id,
            ask_id: row.ask_id,
            artifact_id: row.artifact_id,
            caption: row.caption,
            sort: row.sort,
            filename: row.filename,
            mime: row.mime,
            size: row.size,
            created_at: parse_ts(&row.created_at),
        }
    }
}

async fn attachments_for(db: &SqlitePool, ids: &[i64]) -> AppResult<Vec<ApprovalAttachment>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
        "SELECT aa.id, aa.ask_id, aa.artifact_id, aa.caption, aa.sort, \
                ar.filename, ar.mime, ar.size, aa.created_at \
         FROM approval_attachments aa \
         JOIN issue_artifacts ar ON ar.id = aa.artifact_id \
         WHERE aa.ask_id IN (",
    );
    let mut separated = qb.separated(", ");
    for id in ids {
        separated.push_bind(*id);
    }
    separated.push_unseparated(") ORDER BY aa.ask_id, aa.sort, aa.id");
    let rows: Vec<AttachmentRow> = qb.build_query_as().fetch_all(db).await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Decoded base64 upload, validated before the ask row is written.
struct DecodedAttachment {
    filename: String,
    mime: Option<String>,
    bytes: Vec<u8>,
}

fn decode_attachments(specs: &[ApprovalAttachmentSpec]) -> AppResult<Vec<DecodedAttachment>> {
    use base64::Engine as _;
    let mut out = Vec::new();
    for spec in specs {
        if let Some(raw) = &spec.data_base64 {
            let data = raw
                .split_once("base64,")
                .map(|(_, rest)| rest)
                .unwrap_or(raw.as_str());
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data.trim())
                .map_err(|_| AppError::bad_request("data_base64 is not valid base64"))?;
            if bytes.is_empty() {
                return Err(AppError::bad_request("an attachment is empty"));
            }
            if bytes.len() > artifacts::kind_limit(breadboard_core::ArtifactKind::Screenshot) {
                return Err(AppError::TooLarge(
                    "a screenshot attachment is limited to 10 MB".into(),
                ));
            }
            let filename = spec
                .filename
                .clone()
                .unwrap_or_else(|| "screenshot.png".into());
            let mime = spec.mime.clone();
            let guessed = filename.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            let normalized = mime
                .as_deref()
                .map(|m| m.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| format!("image/{guessed}"));
            if !artifacts::is_raster_mime(&normalized) {
                return Err(AppError::unsupported_media_type(
                    "a screenshot must be png, jpeg, gif or webp; svg and other types are refused",
                ));
            }
            out.push(DecodedAttachment {
                filename,
                mime: Some(normalized),
                bytes,
            });
        }
    }
    Ok(out)
}

/// Validate a referenced artifact before linking: it must belong to the same
/// issue, be a screenshot, and still be live.
async fn validate_existing_artifacts(
    db: &SqlitePool,
    issue_id: Option<i64>,
    specs: &[ApprovalAttachmentSpec],
) -> AppResult<()> {
    for spec in specs {
        let Some(artifact_id) = spec.artifact_id else {
            continue;
        };
        let Some(issue_id) = issue_id else {
            return Err(AppError::bad_request(
                "an artifact attachment needs an ask linked to an issue",
            ));
        };
        let row: Option<(i64, String, String)> = sqlx::query_as(
            "SELECT issue_id, kind, expires_at FROM issue_artifacts WHERE id = ?1",
        )
        .bind(artifact_id)
        .fetch_optional(db)
        .await?;
        let Some((owner_issue, kind, expires_at)) = row else {
            return Err(AppError::NotFound);
        };
        if owner_issue != issue_id {
            return Err(AppError::bad_request(
                "artifact_id does not belong to this issue",
            ));
        }
        if kind != breadboard_core::ArtifactKind::Screenshot.as_str() {
            return Err(AppError::bad_request(
                "only screenshot artifacts can be attached to an ask",
            ));
        }
        if parse_ts(&expires_at) <= Utc::now() {
            return Err(AppError::Expired);
        }
    }
    Ok(())
}

async fn link_attachment(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    ask_id: i64,
    artifact_id: i64,
    caption: &str,
    sort: i64,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO approval_attachments (ask_id, artifact_id, caption, sort, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (ask_id, artifact_id) DO UPDATE SET caption = excluded.caption,
             sort = excluded.sort",
    )
    .bind(ask_id)
    .bind(artifact_id)
    .bind(caption)
    .bind(sort)
    .bind(now_rfc3339())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

type CountRow = (Option<i64>, i64, Option<i64>, Option<i64>, Option<i64>, Option<i64>, Option<i64>);

/// Store base64 screenshots through the existing artifact pipeline and link
/// them. Existing artifact ids are linked directly.
#[allow(clippy::too_many_arguments)]
async fn store_and_link_attachments(
    app: &AppState,
    user: &AuthUser,
    ask_id: i64,
    issue: Option<&Issue>,
    agent_name: &str,
    session_id: Option<&str>,
    specs: &[ApprovalAttachmentSpec],
    decoded: &[DecodedAttachment],
    start_sort: i64,
) -> AppResult<()> {
    let mut decoded = decoded.iter();
    for (sort, spec) in (start_sort..).zip(specs) {
        let artifact_id = if let Some(artifact_id) = spec.artifact_id {
            artifact_id
        } else {
            let Some(issue) = issue else {
                return Err(AppError::bad_request(
                    "a base64 attachment needs an ask linked to an issue",
                ));
            };
            let Some(upload) = decoded.next() else {
                return Err(AppError::bad_request("attachment data is missing"));
            };
            let saved = artifacts::save_artifact(
                app,
                user,
                &issue.key,
                UploadInput {
                    filename: upload.filename.clone(),
                    mime: upload.mime.clone(),
                    bytes: upload.bytes.clone(),
                    kind: breadboard_core::ArtifactKind::Screenshot,
                    comment_id: None,
                    review_ref: Some(format!("approval:{ask_id}")),
                    agent_name: Some(agent_name.to_string()),
                    session_id: session_id.map(str::to_string),
                },
            )
            .await?;
            saved.id
        };
        let caption = spec.caption.clone().unwrap_or_default();
        let mut tx = crate::db::begin_write(&app.db).await?;
        link_attachment(&mut tx, ask_id, artifact_id, &caption, sort).await?;
        tx.commit().await?;
    }
    Ok(())
}

// --- windows --------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct WindowRow {
    id: i64,
    ask_id: Option<i64>,
    project_id: Option<i64>,
    project_key: Option<String>,
    agent_name: String,
    title: String,
    starts_at: String,
    ends_at: String,
    timezone: String,
    services_json: String,
    reason: String,
    risks: String,
    rollback: String,
    runbook_md: String,
    status: String,
    decided_by: Option<i64>,
    decided_at: Option<String>,
    created_at: String,
    updated_at: String,
}

impl From<WindowRow> for MaintenanceWindow {
    fn from(row: WindowRow) -> Self {
        Self {
            id: row.id,
            ask_id: row.ask_id,
            project_id: row.project_id,
            project_key: row.project_key,
            agent_name: row.agent_name,
            title: row.title,
            starts_at: parse_ts(&row.starts_at),
            ends_at: parse_ts(&row.ends_at),
            timezone: row.timezone,
            services: parse_string_list(&row.services_json),
            reason: row.reason,
            risks: row.risks,
            rollback: row.rollback,
            runbook_md: row.runbook_md,
            status: MaintenanceStatus::parse(&row.status).unwrap_or(MaintenanceStatus::Proposed),
            decided_by: row.decided_by,
            decided_at: row.decided_at.as_deref().map(parse_ts),
            created_at: parse_ts(&row.created_at),
            updated_at: parse_ts(&row.updated_at),
        }
    }
}

const WINDOW_SELECT: &str = "SELECT w.id, w.ask_id, w.project_id, p.key AS project_key, \
    w.agent_name, w.title, w.starts_at, w.ends_at, w.timezone, w.services_json, w.reason, \
    w.risks, w.rollback, w.runbook_md, w.status, w.decided_by, w.decided_at, w.created_at, \
    w.updated_at FROM maintenance_windows w LEFT JOIN projects p ON p.id = w.project_id";

async fn windows_for(db: &SqlitePool, ids: &[i64]) -> AppResult<Vec<MaintenanceWindow>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(WINDOW_SELECT_WHERE);
    let mut separated = qb.separated(", ");
    for id in ids {
        separated.push_bind(*id);
    }
    separated.push_unseparated(") ORDER BY w.id");
    let rows: Vec<WindowRow> = qb.build_query_as().fetch_all(db).await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

const WINDOW_SELECT_WHERE: &str = "SELECT w.id, w.ask_id, w.project_id, p.key AS project_key, \
    w.agent_name, w.title, w.starts_at, w.ends_at, w.timezone, w.services_json, w.reason, \
    w.risks, w.rollback, w.runbook_md, w.status, w.decided_by, w.decided_at, w.created_at, \
    w.updated_at FROM maintenance_windows w LEFT JOIN projects p ON p.id = w.project_id \
    WHERE w.ask_id IN (";

async fn get_window(db: &SqlitePool, id: i64) -> AppResult<MaintenanceWindow> {
    let row: Option<WindowRow> = sqlx::query_as(&format!("{WINDOW_SELECT} WHERE w.id = ?1"))
        .bind(id)
        .fetch_optional(db)
        .await?;
    row.map(Into::into).ok_or(AppError::NotFound)
}

async fn insert_window(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    ask_id: Option<i64>,
    project_id: Option<i64>,
    agent_name: &str,
    title: &str,
    new: &NewMaintenanceWindow,
) -> AppResult<i64> {
    let ts = now_rfc3339();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO maintenance_windows
            (ask_id, project_id, agent_name, title, starts_at, ends_at, timezone,
             services_json, reason, risks, rollback, runbook_md, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'proposed', ?13, ?13)
         RETURNING id",
    )
    .bind(ask_id)
    .bind(project_id)
    .bind(agent_name)
    .bind(title)
    .bind(new.starts_at.to_rfc3339())
    .bind(new.ends_at.to_rfc3339())
    .bind(
        new.timezone
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("UTC"),
    )
    .bind(list_json(&new.services))
    .bind(&new.reason)
    .bind(&new.risks)
    .bind(&new.rollback)
    .bind(&new.runbook_md)
    .bind(&ts)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

fn merged_window(
    current: &MaintenanceWindow,
    patch: &MaintenanceWindowPatch,
) -> (DateTime<Utc>, DateTime<Utc>, Vec<String>, String, String, String, String) {
    (
        patch.starts_at.unwrap_or(current.starts_at),
        patch.ends_at.unwrap_or(current.ends_at),
        patch
            .services
            .clone()
            .unwrap_or_else(|| current.services.clone()),
        patch
            .reason
            .clone()
            .unwrap_or_else(|| current.reason.clone()),
        patch.risks.clone().unwrap_or_else(|| current.risks.clone()),
        patch
            .rollback
            .clone()
            .unwrap_or_else(|| current.rollback.clone()),
        patch
            .runbook_md
            .clone()
            .unwrap_or_else(|| current.runbook_md.clone()),
    )
}

fn validate_merged_window(
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    services: &[String],
    reason: &str,
    risks: &str,
    rollback: &str,
    runbook: &str,
) -> AppResult<()> {
    let probe = NewMaintenanceWindow {
        project: None,
        title: None,
        starts_at,
        ends_at,
        timezone: None,
        services: services.to_vec(),
        reason: reason.to_string(),
        risks: risks.to_string(),
        rollback: rollback.to_string(),
        runbook_md: runbook.to_string(),
    };
    probe.validate().map_err(AppError::bad_request)
}

async fn apply_window_patch(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    current: &MaintenanceWindow,
    patch: &MaintenanceWindowPatch,
) -> AppResult<()> {
    let (starts_at, ends_at, services, reason, risks, rollback, runbook) =
        merged_window(current, patch);
    validate_merged_window(
        starts_at,
        ends_at,
        &services,
        &reason,
        &risks,
        &rollback,
        &runbook,
    )?;
    sqlx::query(
        "UPDATE maintenance_windows SET
            title = ?2, starts_at = ?3, ends_at = ?4, timezone = ?5, services_json = ?6,
            reason = ?7, risks = ?8, rollback = ?9, runbook_md = ?10, updated_at = ?11
         WHERE id = ?1",
    )
    .bind(current.id)
    .bind(
        patch
            .title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&current.title),
    )
    .bind(starts_at.to_rfc3339())
    .bind(ends_at.to_rfc3339())
    .bind(
        patch
            .timezone
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&current.timezone),
    )
    .bind(list_json(&services))
    .bind(&reason)
    .bind(&risks)
    .bind(&rollback)
    .bind(&runbook)
    .bind(now_rfc3339())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

// --- read path ------------------------------------------------------------

async fn hydrate(
    db: &SqlitePool,
    mut asks: Vec<ApprovalAsk>,
    viewer: Option<i64>,
    agent_viewer: Option<&str>,
) -> AppResult<Vec<ApprovalAsk>> {
    if asks.is_empty() {
        return Ok(asks);
    }
    let ids: Vec<i64> = asks.iter().map(|a| a.id).collect();
    let events = events_for(db, &ids).await?;
    let attachments = attachments_for(db, &ids).await?;
    let windows = windows_for(db, &ids).await?;

    let mut events_by: HashMap<i64, Vec<ApprovalEvent>> = HashMap::new();
    for event in events {
        events_by.entry(event.ask_id).or_default().push(event);
    }
    let mut attachments_by: HashMap<i64, Vec<ApprovalAttachment>> = HashMap::new();
    for attachment in attachments {
        attachments_by
            .entry(attachment.ask_id)
            .or_default()
            .push(attachment);
    }
    let mut windows_by: HashMap<i64, MaintenanceWindow> = HashMap::new();
    for window in windows {
        if let Some(ask_id) = window.ask_id {
            windows_by.insert(ask_id, window);
        }
    }

    let mut cursors: HashMap<i64, i64> = HashMap::new();
    if let Some(user_id) = viewer {
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT ask_id, last_seen_event_id FROM approval_reads WHERE user_id = ?1",
        )
        .bind(user_id)
        .fetch_all(db)
        .await?;
        cursors.extend(rows);
    }
    let mut agent_cursors: HashMap<i64, i64> = HashMap::new();
    if let Some(agent) = agent_viewer {
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT ask_id, last_seen_event_id FROM approval_agent_reads
              WHERE replace(lower(agent_name), ' ', '-') = ?1",
        )
        .bind(normalize_agent(agent))
        .fetch_all(db)
        .await?;
        agent_cursors.extend(rows);
    }

    for ask in &mut asks {
        ask.events = events_by.remove(&ask.id).unwrap_or_default();
        ask.attachments = attachments_by.remove(&ask.id).unwrap_or_default();
        ask.window = windows_by.remove(&ask.id);
        if agent_viewer.is_some() {
            // The agent marker is about decisions: an approved/declined/
            // changes/info/expired event newer than its cursor. Comments and
            // its own revisions do not count as unread work.
            let newest_decision = ask
                .events
                .iter()
                .rev()
                .find(|event| is_decision_event(event.kind))
                .map(|event| event.id)
                .unwrap_or(0);
            let seen = agent_cursors.get(&ask.id).copied().unwrap_or(0);
            ask.unread = newest_decision > seen;
        } else if viewer.is_some() && ask.status == ApprovalStatus::Pending {
            let newest = ask.events.last().map(|e| e.id).unwrap_or(0);
            let seen = cursors.get(&ask.id).copied().unwrap_or(0);
            ask.unread = newest > seen;
        }
    }
    Ok(asks)
}

fn is_decision_event(kind: ApprovalEventKind) -> bool {
    matches!(
        kind,
        ApprovalEventKind::Approved
            | ApprovalEventKind::Declined
            | ApprovalEventKind::Changes
            | ApprovalEventKind::Info
            | ApprovalEventKind::Expired
    )
}

async fn get_row(db: &SqlitePool, id: i64) -> AppResult<ApprovalAsk> {
    let row: Option<AskRow> = sqlx::query_as(&format!("{SELECT} WHERE a.id = ?1"))
        .bind(id)
        .fetch_optional(db)
        .await?;
    row.map(AskRow::into_ask).ok_or(AppError::NotFound)
}

pub async fn get_ask(
    app: &AppState,
    id: i64,
    viewer: Option<i64>,
    agent_viewer: Option<&str>,
) -> AppResult<ApprovalAsk> {
    // Every lazy read expires through the notifying path, so opening the page
    // cannot swallow an expiry ping before the hourly sweep reaches it.
    expire_due(app).await?;
    let ask = get_row(&app.db, id).await?;
    let mut asks = hydrate(&app.db, vec![ask], viewer, agent_viewer).await?;
    asks.pop().ok_or(AppError::NotFound)
}

/// Expire every pending ask whose one-shot window has lapsed, appending an
/// audit event for each. Returns the number expired.
pub async fn expire_due_rows(db: &SqlitePool, now: DateTime<Utc>) -> AppResult<Vec<i64>> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM approval_asks
          WHERE status = 'pending' AND expires_at IS NOT NULL AND expires_at <= ?1",
    )
    .bind(now.to_rfc3339())
    .fetch_all(db)
    .await?;
    for id in &ids {
        let mut tx = crate::db::begin_write(db).await?;
        let updated = sqlx::query(
            "UPDATE approval_asks
                SET status = 'expired', updated_at = ?2, decided_at = NULL
              WHERE id = ?1 AND status = 'pending'",
        )
        .bind(id)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            tx.commit().await?;
            continue;
        }
        let revision: i64 =
            sqlx::query_scalar("SELECT revision FROM approval_asks WHERE id = ?1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        let mut meta = BTreeMap::new();
        meta.insert("expired_at".into(), now.to_rfc3339());
        meta.insert("revision".into(), revision.to_string());
        insert_event(
            &mut tx,
            *id,
            ApprovalEventKind::Expired,
            ApprovalActorKind::System,
            "system",
            "The one-shot approval expired before it was decided.",
            &meta,
        )
        .await?;
        tx.commit().await?;
    }
    Ok(ids)
}

pub async fn expire_due(app: &AppState) -> AppResult<usize> {
    let expired = expire_due_rows(&app.db, Utc::now()).await?;
    for id in &expired {
        publish(app, *id, "expired");
        notify_expired(app, *id).await;
    }
    Ok(expired.len())
}

async fn notify_expired(app: &AppState, ask_id: i64) {
    if let Ok(Some(event_id)) = sqlx::query_scalar(
        "SELECT id FROM approval_events
          WHERE ask_id = ?1 AND kind = 'expired'
          ORDER BY id DESC LIMIT 1",
    )
    .bind(ask_id)
    .fetch_optional(&app.db)
    .await
    {
        notify_transition(app, ask_id, event_id, "expired").await;
    }
}

/// Hourly sweep so an expiry is recorded even when nobody opens the page.
pub fn spawn_expiry_sweep(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let Err(error) = expire_due(&state).await {
                tracing::warn!(%error, "approval expiry sweep failed");
            }
        }
    });
}

/// Gotify delivery for an ask transition. Only high-urgency asks and asks on
/// an issue labelled `incident` are sent. Delivery is recorded per
/// (ask, event) before the request, so a transition is at-most-once. Revisions,
/// comments and re-reads never call this.
pub async fn notify_transition(app: &AppState, ask_id: i64, event_id: i64, kind: &str) {
    let cfg = &app.config.approvals.notifications;
    if !cfg.usable() {
        return;
    }
    let Ok(ask) = get_row(&app.db, ask_id).await else {
        return;
    };
    let issue = match ask.issue_id {
        Some(issue_id) => store::get_issue(&app.db, issue_id).await.ok(),
        None => None,
    };
    let incident = issue
        .as_ref()
        .map(|issue| issue.labels.iter().any(|label| label.eq_ignore_ascii_case("incident")))
        .unwrap_or(false);
    if ask.urgency != ApprovalUrgency::High && !incident {
        return;
    }
    if !ask_notification_recipients(app).await {
        return;
    }

    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO approval_notifications (ask_id, event_id, kind, created_at)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(ask_id)
    .bind(event_id)
    .bind(kind)
    .bind(now_rfc3339())
    .execute(&app.db)
    .await;
    match inserted {
        Ok(result) if result.rows_affected() > 0 => {}
        _ => return,
    }

    let issue_label = ask
        .issue_key
        .clone()
        .or_else(|| ask.project_key.clone())
        .unwrap_or_else(|| "approvals".into());
    let title = format!("[{}] {}", cfg.app.trim(), ask.title);
    let message = if kind == "expired" {
        format!(
            "Expired without a decision: {issue_label}. {}. Open Approvals to open a new ask.",
            ask.agent_name
        )
    } else {
        format!(
            "{} needs a decision on {issue_label}: {}. Open Approvals to approve, decline or send it back.",
            ask.agent_name,
            if incident { "incident" } else { "high urgency" }
        )
    };
    send_gotify(app, &title, &message).await;
}

/// Whether at least one operator still wants approval pings. The opt-out is
/// the `approvals` notification preference; the in-app badge is not affected.
async fn ask_notification_recipients(app: &AppState) -> bool {
    let mut users: Vec<i64> = Vec::new();
    if app.config.approvals.operators.is_empty() {
        if let Ok(ids) = sqlx::query_scalar::<_, i64>("SELECT id FROM users")
            .fetch_all(&app.db)
            .await
        {
            users.extend(ids);
        }
    } else {
        for login in &app.config.approvals.operators {
            if let Ok(Some(id)) = store::find_user_by_login(&app.db, login).await {
                users.push(id);
            }
        }
    }
    for user_id in users {
        if let Ok(prefs) = store::get_prefs(&app.db, user_id).await {
            if prefs.approvals {
                return true;
            }
        }
    }
    false
}

/// Post one message to the configured Gotify application. The token travels in
/// the `X-Gotify-Key` header, never the URL, so a proxy or an error log cannot
/// capture it. The bounded timeout keeps a dead Gotify from holding a request
/// open, and every failure is logged with a classification, not the raw
/// reqwest error (which embeds the URL).
async fn send_gotify(app: &AppState, title: &str, message: &str) {
    let cfg = &app.config.approvals.notifications;
    let endpoint = format!("{}/message", cfg.url.trim_end_matches('/'));
    let payload = serde_json::json!({
        "title": title,
        "message": message,
        "priority": 8,
    });
    let sent = app
        .http
        .post(&endpoint)
        .header("X-Gotify-Key", cfg.token.trim())
        .timeout(std::time::Duration::from_secs(2))
        .json(&payload)
        .send()
        .await;
    match sent {
        Ok(response) if response.status().is_success() => {}
        Ok(response) => {
            tracing::warn!(
                status = response.status().as_u16(),
                "approval Gotify send refused"
            );
        }
        Err(error) => {
            tracing::warn!(
                failure = gotify_error_class(&error),
                "approval Gotify send failed"
            );
        }
    }
}

/// A short, non-sensitive classification. Never include the URL, the token or
/// the raw error display: reqwest embeds the request URL in its message.
fn gotify_error_class(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_body() {
        "body"
    } else if error.is_decode() {
        "decode"
    } else {
        "request"
    }
}

pub fn publish(app: &AppState, id: i64, status: &str) {
    let event = BoardEvent::ApprovalUpdated {
        id,
        status: status.to_string(),
    };
    app.bus.publish(event.clone());
    crate::notify::emit_bread(app, &event);
}

// --- list ----------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ApprovalQuery {
    /// Inbox chip: needs, all, approval, maintenance, info, changes, answered.
    pub filter: Option<String>,
    pub status: Option<String>,
    pub kind: Option<String>,
    pub project: Option<String>,
    pub issue: Option<String>,
    pub urgency: Option<String>,
    pub agent: Option<String>,
    pub unread: Option<bool>,
    pub q: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub async fn list_asks(
    app: &AppState,
    query: &ApprovalQuery,
    viewer: Option<i64>,
    agent_viewer: Option<&str>,
) -> AppResult<Vec<ApprovalAsk>> {
    let db = &app.db;
    expire_due(app).await?;
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(SELECT);
    qb.push(" WHERE 1 = 1");

    if let Some(filter) = query.filter.as_deref() {
        match filter {
            "needs" => {
                qb.push(" AND a.status = 'pending'");
            }
            "changes" => {
                qb.push(" AND a.status = 'changes'");
            }
            "answered" => {
                qb.push(" AND a.status IN ('approved', 'declined', 'answered')");
            }
            "approval" | "maintenance" | "info" => {
                qb.push(" AND a.kind = ").push_bind(filter.to_string());
            }
            _ => {}
        }
    }
    if let Some(status) = query.status.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND a.status = ").push_bind(status.trim().to_string());
    }
    if let Some(kind) = query.kind.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND a.kind = ").push_bind(kind.trim().to_string());
    }
    if let Some(project) = query.project.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND (p.key = ")
            .push_bind(project.trim().to_uppercase())
            .push(" OR a.project_id = ")
            .push_bind(project.trim().parse::<i64>().unwrap_or(-1))
            .push(")");
    }
    if let Some(issue) = query.issue.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND i.key = ")
            .push_bind(issue.trim().to_uppercase());
    }
    if let Some(urgency) = query.urgency.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND a.urgency = ")
            .push_bind(urgency.trim().to_lowercase());
    }
    if let Some(agent) = query.agent.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND replace(lower(a.agent_name), ' ', '-') = ")
            .push_bind(normalize_agent(agent));
    }
    if let Some(term) = query.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let like = format!("%{}%", term.to_lowercase());
        qb.push(" AND (lower(a.title) LIKE ")
            .push_bind(like.clone())
            .push(" OR lower(a.summary) LIKE ")
            .push_bind(like.clone())
            .push(" OR lower(a.body_md) LIKE ")
            .push_bind(like)
            .push(")");
    }
    if query.unread.unwrap_or(false) {
        let Some(user_id) = viewer else {
            qb.push(" AND 0");
            let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
            qb.push(" ORDER BY a.created_at DESC, a.id DESC LIMIT ")
                .push_bind(limit);
            let rows: Vec<AskRow> = qb.build_query_as().fetch_all(db).await?;
            return hydrate(
                db,
                rows.into_iter().map(AskRow::into_ask).collect(),
                viewer,
                agent_viewer,
            )
            .await;
        };
        qb.push(
            " AND a.status = 'pending' AND (
                NOT EXISTS (SELECT 1 FROM approval_reads r WHERE r.ask_id = a.id AND r.user_id = ",
        )
        .push_bind(user_id)
        .push(
            ")
                OR (SELECT MAX(e.id) FROM approval_events e WHERE e.ask_id = a.id)
                   > (SELECT r.last_seen_event_id FROM approval_reads r
                       WHERE r.ask_id = a.id AND r.user_id = ",
        )
        .push_bind(user_id)
        .push("))");
    }

    qb.push(" ORDER BY CASE a.status WHEN 'pending' THEN 0 ELSE 1 END, a.created_at DESC, a.id DESC");

    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = query.offset.unwrap_or(0).max(0);
    qb.push(" LIMIT ").push_bind(limit);
    qb.push(" OFFSET ").push_bind(offset);

    let rows: Vec<AskRow> = qb.build_query_as().fetch_all(db).await?;
    hydrate(
        db,
        rows.into_iter().map(AskRow::into_ask).collect(),
        viewer,
        agent_viewer,
    )
    .await
}

pub async fn counts(app: &AppState, viewer: Option<i64>) -> AppResult<ApprovalCounts> {
    let db = &app.db;
    expire_due(app).await?;
    let (needs, all, approval, maintenance, info, changes, answered) = sqlx::query_as(
        "SELECT
            SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END),
            COUNT(*),
            SUM(CASE WHEN kind = 'approval' THEN 1 ELSE 0 END),
            SUM(CASE WHEN kind = 'maintenance' THEN 1 ELSE 0 END),
            SUM(CASE WHEN kind = 'info' THEN 1 ELSE 0 END),
            SUM(CASE WHEN status = 'changes' THEN 1 ELSE 0 END),
            SUM(CASE WHEN status IN ('approved', 'declined', 'answered') THEN 1 ELSE 0 END)
         FROM approval_asks",
    )
    .fetch_one(db)
    .await
    .map(|(a, b, c, d, e, f, g): CountRow| {
        (
            a.unwrap_or(0),
            b,
            c.unwrap_or(0),
            d.unwrap_or(0),
            e.unwrap_or(0),
            f.unwrap_or(0),
            g.unwrap_or(0),
        )
    })?;

    let unread = match viewer {
        Some(user_id) => sqlx::query_scalar(
            "SELECT COUNT(*) FROM approval_asks a
              WHERE a.status = 'pending' AND (
                NOT EXISTS (SELECT 1 FROM approval_reads r
                             WHERE r.ask_id = a.id AND r.user_id = ?1)
                OR (SELECT MAX(e.id) FROM approval_events e WHERE e.ask_id = a.id)
                   > (SELECT r.last_seen_event_id FROM approval_reads r
                       WHERE r.ask_id = a.id AND r.user_id = ?1))",
        )
        .bind(user_id)
        .fetch_one(db)
        .await?,
        None => 0,
    };

    Ok(ApprovalCounts {
        needs,
        unread,
        all,
        approval,
        maintenance,
        info,
        changes,
        answered,
    })
}

/// Advance an agent's decision cursor to the newest event on the ask.
pub async fn mark_agent_read(db: &SqlitePool, ask_id: i64, agent_name: &str) -> AppResult<()> {
    let newest: Option<i64> =
        sqlx::query_scalar("SELECT MAX(id) FROM approval_events WHERE ask_id = ?1")
            .bind(ask_id)
            .fetch_one(db)
            .await?;
    sqlx::query(
        "INSERT INTO approval_agent_reads
            (ask_id, agent_name, last_seen_event_id, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (ask_id, agent_name) DO UPDATE SET
            last_seen_event_id =
                MAX(approval_agent_reads.last_seen_event_id, excluded.last_seen_event_id),
            updated_at = excluded.updated_at",
    )
    .bind(ask_id)
    .bind(normalize_agent(agent_name))
    .bind(newest.unwrap_or(0))
    .bind(now_rfc3339())
    .execute(db)
    .await?;
    Ok(())
}

/// Advance an operator's read cursor to the newest event on the ask.
pub async fn mark_read(db: &SqlitePool, ask_id: i64, user_id: i64) -> AppResult<()> {
    let newest: Option<i64> =
        sqlx::query_scalar("SELECT MAX(id) FROM approval_events WHERE ask_id = ?1")
            .bind(ask_id)
            .fetch_one(db)
            .await?;
    sqlx::query(
        "INSERT INTO approval_reads (ask_id, user_id, last_seen_event_id, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (ask_id, user_id) DO UPDATE SET
            last_seen_event_id = MAX(approval_reads.last_seen_event_id, excluded.last_seen_event_id),
            updated_at = excluded.updated_at",
    )
    .bind(ask_id)
    .bind(user_id)
    .bind(newest.unwrap_or(0))
    .bind(now_rfc3339())
    .execute(db)
    .await?;
    Ok(())
}

// --- write path -----------------------------------------------------------

pub async fn create_ask(
    app: &AppState,
    user: &User,
    new: &NewApprovalAsk,
) -> AppResult<ApprovalAsk> {
    new.validate().map_err(AppError::bad_request)?;
    let (agent_name, session_id) =
        resolve_agent(app, user, &new.agent_name, new.session_id.as_deref()).await?;

    let issue = match new.issue.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(key) => Some(store::get_issue_by_key(&app.db, key).await?),
        None => None,
    };
    let explicit_project = match new.project.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(value) => Some(store::find_project(&app.db, value).await?),
        None => None,
    };
    if let (Some(issue), Some(project)) = (&issue, &explicit_project) {
        if issue.project_id != project.id {
            return Err(AppError::bad_request(
                "issue does not belong to the requested project",
            ));
        }
    }
    let window_project = match new.window.as_ref().and_then(|w| w.project.as_deref()) {
        Some(value) => Some(store::find_project(&app.db, value).await?),
        None => None,
    };
    let project_id = issue
        .as_ref()
        .map(|i| i.project_id)
        .or_else(|| explicit_project.as_ref().map(|p| p.id))
        .or_else(|| window_project.as_ref().map(|p| p.id));

    let kind = new.kind.unwrap_or(if new.window.is_some() {
        ApprovalKind::Maintenance
    } else {
        ApprovalKind::Approval
    });
    if kind == ApprovalKind::Maintenance && new.window.is_none() {
        return Err(AppError::bad_request(
            "a maintenance ask needs a proposed window",
        ));
    }
    if kind != ApprovalKind::Maintenance && new.window.is_some() {
        return Err(AppError::bad_request(
            "only a maintenance ask can carry a window",
        ));
    }

    validate_existing_artifacts(&app.db, issue.as_ref().map(|i| i.id), &new.attachments).await?;
    let decoded = decode_attachments(&new.attachments)?;

    let now = now_rfc3339();
    let title = new.title.trim().to_string();
    let mut tx = crate::db::begin_write(&app.db).await?;
    let ask_id: i64 = sqlx::query_scalar(
        "INSERT INTO approval_asks
            (project_id, issue_id, agent_name, session_id, kind, urgency, title, summary,
             body_md, environment, status, revision, checklist_json, risk_md, rollback_md,
             expires_at, deadline, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'pending', 1, ?11, ?12, ?13, ?14,
                 ?15, ?16, ?16)
         RETURNING id",
    )
    .bind(project_id)
    .bind(issue.as_ref().map(|i| i.id))
    .bind(&agent_name)
    .bind(&session_id)
    .bind(kind.as_str())
    .bind(new.urgency.unwrap_or(ApprovalUrgency::Normal).as_str())
    .bind(&title)
    .bind(&new.summary)
    .bind(&new.body_md)
    .bind(new.environment.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(list_json(&new.checklist))
    .bind(new.risk_md.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(new.rollback_md.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(new.expires_at.map(|t| t.to_rfc3339()))
    .bind(new.deadline.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(&now)
    .fetch_one(&mut *tx)
    .await?;

    let mut meta = BTreeMap::new();
    meta.insert("revision".into(), "1".into());
    meta.insert("kind".into(), kind.as_str().into());
    let created_event = insert_event(
        &mut tx,
        ask_id,
        ApprovalEventKind::Created,
        ApprovalActorKind::Agent,
        &agent_name,
        &new.summary,
        &meta,
    )
    .await?;

    if let Some(window) = &new.window {
        let window_title = window
            .title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&title);
        insert_window(
            &mut tx,
            Some(ask_id),
            project_id,
            &agent_name,
            window_title,
            window,
        )
        .await?;
    }
    tx.commit().await?;

    if !new.attachments.is_empty() {
        let linked = store_and_link_attachments(
            app,
            &AuthUser(user.clone()),
            ask_id,
            issue.as_ref(),
            &agent_name,
            session_id.as_deref(),
            &new.attachments,
            &decoded,
            0,
        )
        .await;
        if let Err(error) = linked {
            // The ask without its evidence would look complete when it is not.
            // Remove it so the agent retries with a clean create.
            sqlx::query("DELETE FROM approval_asks WHERE id = ?1")
                .bind(ask_id)
                .execute(&app.db)
                .await
                .ok();
            return Err(error);
        }
    }

    let ask = get_row(&app.db, ask_id).await?;
    publish(app, ask.id, ask.status.as_str());
    notify_transition(app, ask_id, created_event, "created").await;
    get_ask(app, ask_id, None, None).await
}

pub async fn revise_ask(
    app: &AppState,
    user: &User,
    id: i64,
    revision: &ApprovalRevision,
) -> AppResult<ApprovalAsk> {
    revision.validate().map_err(AppError::bad_request)?;
    let current = get_ask(app, id, None, None).await?;
    let (agent_name, session_id) =
        resolve_agent(app, user, &revision.agent_name, revision.session_id.as_deref()).await?;
    if !agent_matches(&current.agent_name, &agent_name) {
        return Err(AppError::forbidden_msg(
            "only the agent that opened this ask can revise it",
        ));
    }
    expire_due(app).await?;
    let current = get_ask(app, id, None, None).await?;
    if !current.status.is_open_for_agent() {
        return Err(AppError::Conflict(format!(
            "this ask is {} and cannot be revised",
            current.status.as_str()
        )));
    }

    if let Some(title) = &revision.title {
        if title.trim().is_empty() {
            return Err(AppError::bad_request("title must not be empty"));
        }
        if title.chars().count() > 300 {
            return Err(AppError::bad_request(
                "title must be 300 characters or fewer",
            ));
        }
    }
    if let Some(checklist) = &revision.checklist {
        if checklist.len() > breadboard_core::dto::MAX_APPROVAL_CHECKLIST {
            return Err(AppError::bad_request("checklist is too long"));
        }
    }
    if let Some(summary) = &revision.summary {
        if summary.chars().count() > 1000 {
            return Err(AppError::bad_request(
                "summary must be 1000 characters or fewer",
            ));
        }
    }
    if let Some(body) = &revision.body_md {
        if body.chars().count() > breadboard_core::dto::MAX_APPROVAL_BODY_CHARS {
            return Err(AppError::bad_request("body_md is too long"));
        }
    }
    validate_existing_artifacts(
        &app.db,
        current.issue_id,
        &revision.attachments,
    )
    .await?;
    let decoded = decode_attachments(&revision.attachments)?;

    // A revised window must still validate as a whole before anything commits.
    let window = current.window.clone();
    if let Some(patch) = &revision.window {
        let Some(current_window) = &window else {
            return Err(AppError::bad_request("this ask has no proposed window"));
        };
        let (starts_at, ends_at, services, reason, risks, rollback, runbook) =
            merged_window(current_window, patch);
        validate_merged_window(
            starts_at,
            ends_at,
            &services,
            &reason,
            &risks,
            &rollback,
            &runbook,
        )?;
    }

    // An explicitly supplied null clears the value; an omitted field keeps it.
    // Keeping that distinction here is what makes the MCP contract honest.
    let expires_at = match revision.expires_at {
        Some(value) => value.map(|t| t.to_rfc3339()),
        None => current.expires_at.map(|t| t.to_rfc3339()),
    };
    let deadline = match &revision.deadline {
        Some(value) => value
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_string),
        None => current.deadline.clone(),
    };

    let now = now_rfc3339();
    let next_revision = current.revision + 1;
    let mut tx = crate::db::begin_write(&app.db).await?;
    sqlx::query(
        "UPDATE approval_asks SET
            title = COALESCE(?2, title),
            summary = COALESCE(?3, summary),
            body_md = COALESCE(?4, body_md),
            urgency = COALESCE(?5, urgency),
            checklist_json = COALESCE(?6, checklist_json),
            risk_md = COALESCE(?7, risk_md),
            rollback_md = COALESCE(?8, rollback_md),
            expires_at = CASE WHEN ?9 THEN ?10 ELSE expires_at END,
            deadline = CASE WHEN ?11 THEN ?12 ELSE deadline END,
            revision = ?13,
            status = 'pending',
            updated_at = ?14
          WHERE id = ?1",
    )
    .bind(id)
    .bind(revision.title.as_deref().map(str::trim))
    .bind(revision.summary.as_deref())
    .bind(revision.body_md.as_deref())
    .bind(revision.urgency.map(|u| u.as_str()))
    .bind(revision.checklist.as_ref().map(|c| list_json(c)))
    .bind(revision.risk_md.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(revision.rollback_md.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(revision.expires_at.is_some())
    .bind(&expires_at)
    .bind(revision.deadline.is_some())
    .bind(&deadline)
    .bind(next_revision)
    .bind(&now)
    .execute(&mut *tx)
    .await?;

    if let (Some(patch), Some(current_window)) = (&revision.window, &window) {
        apply_window_patch(&mut tx, current_window, patch).await?;
    }

    let mut meta = BTreeMap::new();
    meta.insert("revision".into(), next_revision.to_string());
    meta.insert("previous_revision".into(), current.revision.to_string());
    insert_event(
        &mut tx,
        id,
        ApprovalEventKind::Revised,
        ApprovalActorKind::Agent,
        &agent_name,
        revision
            .summary
            .as_deref()
            .or(revision.body_md.as_deref())
            .unwrap_or("Revised and resubmitted with new evidence."),
        &meta,
    )
    .await?;
    tx.commit().await?;

    if !revision.attachments.is_empty() {
        let start = current.attachments.len() as i64;
        let issue = match current.issue_id {
            Some(issue_id) => Some(store::get_issue(&app.db, issue_id).await?),
            None => None,
        };
        store_and_link_attachments(
            app,
            &AuthUser(user.clone()),
            id,
            issue.as_ref(),
            &agent_name,
            session_id.as_deref(),
            &revision.attachments,
            &decoded,
            start,
        )
        .await?;
    }

    let ask = get_row(&app.db, id).await?;
    publish(app, id, ask.status.as_str());
    get_ask(app, id, None, None).await
}

pub async fn comment_on_ask(
    app: &AppState,
    user: &User,
    id: i64,
    comment: &ApprovalComment,
) -> AppResult<ApprovalAsk> {
    comment.validate().map_err(AppError::bad_request)?;
    let current = get_row(&app.db, id).await?;
    let (actor_kind, actor_name, session_id) = match comment
        .agent_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(name) => {
            let (agent_name, session_id) =
                resolve_agent(app, user, name, comment.session_id.as_deref()).await?;
            if !agent_matches(&current.agent_name, &agent_name) {
                return Err(AppError::forbidden_msg(
                    "only this ask's agent can comment as an agent",
                ));
            }
            (ApprovalActorKind::Agent, agent_name, session_id)
        }
        None => (ApprovalActorKind::Human, user.login.clone(), None),
    };
    if !current.status.is_open_for_agent() {
        return Err(AppError::Conflict(
            "this ask is closed; open a new ask instead".into(),
        ));
    }

    let mut tx = crate::db::begin_write(&app.db).await?;
    insert_event(
        &mut tx,
        id,
        ApprovalEventKind::Comment,
        actor_kind,
        &actor_name,
        &comment.body_md,
        &BTreeMap::from([("session_id".into(), session_id.unwrap_or_default())]),
    )
    .await?;
    sqlx::query("UPDATE approval_asks SET updated_at = ?2 WHERE id = ?1")
        .bind(id)
        .bind(now_rfc3339())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    publish(app, id, current.status.as_str());
    get_ask(app, id, None, None).await
}

pub async fn withdraw_ask(
    app: &AppState,
    user: &User,
    id: i64,
    withdrawal: &ApprovalWithdrawal,
) -> AppResult<ApprovalAsk> {
    let current = get_row(&app.db, id).await?;
    let (agent_name, _) =
        resolve_agent(app, user, &withdrawal.agent_name, withdrawal.session_id.as_deref()).await?;
    if !agent_matches(&current.agent_name, &agent_name) {
        return Err(AppError::forbidden_msg(
            "only the agent that opened this ask can withdraw it",
        ));
    }
    expire_due_rows(&app.db, Utc::now()).await?;
    let current = get_row(&app.db, id).await?;
    if !current.status.is_open_for_agent() {
        return Err(AppError::Conflict(format!(
            "this ask is {} and cannot be withdrawn",
            current.status.as_str()
        )));
    }

    let now = now_rfc3339();
    let mut tx = crate::db::begin_write(&app.db).await?;
    sqlx::query("UPDATE approval_asks SET status = 'withdrawn', updated_at = ?2 WHERE id = ?1")
        .bind(id)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    insert_event(
        &mut tx,
        id,
        ApprovalEventKind::Withdrawn,
        ApprovalActorKind::Agent,
        &agent_name,
        withdrawal.note.as_deref().unwrap_or("Withdrawn by the agent."),
        &BTreeMap::new(),
    )
    .await?;
    tx.commit().await?;
    publish(app, id, "withdrawn");
    get_ask(app, id, None, None).await
}

/// The operator's one-shot decision. Everything that can refuse does so before
/// the first write.
pub async fn decide(
    app: &AppState,
    user: &User,
    id: i64,
    input: &ApprovalDecisionInput,
) -> AppResult<ApprovalAsk> {
    input.validate().map_err(AppError::bad_request)?;
    require_operator(app, user, input.agent_name.as_deref())?;
    expire_due(app).await?;
    let current = get_ask(app, id, None, None).await?;
    if current.status != ApprovalStatus::Pending {
        return Err(AppError::Conflict(format!(
            "this ask is already {}; decisions are one-shot",
            current.status.as_str()
        )));
    }

    let mut patched_window = current.window.clone();
    if let Some(patch) = &input.window {
        let Some(window) = &current.window else {
            return Err(AppError::bad_request("this ask has no proposed window"));
        };
        let (starts_at, ends_at, services, reason, risks, rollback, runbook) =
            merged_window(window, patch);
        validate_merged_window(
            starts_at,
            ends_at,
            &services,
            &reason,
            &risks,
            &rollback,
            &runbook,
        )?;
        patched_window = Some(MaintenanceWindow {
            starts_at,
            ends_at,
            services,
            reason,
            risks,
            rollback,
            runbook_md: runbook,
            ..window.clone()
        });
    }

    let status = input.decision.status();
    let now = now_rfc3339();
    let note = input
        .note_md
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let mut tx = crate::db::begin_write(&app.db).await?;
    // The conditional update is the one-shot guard even if two decisions race.
    let updated = sqlx::query(
        "UPDATE approval_asks SET
            status = ?2, decided_by = ?3, decided_by_login = ?4, decided_at = ?5,
            decision_note_md = ?6, updated_at = ?5
          WHERE id = ?1 AND status = 'pending'",
    )
    .bind(id)
    .bind(status.as_str())
    .bind(user.id)
    .bind(&user.login)
    .bind(&now)
    .bind(note.as_deref())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        tx.rollback().await.ok();
        return Err(AppError::Conflict(
            "this ask was already decided; decisions are one-shot".into(),
        ));
    }

    if let (Some(patch), Some(window)) = (&input.window, &current.window) {
        apply_window_patch(&mut tx, window, patch).await?;
    }
    if let Some(window) = &patched_window {
        let window_status = if input.decision == ApprovalDecision::Approve {
            MaintenanceStatus::Approved
        } else {
            MaintenanceStatus::Proposed
        };
        sqlx::query(
            "UPDATE maintenance_windows SET status = ?2, decided_by = ?3, decided_at = ?4,
                    updated_at = ?4
              WHERE id = ?1",
        )
        .bind(window.id)
        .bind(window_status.as_str())
        .bind(user.id)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    }

    let mut meta = BTreeMap::new();
    meta.insert("decision".into(), input.decision.as_str().into());
    meta.insert("revision".into(), current.revision.to_string());
    if let Some(window) = &patched_window {
        meta.insert("window_id".into(), window.id.to_string());
        meta.insert("window_starts_at".into(), window.starts_at.to_rfc3339());
        meta.insert("window_ends_at".into(), window.ends_at.to_rfc3339());
    }
    insert_event(
        &mut tx,
        id,
        input.decision.event_kind(),
        ApprovalActorKind::Human,
        &user.login,
        note.as_deref().unwrap_or(""),
        &meta,
    )
    .await?;
    tx.commit().await?;

    publish(app, id, status.as_str());
    get_ask(app, id, None, None).await
}

pub async fn update_window(
    app: &AppState,
    user: &User,
    window_id: i64,
    agent_name: &str,
    session_id: Option<&str>,
    patch: &MaintenanceWindowPatch,
) -> AppResult<MaintenanceWindow> {
    if patch.is_empty() {
        return Err(AppError::bad_request("nothing to update"));
    }
    let window = get_window(&app.db, window_id).await?;
    let (agent_name, _) = resolve_agent(app, user, agent_name, session_id).await?;
    if !agent_matches(&window.agent_name, &agent_name) {
        return Err(AppError::forbidden_msg(
            "only the agent that proposed this window can change it",
        ));
    }
    if window.status != MaintenanceStatus::Proposed {
        return Err(AppError::Conflict(
            "an approved or finished window cannot be moved".into(),
        ));
    }
    if let Some(ask_id) = window.ask_id {
        let ask = get_row(&app.db, ask_id).await?;
        if !ask.status.is_open_for_agent() {
            return Err(AppError::Conflict(
                "the ask behind this window is closed".into(),
            ));
        }
    }
    let (starts_at, ends_at, services, reason, risks, rollback, runbook) =
        merged_window(&window, patch);
    validate_merged_window(
        starts_at,
        ends_at,
        &services,
        &reason,
        &risks,
        &rollback,
        &runbook,
    )?;

    let mut tx = crate::db::begin_write(&app.db).await?;
    apply_window_patch(&mut tx, &window, patch).await?;
    tx.commit().await?;
    if let Some(ask_id) = window.ask_id {
        publish(app, ask_id, "pending");
    }
    get_window(&app.db, window_id).await
}

// --- windows and freezes: read path --------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct WindowQuery {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub status: Option<String>,
    pub project: Option<String>,
    pub agent: Option<String>,
}

pub async fn list_windows(
    db: &SqlitePool,
    query: &WindowQuery,
) -> AppResult<Vec<MaintenanceWindow>> {
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(WINDOW_SELECT);
    qb.push(" WHERE 1 = 1");
    if let Some(from) = query.from {
        qb.push(" AND w.ends_at > ").push_bind(from.to_rfc3339());
    }
    if let Some(to) = query.to {
        qb.push(" AND w.starts_at < ").push_bind(to.to_rfc3339());
    }
    if let Some(status) = query.status.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND w.status = ")
            .push_bind(status.trim().to_string());
    }
    if let Some(project) = query.project.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND (p.key = ")
            .push_bind(project.trim().to_uppercase())
            .push(" OR w.project_id = ")
            .push_bind(project.trim().parse::<i64>().unwrap_or(-1))
            .push(")");
    }
    if let Some(agent) = query.agent.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND replace(lower(w.agent_name), ' ', '-') = ")
            .push_bind(normalize_agent(agent));
    }
    qb.push(" ORDER BY w.starts_at, w.id");
    let rows: Vec<WindowRow> = qb.build_query_as().fetch_all(db).await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

#[derive(Debug, Default, Deserialize)]
pub struct FreezeQuery {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub project: Option<String>,
}

#[derive(sqlx::FromRow)]
struct FreezeRow {
    id: i64,
    project_id: Option<i64>,
    project_key: Option<String>,
    label: String,
    starts_at: String,
    ends_at: String,
    created_at: String,
}

impl From<FreezeRow> for FreezePeriod {
    fn from(row: FreezeRow) -> Self {
        Self {
            id: row.id,
            project_id: row.project_id,
            project_key: row.project_key,
            label: row.label,
            starts_at: parse_ts(&row.starts_at),
            ends_at: parse_ts(&row.ends_at),
            created_at: parse_ts(&row.created_at),
        }
    }
}

pub async fn list_freezes(db: &SqlitePool, query: &FreezeQuery) -> AppResult<Vec<FreezePeriod>> {
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
        "SELECT f.id, f.project_id, p.key AS project_key, f.label, f.starts_at, f.ends_at, \
         f.created_at FROM maintenance_freezes f LEFT JOIN projects p ON p.id = f.project_id \
         WHERE 1 = 1",
    );
    if let Some(from) = query.from {
        qb.push(" AND f.ends_at > ").push_bind(from.to_rfc3339());
    }
    if let Some(to) = query.to {
        qb.push(" AND f.starts_at < ").push_bind(to.to_rfc3339());
    }
    if let Some(project) = query.project.as_deref().filter(|s| !s.trim().is_empty()) {
        qb.push(" AND (p.key = ")
            .push_bind(project.trim().to_uppercase())
            .push(" OR f.project_id = ")
            .push_bind(project.trim().parse::<i64>().unwrap_or(-1))
            .push(")");
    }
    qb.push(" ORDER BY f.starts_at, f.id");
    let rows: Vec<FreezeRow> = qb.build_query_as().fetch_all(db).await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn create_freeze(
    app: &AppState,
    user: &User,
    new: &NewFreezePeriod,
) -> AppResult<FreezePeriod> {
    new.validate().map_err(AppError::bad_request)?;
    require_operator(app, user, None)?;
    let project_id = match new
        .project
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(value) => Some(store::find_project(&app.db, value).await?.id),
        None => None,
    };
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO maintenance_freezes (project_id, label, starts_at, ends_at, created_by,
             created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING id",
    )
    .bind(project_id)
    .bind(new.label.trim())
    .bind(new.starts_at.to_rfc3339())
    .bind(new.ends_at.to_rfc3339())
    .bind(user.id)
    .bind(now_rfc3339())
    .fetch_one(&app.db)
    .await?;
    let row: FreezeRow = sqlx::query_as(
        "SELECT f.id, f.project_id, p.key AS project_key, f.label, f.starts_at, f.ends_at, \
         f.created_at FROM maintenance_freezes f LEFT JOIN projects p ON p.id = f.project_id \
         WHERE f.id = ?1",
    )
    .bind(id)
    .fetch_one(&app.db)
    .await?;
    Ok(row.into())
}

// --- REST handlers --------------------------------------------------------

pub async fn list(
    State(app): State<AppState>,
    user: AuthUser,
    Query(query): Query<ApprovalQuery>,
) -> AppResult<Json<Vec<ApprovalAsk>>> {
    let agent_view = query
        .agent
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let viewer = if agent_view.is_some() { None } else { Some(user.0.id) };
    // A list is a read: filter by a known agent only when the caller owns it.
    // Unknown names are allowed as a filter and simply match nothing, so a GET
    // never registers or claims an identity as a side effect.
    if let Some(agent) = agent_view {
        if let Some(owner) = agent_owner(&app.db, agent).await? {
            if owner != user.0.id {
                return Err(AppError::forbidden_msg(
                    "agent_name does not belong to this user",
                ));
            }
        }
    }
    Ok(Json(
        list_asks(&app, &query, viewer, agent_view).await?,
    ))
}

/// The user that owns an agent identity, if it is known at all.
async fn agent_owner(db: &SqlitePool, agent_name: &str) -> AppResult<Option<i64>> {
    let name = normalize_agent(agent_name);
    Ok(sqlx::query_scalar(
        "SELECT user_id FROM (
            SELECT user_id FROM agent_profiles
             WHERE replace(lower(agent_name), ' ', '-') = ?1
            UNION
            SELECT claimed_by AS user_id FROM agent_claims
             WHERE replace(lower(agent_name), ' ', '-') = ?1
            UNION
            SELECT user_id FROM agent_sessions
             WHERE replace(lower(agent_name), ' ', '-') = ?1 AND user_id IS NOT NULL
         ) ORDER BY user_id LIMIT 1",
    )
    .bind(&name)
    .fetch_optional(db)
    .await?)
}

pub async fn counts_handler(
    State(app): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<ApprovalCounts>> {
    Ok(Json(counts(&app, Some(user.0.id)).await?))
}

pub async fn get(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Query(query): Query<ApprovalQuery>,
) -> AppResult<Json<ApprovalAsk>> {
    let agent_view = query
        .agent
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(agent) = agent_view {
        if let Some(owner) = agent_owner(&app.db, agent).await? {
            if owner != user.0.id {
                return Err(AppError::forbidden_msg(
                    "agent_name does not belong to this user",
                ));
            }
        }
    }
    let viewer = if agent_view.is_some() { None } else { Some(user.0.id) };
    let ask = get_ask(&app, id, viewer, agent_view).await?;
    match agent_view {
        Some(agent) => mark_agent_read(&app.db, id, agent).await?,
        None => mark_read(&app.db, id, user.0.id).await?,
    }
    Ok(Json(ask))
}

/// The audit trail for one ask: every transition with actor, kind and time.
pub async fn audit(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Vec<ApprovalEvent>>> {
    get_row(&app.db, id).await?;
    Ok(Json(events_for(&app.db, &[id]).await?))
}

pub async fn create(
    State(app): State<AppState>,
    user: AuthUser,
    Json(body): Json<NewApprovalAsk>,
) -> AppResult<(StatusCode, Json<ApprovalAsk>)> {
    let ask = create_ask(&app, &user.0, &body).await?;
    Ok((StatusCode::CREATED, Json(ask)))
}

pub async fn revise(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<ApprovalRevision>,
) -> AppResult<Json<ApprovalAsk>> {
    Ok(Json(revise_ask(&app, &user.0, id, &body).await?))
}

pub async fn comment(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<ApprovalComment>,
) -> AppResult<(StatusCode, Json<ApprovalAsk>)> {
    let ask = comment_on_ask(&app, &user.0, id, &body).await?;
    Ok((StatusCode::CREATED, Json(ask)))
}

pub async fn decide_handler(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<ApprovalDecisionInput>,
) -> AppResult<Json<ApprovalAsk>> {
    Ok(Json(decide(&app, &user.0, id, &body).await?))
}

pub async fn withdraw_handler(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<ApprovalWithdrawal>,
) -> AppResult<Json<ApprovalAsk>> {
    Ok(Json(withdraw_ask(&app, &user.0, id, &body).await?))
}

#[derive(Deserialize)]
pub struct WindowUpdate {
    pub agent_name: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(flatten)]
    pub patch: MaintenanceWindowPatch,
}

pub async fn update_window_handler(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<WindowUpdate>,
) -> AppResult<Json<MaintenanceWindow>> {
    Ok(Json(
        update_window(
            &app,
            &user.0,
            id,
            &body.agent_name,
            body.session_id.as_deref(),
            &body.patch,
        )
        .await?,
    ))
}

pub async fn list_windows_handler(
    State(app): State<AppState>,
    _user: AuthUser,
    Query(query): Query<WindowQuery>,
) -> AppResult<Json<Vec<MaintenanceWindow>>> {
    Ok(Json(list_windows(&app.db, &query).await?))
}

pub async fn list_freezes_handler(
    State(app): State<AppState>,
    _user: AuthUser,
    Query(query): Query<FreezeQuery>,
) -> AppResult<Json<Vec<FreezePeriod>>> {
    Ok(Json(list_freezes(&app.db, &query).await?))
}

pub async fn create_freeze_handler(
    State(app): State<AppState>,
    user: AuthUser,
    Json(body): Json<NewFreezePeriod>,
) -> AppResult<(StatusCode, Json<FreezePeriod>)> {
    let freeze = create_freeze(&app, &user.0, &body).await?;
    Ok((StatusCode::CREATED, Json(freeze)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_names_match_across_separators() {
        assert!(agent_matches("Brazen Croissant", "brazen-croissant"));
        assert!(agent_matches(" Zesty Rye ", "zesty rye"));
        assert!(!agent_matches("Zesty Rye", "zesty-rye-2"));
    }

    #[test]
    fn decisions_map_to_statuses_and_events() {
        assert_eq!(
            ApprovalDecision::Approve.status(),
            ApprovalStatus::Approved
        );
        assert_eq!(
            ApprovalDecision::Changes.status(),
            ApprovalStatus::Changes
        );
        assert_eq!(ApprovalDecision::Info.status(), ApprovalStatus::Answered);
        assert_eq!(
            ApprovalDecision::Info.event_kind(),
            ApprovalEventKind::Info
        );
    }

    #[test]
    fn only_pending_needs_the_operator_and_only_open_states_take_agent_edits() {
        assert!(ApprovalStatus::Pending.needs_operator());
        assert!(!ApprovalStatus::Changes.needs_operator());
        assert!(ApprovalStatus::Changes.is_open_for_agent());
        assert!(!ApprovalStatus::Answered.is_open_for_agent());
        assert!(!ApprovalStatus::Expired.is_open_for_agent());
    }

}
