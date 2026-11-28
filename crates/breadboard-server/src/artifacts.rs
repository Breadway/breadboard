//! Typed issue artifacts: benches, screenshots, CI logs, diffs, traces and
//! other evidence. Uploads are authenticated, rows are immutable, and files
//! live under the same upload dir as chat attachments.
//!
//! This module owns its routes so the shared server files only need a small
//! registration block.

use std::path::{Component, Path as FsPath};

use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use axum::Json;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::Deserialize;
use sqlx::SqlitePool;

use breadboard_core::{ArtifactKind, IssueArtifact};

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::{store, AppState};

/// Absolute ceiling for one artifact, matching the largest per-kind limit.
pub const MAX_ARTIFACT_BYTES: usize = 50 * 1024 * 1024;

/// Multipart body allowance: max artifact plus multipart framing.
pub const MAX_BODY_BYTES: usize = MAX_ARTIFACT_BYTES + 2 * 1024 * 1024;

/// Base64 grows by 4/3, so a full 50 MiB CI log needs about 70 MiB of request
/// body. Keep a little room for JSON framing.
pub const MAX_BASE64_BODY_BYTES: usize = 72 * 1024 * 1024;

/// Only these raster types are served inline. SVG is deliberately excluded:
/// inline SVG executes script same-origin and would defeat the session cookie.
pub const RASTER_MIMES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

pub fn is_raster_mime(mime: &str) -> bool {
    let base = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    RASTER_MIMES.contains(&base.as_str())
}

/// Per-kind byte caps. Large CI logs get room; small evidence stays small.
pub fn kind_limit(kind: ArtifactKind) -> usize {
    match kind {
        ArtifactKind::Screenshot | ArtifactKind::Bench => 10 * 1024 * 1024,
        ArtifactKind::Diff => 25 * 1024 * 1024,
        ArtifactKind::CiLog | ArtifactKind::Trace => MAX_ARTIFACT_BYTES,
        ArtifactKind::Other => 10 * 1024 * 1024,
    }
}

/// Retention per kind. CI evidence expires quickly because it is the bulky
/// kind; screenshots and benches stay long enough to remain useful evidence.
pub fn retention(kind: ArtifactKind) -> ChronoDuration {
    match kind {
        ArtifactKind::Screenshot | ArtifactKind::Bench => ChronoDuration::days(180),
        ArtifactKind::Diff | ArtifactKind::Other => ChronoDuration::days(90),
        ArtifactKind::CiLog | ArtifactKind::Trace => ChronoDuration::days(30),
    }
}

/// A stored artifact row plus the filename inside the upload dir.
pub struct StoredArtifact {
    pub artifact: IssueArtifact,
    pub path: String,
}

#[derive(sqlx::FromRow)]
struct ArtifactRow {
    id: i64,
    issue_id: i64,
    kind: String,
    comment_id: Option<i64>,
    review_ref: Option<String>,
    filename: String,
    mime: String,
    size: i64,
    uploader_agent: String,
    session_id: Option<String>,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    path: String,
}

impl From<ArtifactRow> for IssueArtifact {
    fn from(row: ArtifactRow) -> Self {
        IssueArtifact {
            id: row.id,
            issue_id: row.issue_id,
            kind: ArtifactKind::parse(&row.kind).unwrap_or(ArtifactKind::Other),
            comment_id: row.comment_id,
            review_ref: row.review_ref,
            filename: row.filename,
            mime: row.mime,
            size: row.size,
            uploader_agent: row.uploader_agent,
            session_id: row.session_id,
            created_at: row.created_at,
            expires_at: row.expires_at,
        }
    }
}

const SELECT_COLUMNS: &str = "id, issue_id, kind, comment_id, review_ref, filename, mime,
     size, uploader_agent, session_id, created_at, expires_at, path";

// --- storage --------------------------------------------------------------

