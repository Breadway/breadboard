//! Per-commit attribution: which agent, session, orchestrator and model
//! produced a commit, and how a rewritten commit is matched back to it.
//!
//! This module owns its REST routes so the shared server files only need a
//! small registration block. The rules that matter:
//!
//! - The model is never inferred from whoever was last active. It is either
//!   reported by the agent recording the commit, or resolved from
//!   `usage_sessions` rows for this exact session id when they agree on one
//!   model. Otherwise it is stored as NULL with `model_source = 'unknown'`.
//! - A rebase keeps `patch_id`; a squash of a branch tip produces the same
//!   tree as that tip. Recording either lets a lookup for a new SHA report the
//!   original attribution and how it matched.

use std::collections::BTreeSet;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::{FromRow, SqlitePool};

use breadboard_core::{BoardEvent, CommitAttribution, CommitLookup, CommitMatch, ModelSource};

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::{store, usage, AppState};

const SELECT_COLUMNS: &str = "id, repo, sha, issue_id, issue_key, agent_name, session_id, \
     orchestrator, orchestrator_session_id, model, provider, model_source, patch_id, \
     tree_sha, note, created_at";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/commits", post(record))
        .route("/commits/lookup", get(lookup))
        .route("/issues/{key}/commits", get(list_for_issue))
}

