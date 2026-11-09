//! Token and cost statistics imported from OpenCode's local session store.
//!
//! Input is the SQLite database OpenCode writes on the machine running an
//! agent: `session_v2` holds one row per session with aggregate token counts,
//! and `session_message` holds the assistant messages, each with its own model,
//! token counts and cost. The importer reads both, groups per
//! `(session, model, UTC day)`, and resolves the breadboard agent name from
//! `agent_sessions` / `agent_claims` by session id.
//!
//! The importer is deliberately read-only against the OpenCode database and
//! idempotent against breadboard's own `usage_sessions` table: re-importing a
//! session replaces that session's rows, so a growing session never double
//! counts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;

use axum::{
    extract::{Path as AxumPath, Query, State},
    Json,
};
use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::FromRow;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::AppState;

// ---------------------------------------------------------------------------
// API types
// ---------------------------------------------------------------------------

/// Query string shared by the agent and issue usage routes.
#[derive(Debug, Default, Deserialize)]
pub struct UsageQuery {
    /// `24h`, `7d`, `30d`, `90d` or `all` (default `7d`).
    pub range: Option<String>,
    /// Inclusive first day, `YYYY-MM-DD` or RFC3339. Overrides `range`.
    pub from: Option<String>,
    /// Inclusive last day, `YYYY-MM-DD` or RFC3339. Defaults to today (UTC).
    pub to: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageTotals {
    pub cost: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_tokens: i64,
    pub session_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelShare {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub cost: f64,
    pub total_tokens: i64,
    /// Share of the subject's cost, or of its tokens when no cost is recorded.
    pub share: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentShare {
    pub agent_name: String,
    pub cost: f64,
    pub total_tokens: i64,
    pub share: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionUsage {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    pub cost: f64,
    pub total_tokens: i64,
    pub first_day: String,
    pub last_day: String,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageSummary {
    /// Agent name or issue key, depending on `kind`.
    pub subject: String,
    /// `agent`, `issue` or `all`.
    pub kind: String,
    pub range: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub to: String,
    #[serde(flatten)]
    pub totals: UsageTotals,
    pub models: Vec<ModelShare>,
    pub sessions: Vec<SessionUsage>,
    /// Present on an issue summary: which agents spent the tokens.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<AgentShare>,
    pub spend_warn_usd: f64,
    pub over_threshold: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSettings {
    pub spend_warn_usd: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportReport {
    pub path: String,
    pub sessions: i64,
    pub rows: i64,
    pub skipped_sessions: i64,
}

#[derive(Debug, Default, Deserialize)]
pub struct ImportRequest {
    /// Override the configured path for this import.
    #[serde(default)]
    pub path: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub async fn list_agent_usage(
    State(app): State<AppState>,
    _user: AuthUser,
    Query(q): Query<UsageQuery>,
) -> AppResult<Json<Vec<UsageSummary>>> {
    let window = Window::parse(&q)?;
    let threshold = load_threshold(&app.db, app.config.usage.spend_warn_usd).await?;
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT lower(agent_name) FROM usage_sessions
          WHERE agent_name IS NOT NULL ORDER BY 1",
    )
    .fetch_all(&app.db)
    .await?;
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        out.push(agent_summary(&app.db, &name, &window, threshold, false).await?);
    }
    Ok(Json(out))
}

pub async fn get_agent_usage(
    State(app): State<AppState>,
    _user: AuthUser,
    AxumPath(name): AxumPath<String>,
    Query(q): Query<UsageQuery>,
) -> AppResult<Json<UsageSummary>> {
    let window = Window::parse(&q)?;
    let threshold = load_threshold(&app.db, app.config.usage.spend_warn_usd).await?;
    Ok(Json(
        agent_summary(&app.db, &name, &window, threshold, true).await?,
    ))
}

pub async fn get_issue_usage(
    State(app): State<AppState>,
    _user: AuthUser,
    AxumPath(key): AxumPath<String>,
    Query(q): Query<UsageQuery>,
) -> AppResult<Json<UsageSummary>> {
    let window = Window::parse(&q)?;
    let threshold = load_threshold(&app.db, app.config.usage.spend_warn_usd).await?;
    Ok(Json(
        issue_summary(&app.db, &key, &window, threshold).await?,
    ))
}

pub async fn get_settings(
    State(app): State<AppState>,
    _user: AuthUser,
) -> AppResult<Json<UsageSettings>> {
    let value = load_threshold(&app.db, app.config.usage.spend_warn_usd).await?;
    Ok(Json(UsageSettings {
        spend_warn_usd: value,
    }))
}

pub async fn put_settings(
    State(app): State<AppState>,
    _user: AuthUser,
    Json(body): Json<UsageSettings>,
) -> AppResult<Json<UsageSettings>> {
    if !body.spend_warn_usd.is_finite() || body.spend_warn_usd < 0.0 {
        return Err(AppError::bad_request(
            "spend_warn_usd must be a non-negative number",
        ));
    }
    if body.spend_warn_usd > 1_000_000.0 {
        return Err(AppError::bad_request("spend_warn_usd is unreasonably high"));
    }
    ensure_settings(&app.db, app.config.usage.spend_warn_usd).await?;
    sqlx::query("UPDATE usage_settings SET spend_warn_usd = ?1, updated_at = ?2 WHERE id = 1")
        .bind(body.spend_warn_usd)
        .bind(now_rfc3339())
        .execute(&app.db)
        .await?;
    Ok(Json(body))
}

pub async fn import_now(
    State(app): State<AppState>,
    _user: AuthUser,
    body: Option<Json<ImportRequest>>,
) -> AppResult<Json<ImportReport>> {
    let requested = body.and_then(|Json(b)| b.path);
    let path = requested
        .map(PathBuf::from)
        .or_else(|| app.config.usage.opencode_db.clone())
        .ok_or_else(|| {
            AppError::bad_request(
                "no OpenCode database configured; set usage.opencode_db or BREADBOARD_USAGE_OPENCODE_DB",
            )
        })?;
    Ok(Json(import_opencode(&app.db, &path).await?))
}

// ---------------------------------------------------------------------------
// Background importer
// ---------------------------------------------------------------------------

/// Start the periodic importer. Does nothing when no database path is
/// configured. A missing file is not an error: the agent host may simply not
/// have written one yet.
pub fn spawn_importer(state: AppState) {
    let Some(path) = state.config.usage.opencode_db.clone() else {
        return;
    };
    let minutes = state.config.usage.import_interval_mins.max(1);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(StdDuration::from_secs(minutes * 60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if !path.exists() {
                tracing::debug!(path = %path.display(), "usage importer: no OpenCode database yet");
                continue;
            }
            match import_opencode(&state.db, &path).await {
                Ok(report) => tracing::info!(
                    path = %report.path,
                    sessions = report.sessions,
                    rows = report.rows,
                    "usage importer: imported"
                ),
                Err(error) => {
                    tracing::warn!(path = %path.display(), error = %error, "usage importer failed")
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// Read an OpenCode database and upsert this agent's usage into breadboard.
pub async fn import_opencode(db: &sqlx::SqlitePool, path: &Path) -> AppResult<ImportReport> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .busy_timeout(StdDuration::from_secs(2));
    let source = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .map_err(|error| {
            AppError::Other(anyhow::anyhow!(
                "opening OpenCode database {}: {error}",
                path.display()
            ))
        })?;

    let mappings = session_agent_map(db).await?;
    backfill_issue_links(db).await?;
    let rows = read_usage_rows(&source).await?;
    source.close().await;

    let mut by_session: HashMap<String, Vec<UsageRow>> = HashMap::new();
    let mut skipped: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in rows {
        let Some(agent) = mappings.get(&row.session_id).cloned() else {
            skipped.insert(row.session_id.clone());
            continue;
        };
        let row = row.with_agent(agent);
        by_session
            .entry(row.session_id.clone())
            .or_default()
            .push(row);
    }

    let imported_at = now_rfc3339();
    let mut tx = crate::db::begin_write(db).await?;
    let mut inserted = 0_i64;
    for (session_id, rows) in &by_session {
        sqlx::query("DELETE FROM usage_sessions WHERE session_id = ?1 AND source = 'opencode'")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        for row in rows {
            let tokens = row.totals();
            sqlx::query(
                "INSERT INTO usage_sessions
                    (session_id, agent_name, provider, model, day, input_tokens,
                     output_tokens, reasoning_tokens, cache_read_tokens,
                     cache_write_tokens, cost, source, imported_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'opencode', ?12)
                 ON CONFLICT(session_id, model, day, source) DO UPDATE SET
                    agent_name = excluded.agent_name,
                    provider = excluded.provider,
                    input_tokens = excluded.input_tokens,
                    output_tokens = excluded.output_tokens,
                    reasoning_tokens = excluded.reasoning_tokens,
                    cache_read_tokens = excluded.cache_read_tokens,
                    cache_write_tokens = excluded.cache_write_tokens,
                    cost = excluded.cost,
                    imported_at = excluded.imported_at",
            )
            .bind(&row.session_id)
            .bind(&row.agent_name)
            .bind(&row.provider)
            .bind(&row.model)
            .bind(&row.day)
            .bind(tokens.input)
            .bind(tokens.output)
            .bind(tokens.reasoning)
            .bind(tokens.cache_read)
            .bind(tokens.cache_write)
            .bind(row.cost)
            .bind(&imported_at)
            .execute(&mut *tx)
            .await?;
            inserted += 1;
        }
    }
    tx.commit().await?;

    Ok(ImportReport {
        path: path.display().to_string(),
        sessions: by_session.len() as i64,
        rows: inserted,
        skipped_sessions: skipped.len() as i64,
    })
}

#[derive(Debug, Clone, FromRow)]
struct UsageRow {
    session_id: String,
    agent_name: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    day: Option<String>,
    input_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cost: f64,
}

#[derive(Debug, Clone, Copy)]
struct TokenTotals {
    input: i64,
    output: i64,
    reasoning: i64,
    cache_read: i64,
    cache_write: i64,
}

impl UsageRow {
    fn with_agent(mut self, agent: String) -> Self {
        self.agent_name = Some(agent);
        self
    }

    fn totals(&self) -> TokenTotals {
        TokenTotals {
            input: self.input_tokens,
            output: self.output_tokens,
            reasoning: self.reasoning_tokens,
            cache_read: self.cache_read_tokens,
            cache_write: self.cache_write_tokens,
        }
    }

    fn label(&self) -> String {
        model_label(
            self.provider.as_deref(),
            self.model.as_deref().unwrap_or("unknown"),
        )
    }
}

/// Aggregate assistant messages per session, model and day. The fallback covers
/// sessions whose messages have already been pruned but whose aggregate row in
/// `session_v2` still holds the totals.
async fn read_usage_rows(source: &sqlx::SqlitePool) -> AppResult<Vec<UsageRow>> {
    let mut rows: Vec<UsageRow> = sqlx::query_as(
        r#"
        SELECT
            session_id,
            NULL AS agent_name,
            NULLIF(json_extract(data, '$.model.providerID'), '') AS provider,
            json_extract(data, '$.model.id') AS model,
            COALESCE(date(time_created / 1000, 'unixepoch'), '1970-01-01') AS day,
            CAST(COALESCE(SUM(json_extract(data, '$.tokens.input')), 0) AS INTEGER) AS input_tokens,
            CAST(COALESCE(SUM(json_extract(data, '$.tokens.output')), 0) AS INTEGER) AS output_tokens,
            CAST(COALESCE(SUM(json_extract(data, '$.tokens.reasoning')), 0) AS INTEGER) AS reasoning_tokens,
            CAST(COALESCE(SUM(json_extract(data, '$.tokens.cache.read')), 0) AS INTEGER) AS cache_read_tokens,
            CAST(COALESCE(SUM(json_extract(data, '$.tokens.cache.write')), 0) AS INTEGER) AS cache_write_tokens,
            CAST(COALESCE(SUM(json_extract(data, '$.cost')), 0.0) AS REAL) AS cost
        FROM session_message
        WHERE "type" = 'assistant'
          AND json_extract(data, '$.tokens') IS NOT NULL
          AND json_extract(data, '$.tokens') != 'null'
        GROUP BY session_id,
                 json_extract(data, '$.model.providerID'),
                 json_extract(data, '$.model.id'),
                 date(time_created / 1000, 'unixepoch')
        "#,
    )
    .fetch_all(source)
    .await
    .map_err(|error| {
        AppError::Other(anyhow::anyhow!(
            "reading OpenCode session_message: {error}"
        ))
    })?;

    let fallback: Vec<UsageRow> = sqlx::query_as(
        r#"
        SELECT
            id AS session_id,
            NULL AS agent_name,
            NULLIF(json_extract(model, '$.providerID'), '') AS provider,
            COALESCE(json_extract(model, '$.id'), model, 'unknown') AS model,
            COALESCE(date(time_updated / 1000, 'unixepoch'), '1970-01-01') AS day,
            tokens_input AS input_tokens,
            tokens_output AS output_tokens,
            tokens_reasoning AS reasoning_tokens,
            tokens_cache_read AS cache_read_tokens,
            tokens_cache_write AS cache_write_tokens,
            CAST(cost AS REAL) AS cost
        FROM session_v2
        WHERE id NOT IN (
            SELECT DISTINCT session_id FROM session_message
             WHERE "type" = 'assistant'
               AND json_extract(data, '$.tokens') IS NOT NULL
        )
        "#,
    )
    .fetch_all(source)
    .await
    .map_err(|error| AppError::Other(anyhow::anyhow!("reading OpenCode session_v2: {error}")))?;

    rows.extend(fallback);
    for row in &mut rows {
        let label = row.label();
        row.model = Some(label);
    }
    Ok(rows)
}

/// Session id to breadboard agent name. Roster rows win over stale claims.
async fn session_agent_map(db: &sqlx::SqlitePool) -> AppResult<HashMap<String, String>> {
    let mut map = HashMap::new();
    let claims: Vec<(String, String)> = sqlx::query_as(
        "SELECT session_id, lower(agent_name) FROM agent_claims
          WHERE session_id IS NOT NULL AND session_id != ''
          GROUP BY session_id",
    )
    .fetch_all(db)
    .await?;
    for (session, agent) in claims {
        map.insert(session, agent);
    }
    let sessions: Vec<(String, String)> = sqlx::query_as(
        "SELECT session_id, lower(agent_name) FROM agent_sessions
          WHERE session_id IS NOT NULL AND session_id != ''",
    )
    .fetch_all(db)
    .await?;
    for (session, agent) in sessions {
        map.insert(session, agent);
    }
    Ok(map)
}

/// Record that a session touched an issue. Idempotent; keeps the link after a
/// claim is released so the issue total does not shrink.
pub async fn link_session_issue(
    db: &sqlx::SqlitePool,
    session_id: Option<&str>,
    issue_id: i64,
) -> AppResult<()> {
    let Some(session_id) = session_id.map(str::trim).filter(|id| !id.is_empty()) else {
        return Ok(());
    };
    let ts = now_rfc3339();
    sqlx::query(
        "INSERT INTO usage_issue_links (session_id, issue_id, first_seen, last_seen)
         VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(session_id, issue_id) DO UPDATE SET
            first_seen = MIN(usage_issue_links.first_seen, excluded.first_seen),
            last_seen = MAX(usage_issue_links.last_seen, excluded.last_seen)",
    )
    .bind(session_id)
    .bind(issue_id)
    .bind(&ts)
    .execute(db)
    .await?;
    Ok(())
}

/// Refresh links from claims that existed before the usage module (or were
/// created while an import was not running).
async fn backfill_issue_links(db: &sqlx::SqlitePool) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO usage_issue_links (session_id, issue_id, first_seen, last_seen)
         SELECT session_id, issue_id, claimed_at, updated_at
           FROM agent_claims
          WHERE session_id IS NOT NULL AND session_id != ''
         ON CONFLICT(session_id, issue_id) DO UPDATE SET
            first_seen = MIN(usage_issue_links.first_seen, excluded.first_seen),
            last_seen = MAX(usage_issue_links.last_seen, excluded.last_seen)",
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Fill a claim's session id from the roster when the caller omitted it. This
/// is what lets an issue total the sessions that touched it without asking
/// every agent to repeat its session id on each claim.
pub async fn session_for_agent(
    db: &sqlx::SqlitePool,
    user_id: i64,
    agent_name: &str,
) -> AppResult<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT session_id FROM agent_sessions
          WHERE lower(agent_name) = lower(?1)
            AND (user_id = ?2 OR user_id IS NULL)
            AND session_id IS NOT NULL
          LIMIT 1",
    )
    .bind(agent_name.trim())
    .bind(user_id)
    .fetch_optional(db)
    .await?
    .flatten())
}

// ---------------------------------------------------------------------------
// Summaries
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Window {
    label: String,
    from: Option<String>,
    to: String,
}

impl Window {
    fn parse(q: &UsageQuery) -> AppResult<Self> {
        let today = Utc::now().date_naive();
        let explicit_from = q.from.as_deref().map(parse_day).transpose()?;
        let to = q.to.as_deref().map(parse_day).transpose()?.unwrap_or(today);
        let requested = q.range.as_deref().unwrap_or("7d");
        let (label, default_from) = match requested {
            "24h" => ("24h", Some(today - chrono::Duration::days(1))),
            "7d" => ("7d", Some(today - chrono::Duration::days(6))),
            "30d" => ("30d", Some(today - chrono::Duration::days(29))),
            "90d" => ("90d", Some(today - chrono::Duration::days(89))),
            "all" => ("all", None),
            other => {
                return Err(AppError::bad_request(format!(
                    "unknown range {other:?}; use 24h, 7d, 30d, 90d or all"
                )))
            }
        };
        let from = explicit_from.or(default_from);
        if matches!(from, Some(f) if f > to) {
            return Err(AppError::bad_request("from must not be after to"));
        }
        Ok(Self {
            label: label.to_string(),
            from: from.map(|d| d.format("%Y-%m-%d").to_string()),
            to: to.format("%Y-%m-%d").to_string(),
        })
    }
}

fn parse_day(value: &str) -> AppResult<NaiveDate> {
    let value = value.trim();
    if let Ok(day) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Ok(day);
    }
    if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(value) {
        return Ok(ts.date_naive());
    }
    Err(AppError::bad_request(format!(
        "invalid date {value:?}; use YYYY-MM-DD or RFC3339"
    )))
}

#[derive(Debug, Clone, FromRow)]
struct TotalsRow {
    input_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cost: f64,
    session_count: i64,
}

impl TotalsRow {
    fn into_totals(self) -> UsageTotals {
        UsageTotals {
            cost: round_cost(self.cost),
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            reasoning_tokens: self.reasoning_tokens,
            cache_read_tokens: self.cache_read_tokens,
            cache_write_tokens: self.cache_write_tokens,
            total_tokens: self.input_tokens
                + self.output_tokens
                + self.reasoning_tokens
                + self.cache_read_tokens
                + self.cache_write_tokens,
            session_count: self.session_count,
        }
    }
}

#[derive(Debug, Clone, FromRow)]
struct ModelRow {
    model: String,
    provider: Option<String>,
    cost: f64,
    total_tokens: i64,
}

#[derive(Debug, Clone, FromRow)]
struct AgentRow {
    agent_name: String,
    cost: f64,
    total_tokens: i64,
}

#[derive(Debug, Clone, FromRow)]
struct SessionRow {
    session_id: String,
    agent_name: Option<String>,
    cost: f64,
    total_tokens: i64,
    first_day: String,
    last_day: String,
    models: Option<String>,
}

async fn agent_summary(
    db: &sqlx::SqlitePool,
    name: &str,
    window: &Window,
    threshold: f64,
    detailed: bool,
) -> AppResult<UsageSummary> {
    let name = name.trim().to_lowercase();
    let totals: TotalsRow = sqlx::query_as(
        "SELECT
            CAST(COALESCE(SUM(input_tokens), 0) AS INTEGER) AS input_tokens,
            CAST(COALESCE(SUM(output_tokens), 0) AS INTEGER) AS output_tokens,
            CAST(COALESCE(SUM(reasoning_tokens), 0) AS INTEGER) AS reasoning_tokens,
            CAST(COALESCE(SUM(cache_read_tokens), 0) AS INTEGER) AS cache_read_tokens,
            CAST(COALESCE(SUM(cache_write_tokens), 0) AS INTEGER) AS cache_write_tokens,
            CAST(COALESCE(SUM(cost), 0.0) AS REAL) AS cost,
            COUNT(DISTINCT session_id) AS session_count
         FROM usage_sessions
         WHERE lower(agent_name) = lower(?1)
           AND (?2 IS NULL OR day >= ?2)
           AND day <= ?3",
    )
    .bind(&name)
    .bind(&window.from)
    .bind(&window.to)
    .fetch_one(db)
    .await?;

    let models: Vec<ModelRow> = sqlx::query_as(
        "SELECT model, provider,
                CAST(COALESCE(SUM(cost), 0.0) AS REAL) AS cost,
                CAST(COALESCE(SUM(input_tokens + output_tokens + reasoning_tokens
                     + cache_read_tokens + cache_write_tokens), 0) AS INTEGER) AS total_tokens
         FROM usage_sessions
         WHERE lower(agent_name) = lower(?1)
           AND (?2 IS NULL OR day >= ?2)
           AND day <= ?3
         GROUP BY model, provider
         ORDER BY cost DESC, total_tokens DESC",
    )
    .bind(&name)
    .bind(&window.from)
    .bind(&window.to)
    .fetch_all(db)
    .await?;

    let totals = totals.into_totals();
    let models = model_shares(models, totals.cost, totals.total_tokens);

    let sessions = if detailed {
        session_rows(db, Some(&name), None, window).await?
    } else {
        Vec::new()
    };

    Ok(UsageSummary {
        subject: name,
        kind: "agent".into(),
        range: window.label.clone(),
        from: window.from.clone(),
        to: window.to.clone(),
        over_threshold: threshold > 0.0 && totals.cost > threshold,
        spend_warn_usd: threshold,
        totals,
        models,
        sessions,
        agents: Vec::new(),
    })
}

async fn issue_summary(
    db: &sqlx::SqlitePool,
    key: &str,
    window: &Window,
    threshold: f64,
) -> AppResult<UsageSummary> {
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM issues WHERE key = ?1")
        .bind(key)
        .fetch_optional(db)
        .await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }

    let totals: TotalsRow = sqlx::query_as(
        "SELECT
            CAST(COALESCE(SUM(u.input_tokens), 0) AS INTEGER) AS input_tokens,
            CAST(COALESCE(SUM(u.output_tokens), 0) AS INTEGER) AS output_tokens,
            CAST(COALESCE(SUM(u.reasoning_tokens), 0) AS INTEGER) AS reasoning_tokens,
            CAST(COALESCE(SUM(u.cache_read_tokens), 0) AS INTEGER) AS cache_read_tokens,
            CAST(COALESCE(SUM(u.cache_write_tokens), 0) AS INTEGER) AS cache_write_tokens,
            CAST(COALESCE(SUM(u.cost), 0.0) AS REAL) AS cost,
            COUNT(DISTINCT u.session_id) AS session_count
         FROM usage_sessions u
         JOIN usage_issue_links c ON c.session_id = u.session_id
         JOIN issues i ON i.id = c.issue_id
         WHERE i.key = ?1
           AND (?2 IS NULL OR u.day >= ?2)
           AND u.day <= ?3",
    )
    .bind(key)
    .bind(&window.from)
    .bind(&window.to)
    .fetch_one(db)
    .await?;

    let models: Vec<ModelRow> = sqlx::query_as(
        "SELECT u.model, u.provider,
                CAST(COALESCE(SUM(u.cost), 0.0) AS REAL) AS cost,
                CAST(COALESCE(SUM(u.input_tokens + u.output_tokens + u.reasoning_tokens
                     + u.cache_read_tokens + u.cache_write_tokens), 0) AS INTEGER) AS total_tokens
         FROM usage_sessions u
         JOIN usage_issue_links c ON c.session_id = u.session_id
         JOIN issues i ON i.id = c.issue_id
         WHERE i.key = ?1
           AND (?2 IS NULL OR u.day >= ?2)
           AND u.day <= ?3
         GROUP BY u.model, u.provider
         ORDER BY cost DESC, total_tokens DESC",
    )
    .bind(key)
    .bind(&window.from)
    .bind(&window.to)
    .fetch_all(db)
    .await?;

    let agents: Vec<AgentRow> = sqlx::query_as(
        "SELECT COALESCE(lower(u.agent_name), 'unattributed') AS agent_name,
                CAST(COALESCE(SUM(u.cost), 0.0) AS REAL) AS cost,
                CAST(COALESCE(SUM(u.input_tokens + u.output_tokens + u.reasoning_tokens
                     + u.cache_read_tokens + u.cache_write_tokens), 0) AS INTEGER) AS total_tokens
         FROM usage_sessions u
         JOIN usage_issue_links c ON c.session_id = u.session_id
         JOIN issues i ON i.id = c.issue_id
         WHERE i.key = ?1
           AND (?2 IS NULL OR u.day >= ?2)
           AND u.day <= ?3
         GROUP BY COALESCE(lower(u.agent_name), 'unattributed')
         ORDER BY cost DESC, total_tokens DESC",
    )
    .bind(key)
    .bind(&window.from)
    .bind(&window.to)
    .fetch_all(db)
    .await?;

    let totals = totals.into_totals();
    let models = model_shares(models, totals.cost, totals.total_tokens);
    let agents = agents
        .into_iter()
        .map(|row| {
            let share = share(row.cost, row.total_tokens, totals.cost, totals.total_tokens);
            AgentShare {
                agent_name: row.agent_name,
                cost: round_cost(row.cost),
                total_tokens: row.total_tokens,
                share,
            }
        })
        .collect();
    let sessions = session_rows(db, None, Some(key), window).await?;

    Ok(UsageSummary {
        subject: key.to_string(),
        kind: "issue".into(),
        range: window.label.clone(),
        from: window.from.clone(),
        to: window.to.clone(),
        over_threshold: threshold > 0.0 && totals.cost > threshold,
        spend_warn_usd: threshold,
        totals,
        models,
        sessions,
        agents,
    })
}

async fn session_rows(
    db: &sqlx::SqlitePool,
    agent: Option<&str>,
    issue_key: Option<&str>,
    window: &Window,
) -> AppResult<Vec<SessionUsage>> {
    let rows: Vec<SessionRow> = sqlx::query_as(
        "SELECT u.session_id,
                MIN(COALESCE(lower(u.agent_name), 'unattributed')) AS agent_name,
                CAST(COALESCE(SUM(u.cost), 0.0) AS REAL) AS cost,
                CAST(COALESCE(SUM(u.input_tokens + u.output_tokens + u.reasoning_tokens
                     + u.cache_read_tokens + u.cache_write_tokens), 0) AS INTEGER) AS total_tokens,
                MIN(u.day) AS first_day,
                MAX(u.day) AS last_day,
                GROUP_CONCAT(DISTINCT u.model) AS models
         FROM usage_sessions u
         WHERE (?1 IS NULL OR lower(u.agent_name) = lower(?1))
           AND (?2 IS NULL OR u.session_id IN (
                SELECT DISTINCT l.session_id FROM usage_issue_links l
                JOIN issues i ON i.id = l.issue_id
                WHERE i.key = ?2))
           AND (?3 IS NULL OR u.day >= ?3)
           AND u.day <= ?4
         GROUP BY u.session_id
         ORDER BY MAX(u.day) DESC, total_tokens DESC
         LIMIT 50",
    )
    .bind(agent)
    .bind(issue_key)
    .bind(&window.from)
    .bind(&window.to)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| SessionUsage {
            session_id: row.session_id,
            agent_name: row.agent_name,
            cost: round_cost(row.cost),
            total_tokens: row.total_tokens,
            first_day: row.first_day,
            last_day: row.last_day,
            models: row
                .models
                .unwrap_or_default()
                .split(',')
                .filter(|m| !m.is_empty())
                .map(str::to_string)
                .collect(),
        })
        .collect())
}

fn model_shares(rows: Vec<ModelRow>, total_cost: f64, total_tokens: i64) -> Vec<ModelShare> {
    rows.into_iter()
        .map(|row| {
            let share = share(row.cost, row.total_tokens, total_cost, total_tokens);
            ModelShare {
                model: row.model,
                provider: row.provider,
                cost: round_cost(row.cost),
                total_tokens: row.total_tokens,
                share,
            }
        })
        .collect()
}

fn share(cost: f64, tokens: i64, total_cost: f64, total_tokens: i64) -> f64 {
    let value = if total_cost > 0.0 {
        cost / total_cost
    } else if total_tokens > 0 {
        tokens as f64 / total_tokens as f64
    } else {
        0.0
    };
    (value * 10_000.0).round() / 10_000.0
}

fn round_cost(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn model_label(provider: Option<&str>, model: &str) -> String {
    let model = model.trim();
    if model.is_empty() || model == "unknown" {
        return "unknown".to_string();
    }
    match provider.map(str::trim).filter(|p| !p.is_empty()) {
        Some(provider) if !model.starts_with(provider) && !model.contains('/') => {
            format!("{provider}/{model}")
        }
        _ => model.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

const DEFAULT_SPEND_WARN_USD: f64 = 5.0;

pub async fn ensure_settings(db: &sqlx::SqlitePool, default_threshold: f64) -> AppResult<()> {
    let value = if default_threshold.is_finite() && default_threshold >= 0.0 {
        default_threshold
    } else {
        DEFAULT_SPEND_WARN_USD
    };
    sqlx::query(
        "INSERT INTO usage_settings (id, spend_warn_usd, updated_at)
         VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(value)
    .bind(now_rfc3339())
    .execute(db)
    .await?;
    Ok(())
}

async fn load_threshold(db: &sqlx::SqlitePool, default_threshold: f64) -> AppResult<f64> {
    ensure_settings(db, default_threshold).await?;
    Ok(
        sqlx::query_scalar("SELECT spend_warn_usd FROM usage_settings WHERE id = 1")
            .fetch_one(db)
            .await?,
    )
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_labels_are_unambiguous() {
        assert_eq!(
            model_label(Some("opencode-go"), "deepseek-v4.1-flash"),
            "opencode-go/deepseek-v4.1-flash"
        );
        assert_eq!(
            model_label(Some("openrouter"), "deepseek/deepseek-v4.1-flash"),
            "deepseek/deepseek-v4.1-flash"
        );
        assert_eq!(model_label(None, ""), "unknown");
    }

    #[test]
    fn range_parsing_accepts_defaults_and_explicit_days() {
        let window = Window::parse(&UsageQuery::default()).unwrap();
        assert_eq!(window.label, "7d");
        assert!(window.from.is_some());

        let window = Window::parse(&UsageQuery {
            range: Some("all".into()),
            from: Some("2026-01-02".into()),
            to: Some("2026-01-03".into()),
        })
        .unwrap();
        assert_eq!(window.from.as_deref(), Some("2026-01-02"));
        assert_eq!(window.to, "2026-01-03");

        assert!(Window::parse(&UsageQuery {
            range: Some("nonsense".into()),
            ..Default::default()
        })
        .is_err());
    }
}