pub async fn list_artifacts(db: &SqlitePool, issue_id: i64) -> AppResult<Vec<IssueArtifact>> {
    let rows: Vec<ArtifactRow> = sqlx::query_as(&format!(
        "SELECT {SELECT_COLUMNS}
           FROM issue_artifacts
          WHERE issue_id = ?1 AND expires_at > ?2
          ORDER BY created_at DESC, id DESC"
    ))
    .bind(issue_id)
    .bind(Utc::now())
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn get_artifact(db: &SqlitePool, id: i64) -> AppResult<StoredArtifact> {
    let row: Option<ArtifactRow> = sqlx::query_as(&format!(
        "SELECT {SELECT_COLUMNS} FROM issue_artifacts WHERE id = ?1"
    ))
    .bind(id)
    .fetch_optional(db)
    .await?;
    let row = row.ok_or(AppError::NotFound)?;
    Ok(StoredArtifact {
        path: row.path.clone(),
        artifact: row.into(),
    })
}

#[allow(clippy::too_many_arguments)]
async fn insert_artifact(
    db: &SqlitePool,
    issue_id: i64,
    kind: ArtifactKind,
    comment_id: Option<i64>,
    review_ref: Option<&str>,
    filename: &str,
    mime: &str,
    size: i64,
    path: &str,
    uploader_agent: &str,
    session_id: Option<&str>,
) -> AppResult<IssueArtifact> {
    let created_at = Utc::now();
    let expires_at = created_at + retention(kind);
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO issue_artifacts
            (issue_id, kind, comment_id, review_ref, filename, mime, size, path,
             uploader_agent, session_id, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         RETURNING id",
    )
    .bind(issue_id)
    .bind(kind.as_str())
    .bind(comment_id)
    .bind(review_ref)
    .bind(filename)
    .bind(mime)
    .bind(size)
    .bind(path)
    .bind(uploader_agent)
    .bind(session_id)
    .bind(created_at)
    .bind(expires_at)
    .fetch_one(db)
    .await?;
    Ok(IssueArtifact {
        id,
        issue_id,
        kind,
        comment_id,
        review_ref: review_ref.map(str::to_string),
        filename: filename.to_string(),
        mime: mime.to_string(),
        size,
        uploader_agent: uploader_agent.to_string(),
        session_id: session_id.map(str::to_string),
        created_at,
        expires_at,
    })
}

/// Delete expired rows and their files. Called at startup and daily, and also
/// by tests that need to prove expiry actually removes the evidence.
pub async fn purge_expired(state: &AppState) -> AppResult<usize> {
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, path FROM issue_artifacts WHERE expires_at <= ?1")
            .bind(Utc::now())
            .fetch_all(&state.db)
            .await?;
    let base = state.config.uploads_dir();
    for (_, stored) in &rows {
        let path = safe_join(&base, stored)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "artifact expiry: could not remove file");
            }
        }
    }
    for (id, _) in &rows {
        sqlx::query("DELETE FROM issue_artifacts WHERE id = ?1")
            .bind(id)
            .execute(&state.db)
            .await?;
    }
    Ok(rows.len())
}

/// Startup sweep plus a daily re-check. Expired evidence is removed on both
/// the database row and the file, never just hidden.
pub fn spawn_expiry_sweep(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let Err(error) = purge_expired(&state).await {
                tracing::warn!(%error, "artifact expiry sweep failed");
            }
        }
    });
}

fn safe_join(base: &FsPath, stored: &str) -> AppResult<std::path::PathBuf> {
    let relative = FsPath::new(stored);
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(AppError::bad_request("invalid artifact path"));
    }
    Ok(base.join(relative))
}

// --- upload helpers -------------------------------------------------------

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
    if let Some(mime) = provided
        .map(str::trim)
        .filter(|m| !m.is_empty() && *m != "application/octet-stream")
    {
        return mime.to_string();
    }
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "txt" | "log" => "text/plain",
        "md" => "text/markdown",
        "json" => "application/json",
        "diff" | "patch" => "text/x-diff",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn normalize_mime(mime: &str) -> String {
    mime.split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

fn storage_ext(filename: &str, kind: ArtifactKind) -> String {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    if !ext.is_empty() && ext.len() <= 10 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return ext;
    }
    match kind {
        ArtifactKind::Screenshot => "png",
        ArtifactKind::Bench => "json",
        ArtifactKind::Diff => "patch",
        ArtifactKind::CiLog | ArtifactKind::Trace => "log",
        ArtifactKind::Other => "bin",
    }
    .to_string()
}

pub(crate) struct UploadInput {
    // Shared with the approvals module, which stores screenshots through this
    // exact pipeline instead of a parallel one.
    pub filename: String,
    pub mime: Option<String>,
    pub bytes: Vec<u8>,
    pub kind: ArtifactKind,
    pub comment_id: Option<i64>,
    pub review_ref: Option<String>,
    pub agent_name: Option<String>,
    pub session_id: Option<String>,
}