#[derive(Debug, Deserialize)]
pub struct NewCommit {
    /// `owner/name`. Stored as given; lookups are case-insensitive.
    pub repo: String,
    /// Full 40- or 64-character commit SHA.
    pub sha: String,
    /// Issue key (`BB-163`) or numeric issue id.
    pub issue: String,
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    /// Explicit model id. When omitted the server resolves it from usage data
    /// for this exact session, or leaves it unknown.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    /// `git patch-id --stable`, for matching a rebased commit.
    #[serde(default)]
    pub patch_id: Option<String>,
    /// Tree SHA, for matching a squash of this commit's branch tip.
    #[serde(default)]
    pub tree_sha: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LookupQuery {
    pub repo: String,
    pub sha: String,
    /// Patch id of the rewritten SHA, when the caller can compute it.
    #[serde(default)]
    pub patch_id: Option<String>,
    /// Tree SHA of the rewritten SHA, when the caller can compute it.
    #[serde(default)]
    pub tree_sha: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
struct CommitRow {
    id: i64,
    repo: String,
    sha: String,
    issue_id: Option<i64>,
    issue_key: Option<String>,
    agent_name: String,
    session_id: Option<String>,
    orchestrator: Option<String>,
    orchestrator_session_id: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    model_source: String,
    patch_id: Option<String>,
    tree_sha: Option<String>,
    note: Option<String>,
    created_at: String,
}

impl From<CommitRow> for CommitAttribution {
    fn from(row: CommitRow) -> Self {
        CommitAttribution {
            id: row.id,
            repo: row.repo,
            sha: row.sha,
            issue_id: row.issue_id,
            issue_key: row.issue_key,
            agent_name: row.agent_name,
            session_id: row.session_id,
            orchestrator: row.orchestrator,
            orchestrator_session_id: row.orchestrator_session_id,
            model: row.model,
            provider: row.provider,
            model_source: ModelSource::parse(&row.model_source).unwrap_or(ModelSource::Unknown),
            patch_id: row.patch_id,
            tree_sha: row.tree_sha,
            note: row.note,
            created_at: parse_ts(&row.created_at),
        }
    }
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

fn parse_ts(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| DateTime::<Utc>::from_timestamp(0, 0).unwrap_or_default())
}

fn normalize_repo(raw: &str) -> AppResult<String> {
    let raw = raw.trim().trim_end_matches('/');
    let raw = raw.strip_suffix(".git").unwrap_or(raw);
    let (owner, name) = raw
        .split_once('/')
        .ok_or_else(|| AppError::bad_request("repo must be owner/name"))?;
    if owner.is_empty()
        || name.is_empty()
        || name.contains('/')
        || raw.chars().any(char::is_whitespace)
    {
        return Err(AppError::bad_request("repo must be a single owner/name"));
    }
    Ok(format!("{owner}/{name}"))
}

fn is_hex_sha(value: &str) -> bool {
    let len = value.len();
    (len == 40 || len == 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn normalize_sha(raw: &str, field: &str) -> AppResult<String> {
    let value = raw.trim().to_ascii_lowercase();
    if !is_hex_sha(&value) {
        return Err(AppError::bad_request(format!(
            "{field} must be a full 40- or 64-character hex commit SHA"
        )));
    }
    Ok(value)
}

fn normalize_optional_sha(raw: Option<&str>, field: &str) -> AppResult<Option<String>> {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => Ok(Some(normalize_sha(value, field)?)),
        None => Ok(None),
    }
}

async fn resolve_issue(db: &SqlitePool, reference: &str) -> AppResult<breadboard_core::Issue> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Err(AppError::bad_request("issue is required"));
    }
    if let Ok(id) = reference.parse::<i64>() {
        return store::get_issue(db, id).await;
    }
    store::get_issue_by_key(db, &reference.to_ascii_uppercase()).await
}

/// Resolve the agent and session that produced the commit. The caller supplies
/// them when it knows them; otherwise the server uses this issue's claim or the
/// roster row registered for the supplied session id. No global "last active"
/// lookup is used here.
async fn resolve_identity(
    db: &SqlitePool,
    user_id: i64,
    issue_id: i64,
    agent_name: Option<&str>,
    session_id: Option<&str>,
) -> AppResult<(String, Option<String>)> {
    let session = session_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let name = match agent_name.map(str::trim).filter(|value| !value.is_empty()) {
        Some(name) => store::ensure_agent_writable(db, user_id, name, session.as_deref()).await?,
        None => if let Some(session) = &session {
            lookup_agent_by_session(db, session).await?
        } else {
            sqlx::query_scalar(
                "SELECT agent_name FROM agent_claims
                      WHERE issue_id = ?1 AND claimed_by = ?2
                      ORDER BY CASE mode WHEN 'exclusive' THEN 0 ELSE 1 END,
                               last_heartbeat DESC
                      LIMIT 1",
            )
            .bind(issue_id)
            .bind(user_id)
            .fetch_optional(db)
            .await?
        }
        .ok_or_else(|| {
            AppError::bad_request("agent_name or session_id is required, or claim the issue first")
        })?,
    };

    // When the caller named the agent but not the session, use that agent's
    // registered session. The commit's model is then still keyed by the exact
    // session id, never by whichever session happened to be busy.
    let session = match session {
        Some(session) => Some(session),
        None => usage::session_for_agent(db, user_id, &name).await?,
    };
    Ok((name, session))
}

async fn lookup_agent_by_session(db: &SqlitePool, session_id: &str) -> AppResult<Option<String>> {
    if let Some(name) = sqlx::query_scalar(
        "SELECT agent_name FROM agent_sessions
          WHERE session_id = ?1
          ORDER BY last_seen DESC
          LIMIT 1",
    )
    .bind(session_id)
    .fetch_optional(db)
    .await?
    {
        return Ok(Some(name));
    }
    Ok(sqlx::query_scalar(
        "SELECT agent_name FROM agent_claims
          WHERE session_id = ?1
          ORDER BY last_heartbeat DESC
          LIMIT 1",
    )
    .bind(session_id)
    .fetch_optional(db)
    .await?)
}

async fn resolve_orchestrator(
    db: &SqlitePool,
    agent_name: &str,
) -> AppResult<(Option<String>, Option<String>)> {
    let parent: Option<Option<String>> =
        sqlx::query_scalar("SELECT parent_session_id FROM agent_sessions WHERE agent_name = ?1")
            .bind(agent_name)
            .fetch_optional(db)
            .await?;
    let Some(parent) = parent.flatten().filter(|value| !value.trim().is_empty()) else {
        return Ok((None, None));
    };
    let name = sqlx::query_scalar::<_, String>(
        "SELECT agent_name FROM agent_sessions
          WHERE session_id = ?1
          LIMIT 1",
    )
    .bind(&parent)
    .fetch_optional(db)
    .await?
    .or(sqlx::query_scalar(
        "SELECT agent_name FROM agent_claims
          WHERE session_id = ?1
          ORDER BY last_heartbeat DESC
          LIMIT 1",
    )
    .bind(&parent)
    .fetch_optional(db)
    .await?);
    Ok((name, Some(parent)))
}

#[derive(Debug, Clone)]
struct ResolvedModel {
    model: Option<String>,
    provider: Option<String>,
    source: ModelSource,
}

impl ResolvedModel {
    fn unknown() -> Self {
        Self {
            model: None,
            provider: None,
            source: ModelSource::Unknown,
        }
    }