pub(crate) async fn save_artifact(
    app: &AppState,
    user: &AuthUser,
    key: &str,
    input: UploadInput,
) -> AppResult<IssueArtifact> {
    let issue = store::get_issue_by_key(&app.db, key).await?;
    if input.bytes.is_empty() {
        return Err(AppError::bad_request("file is empty"));
    }
    let limit = kind_limit(input.kind);
    if input.bytes.len() > limit {
        return Err(AppError::TooLarge(format!(
            "{} artifacts are limited to {} MB",
            input.kind.as_str(),
            limit / (1024 * 1024)
        )));
    }
    let display = safe_display_name(&input.filename);
    let mime = normalize_mime(&guess_mime(&display, input.mime.as_deref()));
    if input.kind == ArtifactKind::Screenshot && !is_raster_mime(&mime) {
        return Err(AppError::unsupported_media_type(
            "a screenshot artifact must be png, jpeg, gif or webp; svg and other types are refused",
        ));
    }
    if let Some(comment_id) = input.comment_id {
        let belongs: Option<i64> =
            sqlx::query_scalar("SELECT id FROM comments WHERE id = ?1 AND issue_id = ?2")
                .bind(comment_id)
                .bind(issue.id)
                .fetch_optional(&app.db)
                .await?;
        if belongs.is_none() {
            return Err(AppError::bad_request(
                "comment_id does not belong to this issue",
            ));
        }
    }
    let review_ref = input
        .review_ref
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty());
    if review_ref
        .as_ref()
        .map(|r| r.chars().count() > 500)
        .unwrap_or(false)
    {
        return Err(AppError::bad_request(
            "review_ref is longer than 500 characters",
        ));
    }
    let (uploader_agent, session_id) = resolve_uploader(
        app,
        user,
        input.agent_name.as_deref(),
        input.session_id.as_deref(),
    )
    .await?;

    let dir = app.config.uploads_dir().join("artifacts");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("creating artifact dir: {e}")))?;
    let stored = format!(
        "artifacts/{}.{}",
        crate::auth::random_token(),
        storage_ext(&display, input.kind)
    );
    let full_path = safe_join(&app.config.uploads_dir(), &stored)?;
    tokio::fs::write(&full_path, &input.bytes)
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("writing artifact: {e}")))?;

    let inserted = insert_artifact(
        &app.db,
        issue.id,
        input.kind,
        input.comment_id,
        review_ref.as_deref(),
        &display,
        &mime,
        input.bytes.len() as i64,
        &stored,
        &uploader_agent,
        session_id.as_deref(),
    )
    .await;
    if inserted.is_err() {
        let _ = tokio::fs::remove_file(&full_path).await;
    }
    inserted
}

/// The uploader is the verified agent name when supplied, or `human:<login>`.
/// A named agent must belong to the caller, matching chat attribution.
async fn resolve_uploader(
    app: &AppState,
    user: &AuthUser,
    agent_name: Option<&str>,
    session_id: Option<&str>,
) -> AppResult<(String, Option<String>)> {
    let session_id = session_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    match agent_name.map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => {
            let normalized =
                store::ensure_agent_writable(&app.db, user.0.id, name, session_id.as_deref())
                    .await?;
            if !store::user_has_agent(&app.db, user.0.id, &normalized).await? {
                return Err(AppError::forbidden());
            }
            Ok((normalized, session_id))
        }
        None => Ok((format!("human:{}", user.0.login), None)),
    }
}

// --- HTTP handlers --------------------------------------------------------

pub async fn list(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(key): Path<String>,
) -> AppResult<Json<Vec<IssueArtifact>>> {
    let issue = store::get_issue_by_key(&app.db, &key).await?;
    Ok(Json(list_artifacts(&app.db, issue.id).await?))
}