    fn label(&self) -> Option<String> {
        match (&self.provider, &self.model) {
            (Some(provider), Some(model))
                if !model.starts_with(&format!("{provider}/")) && !model.contains('/') =>
            {
                Some(format!("{provider}/{model}"))
            }
            (_, Some(model)) => Some(model.clone()),
            _ => None,
        }
    }
}

/// Resolve the model for a commit. Explicit report wins. Otherwise usage rows
/// for this exact session must agree on a single model; anything else is
/// explicitly unknown.
async fn resolve_model(
    db: &SqlitePool,
    session_id: Option<&str>,
    reported_model: Option<&str>,
    reported_provider: Option<&str>,
) -> AppResult<ResolvedModel> {
    let reported_model = reported_model
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("unknown"));
    if let Some(model) = reported_model {
        let mut provider = reported_provider
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if provider.is_none() {
            provider = provider_for_reported(db, session_id, model).await?;
        }
        return Ok(ResolvedModel {
            model: Some(model.to_string()),
            provider,
            source: ModelSource::Reported,
        });
    }

    let Some(session_id) = session_id else {
        return Ok(ResolvedModel::unknown());
    };
    let rows: Vec<(Option<String>, String)> = sqlx::query_as(
        "SELECT DISTINCT provider, model FROM usage_sessions
          WHERE session_id = ?1
            AND model IS NOT NULL
            AND trim(model) != ''
            AND lower(model) != 'unknown'",
    )
    .bind(session_id)
    .fetch_all(db)
    .await?;

    let models: BTreeSet<String> = rows
        .iter()
        .map(|(_, model)| model.trim().to_string())
        .collect();
    if models.len() != 1 {
        // Zero models: no usage data yet. More than one: this session switched
        // models, so the commit's model cannot be known without a report.
        return Ok(ResolvedModel::unknown());
    }
    let model = models.into_iter().next().expect("one model checked above");
    let providers: BTreeSet<String> = rows
        .iter()
        .filter(|(_, row_model)| row_model.trim() == model)
        .filter_map(|(provider, _)| provider.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();
    Ok(ResolvedModel {
        model: Some(model),
        provider: (providers.len() == 1)
            .then(|| providers.into_iter().next())
            .flatten(),
        source: ModelSource::Session,
    })
}

async fn provider_for_reported(
    db: &SqlitePool,
    session_id: Option<&str>,
    model: &str,
) -> AppResult<Option<String>> {
    let Some(session_id) = session_id else {
        return Ok(None);
    };
    let providers: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT provider FROM usage_sessions
          WHERE session_id = ?1
            AND lower(model) = lower(?2)
            AND provider IS NOT NULL
            AND trim(provider) != ''",
    )
    .bind(session_id)
    .bind(model)
    .fetch_all(db)
    .await?;
    Ok((providers.len() == 1)
        .then(|| providers.into_iter().next())
        .flatten())
}

fn merge_model(existing: Option<&ResolvedModel>, next: &ResolvedModel) -> ResolvedModel {
    let Some(existing) = existing else {
        return next.clone();
    };
    // Only an explicit report may replace an already-known model. A later
    // session resolution or an unknown must not erase the original answer.
    if existing.source != ModelSource::Unknown && next.source != ModelSource::Reported {
        return existing.clone();
    }
    next.clone()
}

async fn record(
    State(app): State<AppState>,
    user: AuthUser,
    Json(body): Json<NewCommit>,
) -> AppResult<(StatusCode, Json<CommitAttribution>)> {
    let repo = normalize_repo(&body.repo)?;
    let sha = normalize_sha(&body.sha, "sha")?;
    let patch_id = normalize_optional_sha(body.patch_id.as_deref(), "patch_id")?;
    let tree_sha = normalize_optional_sha(body.tree_sha.as_deref(), "tree_sha")?;
    let issue = resolve_issue(&app.db, &body.issue).await?;
    let (agent_name, session_id) = resolve_identity(
        &app.db,
        user.0.id,
        issue.id,
        body.agent_name.as_deref(),
        body.session_id.as_deref(),
    )
    .await?;
    let (orchestrator, orchestrator_session_id) =
        resolve_orchestrator(&app.db, &agent_name).await?;
    let resolved = resolve_model(
        &app.db,
        session_id.as_deref(),
        body.model.as_deref(),
        body.provider.as_deref(),
    )
    .await?;

    let ts = now_rfc3339();
    let mut tx = crate::db::begin_write(&app.db).await?;

    let existing: Option<CommitRow> = sqlx::query_as(&format!(
        "SELECT {SELECT_COLUMNS} FROM commit_attribution WHERE repo = ?1 AND sha = ?2"
    ))
    .bind(&repo)
    .bind(&sha)
    .fetch_optional(&mut *tx)
    .await?;
    let existing_model = existing.as_ref().map(|row| ResolvedModel {
        model: row.model.clone(),
        provider: row.provider.clone(),
        source: ModelSource::parse(&row.model_source).unwrap_or(ModelSource::Unknown),
    });
    let model = merge_model(existing_model.as_ref(), &resolved);

    let note = body
        .note
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| existing.as_ref().and_then(|row| row.note.clone()));
    let session_id =
        session_id.or_else(|| existing.as_ref().and_then(|row| row.session_id.clone()));
    let orchestrator =
        orchestrator.or_else(|| existing.as_ref().and_then(|row| row.orchestrator.clone()));
    let orchestrator_session_id = orchestrator_session_id.or_else(|| {
        existing
            .as_ref()
            .and_then(|row| row.orchestrator_session_id.clone())
    });
    let patch_id = patch_id.or_else(|| existing.as_ref().and_then(|row| row.patch_id.clone()));
    let tree_sha = tree_sha.or_else(|| existing.as_ref().and_then(|row| row.tree_sha.clone()));

    sqlx::query(
        "INSERT INTO commit_attribution
            (repo, sha, issue_id, issue_key, agent_name, session_id, orchestrator,
             orchestrator_session_id, model, provider, model_source, patch_id,
             tree_sha, note, recorded_by, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16)
         ON CONFLICT(repo, sha) DO UPDATE SET
            issue_id = excluded.issue_id,
            issue_key = excluded.issue_key,
            agent_name = excluded.agent_name,
            session_id = excluded.session_id,
            orchestrator = excluded.orchestrator,
            orchestrator_session_id = excluded.orchestrator_session_id,
            model = excluded.model,
            provider = excluded.provider,
            model_source = excluded.model_source,
            patch_id = excluded.patch_id,
            tree_sha = excluded.tree_sha,
            note = excluded.note,
            recorded_by = excluded.recorded_by,
            updated_at = excluded.updated_at",
    )
    .bind(&repo)
    .bind(&sha)
    .bind(issue.id)
    .bind(&issue.key)
    .bind(&agent_name)
    .bind(&session_id)
    .bind(&orchestrator)
    .bind(&orchestrator_session_id)
    .bind(&model.model)
    .bind(&model.provider)
    .bind(model.source.as_str())
    .bind(&patch_id)
    .bind(&tree_sha)
    .bind(&note)
    .bind(user.0.id)
    .bind(&ts)
    .execute(&mut *tx)
    .await?;

    // The activity timeline is where a reader sees that a commit was attached
    // to this issue. It is written in the same transaction as the attribution.
    let summary = match model.label() {
        Some(label) => format!("{repo}@{} by {agent_name} ({label})", short_sha(&sha)),
        None => format!("{repo}@{} by {agent_name} (model unknown)", short_sha(&sha)),
    };
    sqlx::query(
        "INSERT INTO sync_log (issue_id, field, old_value, new_value, source, actor_id, at)
         VALUES (?1, 'commit', NULL, ?2, 'breadboard', ?3, ?4)",
    )
    .bind(issue.id)
    .bind(&summary)
    .bind(user.0.id)
    .bind(&ts)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    app.bus.publish(BoardEvent::CardUpdated {
        id: issue.id,
        key: issue.key.clone(),
    });
    let row = get_exact(&app.db, &repo, &sha).await?.ok_or_else(|| {
        AppError::Other(anyhow::anyhow!("commit attribution vanished after write"))
    })?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

async fn lookup(
    State(app): State<AppState>,
    _user: AuthUser,
    Query(query): Query<LookupQuery>,
) -> AppResult<Json<CommitLookup>> {
    let repo = normalize_repo(&query.repo)?;
    let sha = normalize_sha(&query.sha, "sha")?;
    let patch_id = normalize_optional_sha(query.patch_id.as_deref(), "patch_id")?;
    let tree_sha = normalize_optional_sha(query.tree_sha.as_deref(), "tree_sha")?;

    if let Some(row) = get_exact(&app.db, &repo, &sha).await? {
        return Ok(Json(known_lookup(repo, sha, CommitMatch::Exact, row)));
    }
    if let Some(patch_id) = patch_id {
        if let Some(row) = get_by_patch(&app.db, &repo, &patch_id).await? {
            return Ok(Json(known_lookup(repo, sha, CommitMatch::PatchId, row)));
        }
    }
    if let Some(tree_sha) = tree_sha {
        if let Some(row) = get_by_tree(&app.db, &repo, &tree_sha).await? {
            return Ok(Json(known_lookup(repo, sha, CommitMatch::TreeSha, row)));
        }
    }
    Ok(Json(CommitLookup {
        repo,
        sha,
        known: false,
        matched_by: None,
        attribution: None,
        message: Some("no attribution recorded for this commit".into()),
    }))
}