pub async fn upload(
    State(app): State<AppState>,
    user: AuthUser,
    Path(key): Path<String>,
    mut multipart: Multipart,
) -> AppResult<(StatusCode, Json<IssueArtifact>)> {
    let mut filename = "file".to_string();
    let mut mime = None;
    let mut bytes = Vec::new();
    let mut kind = None;
    let mut comment_id = None;
    let mut review_ref = None;
    let mut agent_name = None;
    let mut session_id = None;
    let mut seen_file = false;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::bad_request(format!("bad multipart: {e}")))?
    {
        match field.name() {
            Some("file") => {
                filename = field.file_name().unwrap_or("file").to_string();
                mime = field.content_type().map(str::to_string);
                bytes = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::bad_request(format!("reading file: {e}")))?
                    .to_vec();
                seen_file = true;
            }
            Some("kind") => {
                let raw = field
                    .text()
                    .await
                    .map_err(|e| AppError::bad_request(format!("reading kind: {e}")))?;
                kind = Some(ArtifactKind::parse(raw.trim()).ok_or_else(|| {
                    AppError::bad_request(format!("unknown artifact kind: {raw}"))
                })?);
            }
            Some("comment_id") => {
                let raw = field
                    .text()
                    .await
                    .map_err(|e| AppError::bad_request(format!("reading comment_id: {e}")))?;
                if !raw.trim().is_empty() {
                    comment_id = Some(
                        raw.trim()
                            .parse()
                            .map_err(|_| AppError::bad_request("comment_id must be a number"))?,
                    );
                }
            }
            Some("review_ref") => {
                review_ref = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError::bad_request(format!("reading review_ref: {e}")))?,
                );
            }
            Some("agent_name") => {
                agent_name = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError::bad_request(format!("reading agent_name: {e}")))?,
                );
            }
            Some("session_id") => {
                session_id = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError::bad_request(format!("reading session_id: {e}")))?,
                );
            }
            _ => {}
        }
    }
    if !seen_file {
        return Err(AppError::bad_request("no file field"));
    }
    let kind = kind.ok_or_else(|| AppError::bad_request("kind is required"))?;
    let saved = save_artifact(
        &app,
        &user,
        &key,
        UploadInput {
            filename,
            mime,
            bytes,
            kind,
            comment_id,
            review_ref,
            agent_name,
            session_id,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(saved)))
}

#[derive(Deserialize)]
pub struct ArtifactBase64 {
    pub filename: String,
    #[serde(default)]
    pub mime: Option<String>,
    pub data_base64: String,
    pub kind: String,
    #[serde(default)]
    pub comment_id: Option<i64>,
    #[serde(default)]
    pub review_ref: Option<String>,
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Base64 variant for the MCP server and other JSON-only clients.
pub async fn upload_base64(
    State(app): State<AppState>,
    user: AuthUser,
    Path(key): Path<String>,
    Json(body): Json<ArtifactBase64>,
) -> AppResult<(StatusCode, Json<IssueArtifact>)> {
    use base64::Engine as _;
    let kind = ArtifactKind::parse(body.kind.trim())
        .ok_or_else(|| AppError::bad_request(format!("unknown artifact kind: {}", body.kind)))?;
    let raw = body
        .data_base64
        .split_once("base64,")
        .map(|(_, rest)| rest)
        .unwrap_or(&body.data_base64);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .map_err(|_| AppError::bad_request("data_base64 is not valid base64"))?;
    let saved = save_artifact(
        &app,
        &user,
        &key,
        UploadInput {
            filename: body.filename,
            mime: body.mime,
            bytes,
            kind,
            comment_id: body.comment_id,
            review_ref: body.review_ref,
            agent_name: body.agent_name,
            session_id: body.session_id,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(saved)))
}

pub async fn download(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let stored = get_artifact(&app.db, id).await?;
    if stored.artifact.expires_at <= Utc::now() {
        return Err(AppError::Expired);
    }
    let path = safe_join(&app.config.uploads_dir(), &stored.path)?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let raster =
        stored.artifact.kind == ArtifactKind::Screenshot && is_raster_mime(&stored.artifact.mime);
    let content_type = if raster {
        stored.artifact.mime.as_str()
    } else if stored.artifact.kind.is_text() || stored.artifact.kind == ArtifactKind::Bench {
        "text/plain"
    } else {
        "application/octet-stream"
    };
    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "private, max-age=3600");
    if raster {
        builder = builder.header(header::CONTENT_DISPOSITION, "inline");
    } else {
        let name = safe_display_name(&stored.artifact.filename);
        builder = builder.header(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
                .map_err(|_| AppError::bad_request("invalid filename"))?,
        );
    }
    builder
        .body(Body::from(bytes))
        .map_err(|e| AppError::Other(anyhow::anyhow!("building artifact response: {e}")))
}