async fn list_for_issue(
    State(app): State<AppState>,
    _user: AuthUser,
    Path(key): Path<String>,
) -> AppResult<Json<Vec<CommitAttribution>>> {
    let issue = store::get_issue_by_key(&app.db, &key.to_ascii_uppercase()).await?;
    let rows: Vec<CommitRow> = sqlx::query_as(&format!(
        "SELECT {SELECT_COLUMNS} FROM commit_attribution
          WHERE issue_id IN (
                SELECT ?1
                UNION
                SELECT CASE WHEN l.issue_id = ?1 THEN l.target_id ELSE l.issue_id END
                  FROM issue_links l
                 WHERE l.kind IN ('closes', 'closed-by')
                   AND (l.issue_id = ?1 OR l.target_id = ?1)
          )
          ORDER BY created_at DESC, id DESC"
    ))
    .bind(issue.id)
    .fetch_all(&app.db)
    .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

fn known_lookup(
    repo: String,
    sha: String,
    matched_by: CommitMatch,
    row: CommitRow,
) -> CommitLookup {
    CommitLookup {
        repo,
        sha,
        known: true,
        matched_by: Some(matched_by),
        attribution: Some(row.into()),
        message: None,
    }
}

async fn get_exact(db: &SqlitePool, repo: &str, sha: &str) -> AppResult<Option<CommitRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SELECT_COLUMNS} FROM commit_attribution WHERE repo = ?1 AND sha = ?2"
    ))
    .bind(repo)
    .bind(sha)
    .fetch_optional(db)
    .await?)
}

async fn get_by_patch(db: &SqlitePool, repo: &str, patch_id: &str) -> AppResult<Option<CommitRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SELECT_COLUMNS} FROM commit_attribution
          WHERE repo = ?1 AND patch_id = ?2
          ORDER BY created_at DESC, id DESC LIMIT 1"
    ))
    .bind(repo)
    .bind(patch_id)
    .fetch_optional(db)
    .await?)
}

async fn get_by_tree(db: &SqlitePool, repo: &str, tree_sha: &str) -> AppResult<Option<CommitRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SELECT_COLUMNS} FROM commit_attribution
          WHERE repo = ?1 AND tree_sha = ?2
          ORDER BY created_at DESC, id DESC LIMIT 1"
    ))
    .bind(repo)
    .bind(tree_sha)
    .fetch_optional(db)
    .await?)
}

fn short_sha(sha: &str) -> &str {
    sha.get(..10).unwrap_or(sha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_and_sha_validation_is_strict() {
        assert_eq!(
            normalize_repo(" Breadway/breadboard.git ").unwrap(),
            "Breadway/breadboard"
        );
        assert!(normalize_repo("breadboard").is_err());
        assert!(normalize_repo("owner/name/extra").is_err());

        let full = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(normalize_sha(full, "sha").unwrap(), full);
        assert!(normalize_sha("0123456", "sha").is_err());
        assert!(normalize_sha(&format!("{full}0"), "sha").is_err());
    }

    #[test]
    fn merge_keeps_reported_model_over_a_session_guess() {
        let existing = ResolvedModel {
            model: Some("reported-model".into()),
            provider: Some("provider".into()),
            source: ModelSource::Reported,
        };
        let next = ResolvedModel {
            model: Some("other-model".into()),
            provider: None,
            source: ModelSource::Session,
        };
        assert_eq!(
            merge_model(Some(&existing), &next).model.as_deref(),
            Some("reported-model")
        );

        let unknown = ResolvedModel::unknown();
        assert_eq!(
            merge_model(Some(&existing), &unknown).source,
            ModelSource::Reported
        );
        assert_eq!(
            merge_model(Some(&unknown), &next).source,
            ModelSource::Session
        );

        let session = ResolvedModel {
            model: Some("session-model".into()),
            provider: Some("provider".into()),
            source: ModelSource::Session,
        };
        let other_session = ResolvedModel {
            model: Some("later-session-model".into()),
            provider: Some("provider".into()),
            source: ModelSource::Session,
        };
        assert_eq!(
            merge_model(Some(&session), &other_session).model.as_deref(),
            Some("session-model")
        );
    }

    #[test]
    fn model_label_does_not_double_prefix_provider() {
        let resolved = ResolvedModel {
            model: Some("opencode-go/deepseek-v4.1-flash".into()),
            provider: Some("opencode-go".into()),
            source: ModelSource::Session,
        };
        assert_eq!(
            resolved.label().as_deref(),
            Some("opencode-go/deepseek-v4.1-flash")
        );
    }
}
