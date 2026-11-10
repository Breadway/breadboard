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
//!
//! Billing mode is resolved from config at read time, never stored with the
//! rows: a provider is either metered (its reported cost is billed spend) or an
//! allowance plan (prepaid, so consumption is computed from tokens and
//! configured rates and OpenCode's reported cost is ignored). Changing the
//! config re-interprets history without a re-import.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;

use axum::{
    extract::{Path as AxumPath, Query, State},
    Json,
};
use chrono::{NaiveDate, Timelike, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::FromRow;

use crate::auth::AuthUser;
use crate::config::{BillingMode, ProviderBillingConfig, TokenRates, UsageConfig};
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
    /// Billed spend in USD: reported cost from metered providers only.
    /// Allowance providers contribute nothing here because their tokens draw
    /// down a prepaid allowance instead of a per-token invoice.
    pub cost: f64,
    /// Computed allowance consumption in USD for the selected range. This is
    /// the number that counts against a model's monthly allowance. It is never
    /// OpenCode's reported cost for that provider, which is inconsistent.
    pub allowance_usd: f64,
    /// Metered tokens whose source rows reported zero cost. These are never
    /// shown as $0 spend: no price source exists for them, so clients label
    /// them unpriced (BB-220).
    pub unpriced_tokens: i64,
    /// Allowance tokens with no rate configured for that model. They cannot be
    /// priced, so clients label them unrated instead of guessing a number.
    pub unrated_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_tokens: i64,
    pub session_count: i64,
}

/// Which quantity the `share` fields in one summary are a fraction of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ShareBasis {
    /// Metered reported cost.
    Cost,
    /// Computed allowance consumption.
    Allowance,
    /// Both in dollars, with different provenance (metered plus allowance).
    Dollars,
    /// No dollars exist; shares are token fractions.
    Tokens,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelShare {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Billing mode resolved from config at read time.
    pub billing: BillingMode,
    /// Billed cost for a metered row; always 0 for an allowance row.
    pub cost: f64,
    /// Computed allowance consumption for an allowance row; 0 for metered.
    pub allowance_usd: f64,
    pub unpriced_tokens: i64,
    pub unrated_tokens: i64,
    pub total_tokens: i64,
    /// Fraction of the subject's dollars, or of its tokens when no dollars
    /// exist. See `UsageSummary::share_basis`.
    pub share: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentShare {
    pub agent_name: String,
    /// Billed cost only.
    pub cost: f64,
    /// Computed allowance consumption only.
    pub allowance_usd: f64,
    pub unpriced_tokens: i64,
    pub unrated_tokens: i64,
    pub total_tokens: i64,
    pub share: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionUsage {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Billed cost only.
    pub cost: f64,
    /// Computed allowance consumption only.
    pub allowance_usd: f64,
    pub unpriced_tokens: i64,
    pub unrated_tokens: i64,
    pub total_tokens: i64,
    pub first_day: String,
    pub last_day: String,
    pub models: Vec<String>,
}

/// One rolling rate-limit window over a model's monthly allowance.
#[derive(Debug, Clone, Serialize)]
pub struct AllowanceWindow {
    /// `5h`, `weekly` or `monthly`.
    pub label: String,
    /// Computed consumption inside the window.
    pub used_usd: f64,
    /// The window's share of the monthly allowance (20%, 50%, 100%).
    pub limit_usd: f64,
    pub over_limit: bool,
    /// Inclusive UTC bucket the window starts at.
    pub from: String,
    /// UTC bucket the window ends at (the request time, truncated).
    pub to: String,
}

/// Allowance state for one model on one allowance provider.
#[derive(Debug, Clone, Serialize)]
pub struct ModelAllowance {
    pub provider: String,
    pub model: String,
    /// Monthly allowance for this model, from config.
    pub allowance_usd: f64,
    /// Plan price, from config. Used only for the leverage multiplier.
    pub plan_price_usd: f64,
    /// Monthly allowance divided by plan price: the plan's leverage on this
    /// model, not a price multiplier.
    pub multiplier: f64,
    /// Computed consumption in the selected range.
    pub used_usd: f64,
    pub unrated_tokens: i64,
    pub windows: Vec<AllowanceWindow>,
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
    /// What every `share` field below is a fraction of.
    pub share_basis: ShareBasis,
    pub models: Vec<ModelShare>,
    pub sessions: Vec<SessionUsage>,
    /// Allowance providers touched by the subject, one entry per model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowances: Vec<ModelAllowance>,
    /// True when any allowance window is over its share of the allowance.
    #[serde(default)]
    pub over_allowance: bool,
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
    // Resolve names at read time, not import time: a rename must move the
    // spend with the agent, and a removed session must not resurrect a dead
    // name. Sessions with no current owner fall back to their stored name only
    // while that name is still on the roster; otherwise their history shows
    // under the `unattributed` bucket rather than disappearing.
    let names: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT DISTINCT {RESOLVED_AGENT_SQL} AS resolved
           FROM usage_sessions u
          ORDER BY 1"
    ))
    .fetch_all(&app.db)
    .await?;
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        out.push(
            agent_summary(&app.db, &name, &window, threshold, &app.config.usage, false).await?,
        );
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
        agent_summary(&app.db, &name, &window, threshold, &app.config.usage, true).await?,
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
        issue_summary(&app.db, &key, &window, threshold, &app.config.usage).await?,
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
        // Skip the interval's immediate first tick: importing 100k+ OpenCode
        // rows during server startup competes with migrations and health
        // probes. The on-demand route still imports immediately.
        tick.tick().await;
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

/// Aggregate assistant messages per session, model and 30-minute UTC bucket.
/// The fallback covers sessions whose messages have already been pruned but
/// whose aggregate row in `session_v2` still holds the totals.
///
/// Bucket granularity is deliberate: rolling allowance windows (5h) and the
/// provider's peak/off-peak window cannot be recovered from a whole-day row.
async fn read_usage_rows(source: &sqlx::SqlitePool) -> AppResult<Vec<UsageRow>> {
    let created = bucket_sql("time_created");
    let mut rows: Vec<UsageRow> = sqlx::query_as(&format!(
        r#"
        SELECT
            session_id,
            NULL AS agent_name,
            NULLIF(json_extract(data, '$.model.providerID'), '') AS provider,
            json_extract(data, '$.model.id') AS model,
            {created} AS day,
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
                 {created}
        "#
    ))
    .fetch_all(source)
    .await
    .map_err(|error| {
        AppError::Other(anyhow::anyhow!(
            "reading OpenCode session_message: {error}"
        ))
    })?;

    let updated = bucket_sql("time_updated");
    let fallback: Vec<UsageRow> = sqlx::query_as(&format!(
        r#"
        SELECT
            id AS session_id,
            NULL AS agent_name,
            NULLIF(json_extract(model, '$.providerID'), '') AS provider,
            COALESCE(json_extract(model, '$.id'), model, 'unknown') AS model,
            {updated} AS day,
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
        "#
    ))
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

/// `YYYY-MM-DDTHH:MM` for the start of the 30-minute bucket containing the
/// given epoch-millisecond column. Kept in SQL so a single scan does the
/// grouping.
fn bucket_sql(column: &str) -> String {
    format!(
        "COALESCE(strftime('%Y-%m-%dT%H:', {column} / 1000, 'unixepoch') || \
         CASE WHEN CAST(strftime('%M', {column} / 1000, 'unixepoch') AS INTEGER) < 30 \
              THEN '00' ELSE '30' END, '1970-01-01T00:00')"
    )
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

/// The agent a usage row belongs to, resolved from the session id against the
/// current roster rather than the name captured at import time.
///
/// `agent_sessions` is authoritative because rename updates it in place. A
/// claim can still name a session whose session row was cleaned up, so it is
/// the second choice. If the session has no current owner, the stored name is
/// used only when it is still on the roster; a name that was removed entirely
/// becomes `unattributed`, which keeps its history without putting a dead name
/// back in the agent list.
///
/// Every query using this fragment must alias `usage_sessions` as `u`.
const RESOLVED_AGENT_SQL: &str = r#"
    COALESCE(
        (SELECT s.agent_name
           FROM agent_sessions s
          WHERE s.session_id = u.session_id
          ORDER BY s.last_seen DESC, s.agent_name ASC
          LIMIT 1),
        (SELECT lower(c.agent_name)
           FROM agent_claims c
          WHERE c.session_id = u.session_id
          ORDER BY c.last_heartbeat DESC, c.agent_name ASC
          LIMIT 1),
        CASE
            WHEN u.agent_name IS NOT NULL
             AND EXISTS (
                    SELECT 1
                      FROM agent_sessions r
                     WHERE lower(r.agent_name) = lower(u.agent_name)
                 )
            THEN lower(u.agent_name)
            ELSE 'unattributed'
        END
    )
"#;

/// Tokens in rows that reported no cost. `cost = 0` with tokens is "unpriced"
/// for a metered provider; an allowance provider ignores reported cost anyway,
/// so this only measures the raw condition and Rust decides which applies.
const ZERO_COST_TOKENS_SQL: &str = "(CASE WHEN u.cost <= 0 THEN \
    u.input_tokens + u.output_tokens + u.reasoning_tokens \
    + u.cache_read_tokens + u.cache_write_tokens ELSE 0 END)";

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

/// Tokens and cost for one provider in one scope. Billing mode is resolved in
/// Rust (it is config, not data), so every query groups by provider and the
/// split into billed, unpriced and allowance consumption happens once here.
#[derive(Debug, Clone, FromRow)]
struct ProviderTotalsRow {
    provider: Option<String>,
    input_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cost: f64,
    zero_cost_tokens: i64,
}

impl ProviderTotalsRow {
    fn split(&self) -> TokenSplit {
        TokenSplit {
            input: self.input_tokens,
            output: self.output_tokens,
            reasoning: self.reasoning_tokens,
            cache_read: self.cache_read_tokens,
            cache_write: self.cache_write_tokens,
        }
    }
}

#[derive(Debug, Clone, FromRow)]
struct ModelRow {
    model: String,
    provider: Option<String>,
    input_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cost: f64,
    zero_cost_tokens: i64,
}

impl ModelRow {
    fn split(&self) -> TokenSplit {
        TokenSplit {
            input: self.input_tokens,
            output: self.output_tokens,
            reasoning: self.reasoning_tokens,
            cache_read: self.cache_read_tokens,
            cache_write: self.cache_write_tokens,
        }
    }
}

#[derive(Debug, Clone, FromRow)]
struct AgentRow {
    agent_name: String,
    provider: Option<String>,
    input_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cost: f64,
    zero_cost_tokens: i64,
}

impl AgentRow {
    fn split(&self) -> TokenSplit {
        TokenSplit {
            input: self.input_tokens,
            output: self.output_tokens,
            reasoning: self.reasoning_tokens,
            cache_read: self.cache_read_tokens,
            cache_write: self.cache_write_tokens,
        }
    }
}

/// One (session, provider, model, 30-minute bucket) row. Bucket granularity is
/// what lets summaries compute rolling allowance windows and decide whether a
/// request was billed at peak or off-peak rates.
#[derive(Debug, Clone, FromRow)]
struct SessionBucketRow {
    session_id: String,
    agent_name: Option<String>,
    provider: Option<String>,
    model: String,
    day: String,
    input_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cost: f64,
    zero_cost_tokens: i64,
}

impl SessionBucketRow {
    fn split(&self) -> TokenSplit {
        TokenSplit {
            input: self.input_tokens,
            output: self.output_tokens,
            reasoning: self.reasoning_tokens,
            cache_read: self.cache_read_tokens,
            cache_write: self.cache_write_tokens,
        }
    }
}

/// Allowance-provider rows for one scope, one bucket per (provider, model, day).
#[derive(Debug, Clone, FromRow)]
struct AllowanceBucketRow {
    agent_name: String,
    provider: Option<String>,
    model: String,
    day: String,
    input_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
}

impl AllowanceBucketRow {
    fn split(&self) -> TokenSplit {
        TokenSplit {
            input: self.input_tokens,
            output: self.output_tokens,
            reasoning: self.reasoning_tokens,
            cache_read: self.cache_read_tokens,
            cache_write: self.cache_write_tokens,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct TokenSplit {
    input: i64,
    output: i64,
    reasoning: i64,
    cache_read: i64,
    cache_write: i64,
}

impl TokenSplit {
    fn total(&self) -> i64 {
        self.input + self.output + self.reasoning + self.cache_read + self.cache_write
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct AllowanceCharge {
    usd: f64,
    unrated_tokens: i64,
}

/// Billed/unpriced split for metered providers. Allowance providers never
/// contribute here: their reported catalog cost is not billed.
#[derive(Debug, Clone, Copy, Default)]
struct BillingSplit {
    cost: f64,
    unpriced_tokens: i64,
}

impl BillingSplit {
    fn add(&mut self, billing: BillingMode, cost: f64, zero_cost_tokens: i64) {
        if billing == BillingMode::Metered {
            self.cost += cost;
            self.unpriced_tokens += zero_cost_tokens;
        }
    }
}

fn totals_from_rows(
    rows: &[ProviderTotalsRow],
    billing: &UsageConfig,
    session_count: i64,
    allowance_usd: f64,
    unrated_tokens: i64,
) -> UsageTotals {
    let mut split = BillingSplit::default();
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    let mut reasoning_tokens = 0;
    let mut cache_read_tokens = 0;
    let mut cache_write_tokens = 0;
    for row in rows {
        let tokens = row.split();
        split.add(
            billing.billing_for(row.provider.as_deref()),
            row.cost,
            row.zero_cost_tokens,
        );
        input_tokens += tokens.input;
        output_tokens += tokens.output;
        reasoning_tokens += tokens.reasoning;
        cache_read_tokens += tokens.cache_read;
        cache_write_tokens += tokens.cache_write;
    }
    UsageTotals {
        cost: round_cost(split.cost),
        allowance_usd: round_cost(allowance_usd),
        unpriced_tokens: split.unpriced_tokens,
        unrated_tokens,
        input_tokens,
        output_tokens,
        reasoning_tokens,
        cache_read_tokens,
        cache_write_tokens,
        total_tokens: input_tokens
            + output_tokens
            + reasoning_tokens
            + cache_read_tokens
            + cache_write_tokens,
        session_count,
    }
}

async fn agent_summary(
    db: &sqlx::SqlitePool,
    name: &str,
    window: &Window,
    threshold: f64,
    billing: &UsageConfig,
    detailed: bool,
) -> AppResult<UsageSummary> {
    let name = name.trim().to_lowercase();
    let totals_rows: Vec<ProviderTotalsRow> = sqlx::query_as(&format!(
        "SELECT u.provider,
            CAST(COALESCE(SUM(u.input_tokens), 0) AS INTEGER) AS input_tokens,
            CAST(COALESCE(SUM(u.output_tokens), 0) AS INTEGER) AS output_tokens,
            CAST(COALESCE(SUM(u.reasoning_tokens), 0) AS INTEGER) AS reasoning_tokens,
            CAST(COALESCE(SUM(u.cache_read_tokens), 0) AS INTEGER) AS cache_read_tokens,
            CAST(COALESCE(SUM(u.cache_write_tokens), 0) AS INTEGER) AS cache_write_tokens,
            CAST(COALESCE(SUM(u.cost), 0.0) AS REAL) AS cost,
            CAST(COALESCE(SUM({ZERO_COST_TOKENS_SQL}), 0) AS INTEGER) AS zero_cost_tokens
         FROM usage_sessions u
         WHERE lower({RESOLVED_AGENT_SQL}) = lower(?1)
           AND (?2 IS NULL OR u.day >= ?2)
           AND u.day < date(?3, '+1 day')
         GROUP BY u.provider"
    ))
    .bind(&name)
    .bind(&window.from)
    .bind(&window.to)
    .fetch_all(db)
    .await?;

    let session_count: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(DISTINCT u.session_id)
           FROM usage_sessions u
          WHERE lower({RESOLVED_AGENT_SQL}) = lower(?1)
            AND (?2 IS NULL OR u.day >= ?2)
            AND u.day < date(?3, '+1 day')"
    ))
    .bind(&name)
    .bind(&window.from)
    .bind(&window.to)
    .fetch_one(db)
    .await?;

    let model_rows: Vec<ModelRow> = sqlx::query_as(&format!(
        "SELECT u.model, u.provider,
                CAST(COALESCE(SUM(u.input_tokens), 0) AS INTEGER) AS input_tokens,
                CAST(COALESCE(SUM(u.output_tokens), 0) AS INTEGER) AS output_tokens,
                CAST(COALESCE(SUM(u.reasoning_tokens), 0) AS INTEGER) AS reasoning_tokens,
                CAST(COALESCE(SUM(u.cache_read_tokens), 0) AS INTEGER) AS cache_read_tokens,
                CAST(COALESCE(SUM(u.cache_write_tokens), 0) AS INTEGER) AS cache_write_tokens,
                CAST(COALESCE(SUM(u.cost), 0.0) AS REAL) AS cost,
                CAST(COALESCE(SUM({ZERO_COST_TOKENS_SQL}), 0) AS INTEGER) AS zero_cost_tokens
         FROM usage_sessions u
         WHERE lower({RESOLVED_AGENT_SQL}) = lower(?1)
           AND (?2 IS NULL OR u.day >= ?2)
           AND u.day < date(?3, '+1 day')
         GROUP BY u.model, u.provider"
    ))
    .bind(&name)
    .bind(&window.from)
    .bind(&window.to)
    .fetch_all(db)
    .await?;

    let now = Utc::now();
    let month_from = bucket_start(now - chrono::Duration::days(30));
    let allowance_rows =
        allowance_buckets(db, UsageScope::Agent(&name), billing, &month_from).await?;
    let rollup = rollup_allowance(allowance_rows, billing, window, now, &month_from);

    let totals = totals_from_rows(
        &totals_rows,
        billing,
        session_count,
        rollup.range_usd,
        rollup.range_unrated,
    );
    let basis = share_basis(totals.cost, totals.allowance_usd);
    let models = model_shares(
        model_rows,
        &rollup,
        billing,
        basis,
        totals.cost + totals.allowance_usd,
        totals.total_tokens,
    );

    let sessions = if detailed {
        session_rows(db, Some(&name), None, window, billing).await?
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
        share_basis: basis,
        models,
        sessions,
        allowances: rollup.allowances,
        over_allowance: rollup.over_allowance,
        agents: Vec::new(),
    })
}

async fn issue_summary(
    db: &sqlx::SqlitePool,
    key: &str,
    window: &Window,
    threshold: f64,
    billing: &UsageConfig,
) -> AppResult<UsageSummary> {
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM issues WHERE key = ?1")
        .bind(key)
        .fetch_optional(db)
        .await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }

    let totals_rows: Vec<ProviderTotalsRow> = sqlx::query_as(&format!(
        "SELECT u.provider,
            CAST(COALESCE(SUM(u.input_tokens), 0) AS INTEGER) AS input_tokens,
            CAST(COALESCE(SUM(u.output_tokens), 0) AS INTEGER) AS output_tokens,
            CAST(COALESCE(SUM(u.reasoning_tokens), 0) AS INTEGER) AS reasoning_tokens,
            CAST(COALESCE(SUM(u.cache_read_tokens), 0) AS INTEGER) AS cache_read_tokens,
            CAST(COALESCE(SUM(u.cache_write_tokens), 0) AS INTEGER) AS cache_write_tokens,
            CAST(COALESCE(SUM(u.cost), 0.0) AS REAL) AS cost,
            CAST(COALESCE(SUM({ZERO_COST_TOKENS_SQL}), 0) AS INTEGER) AS zero_cost_tokens
         FROM usage_sessions u
         JOIN usage_issue_links c ON c.session_id = u.session_id
         JOIN issues i ON i.id = c.issue_id
         WHERE i.key = ?1
           AND (?2 IS NULL OR u.day >= ?2)
           AND u.day < date(?3, '+1 day')
         GROUP BY u.provider"
    ))
    .bind(key)
    .bind(&window.from)
    .bind(&window.to)
    .fetch_all(db)
    .await?;

    let session_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT u.session_id)
           FROM usage_sessions u
           JOIN usage_issue_links c ON c.session_id = u.session_id
           JOIN issues i ON i.id = c.issue_id
          WHERE i.key = ?1
            AND (?2 IS NULL OR u.day >= ?2)
            AND u.day < date(?3, '+1 day')",
    )
    .bind(key)
    .bind(&window.from)
    .bind(&window.to)
    .fetch_one(db)
    .await?;

    let model_rows: Vec<ModelRow> = sqlx::query_as(&format!(
        "SELECT u.model, u.provider,
                CAST(COALESCE(SUM(u.input_tokens), 0) AS INTEGER) AS input_tokens,
                CAST(COALESCE(SUM(u.output_tokens), 0) AS INTEGER) AS output_tokens,
                CAST(COALESCE(SUM(u.reasoning_tokens), 0) AS INTEGER) AS reasoning_tokens,
                CAST(COALESCE(SUM(u.cache_read_tokens), 0) AS INTEGER) AS cache_read_tokens,
                CAST(COALESCE(SUM(u.cache_write_tokens), 0) AS INTEGER) AS cache_write_tokens,
                CAST(COALESCE(SUM(u.cost), 0.0) AS REAL) AS cost,
                CAST(COALESCE(SUM({ZERO_COST_TOKENS_SQL}), 0) AS INTEGER) AS zero_cost_tokens
         FROM usage_sessions u
         JOIN usage_issue_links c ON c.session_id = u.session_id
         JOIN issues i ON i.id = c.issue_id
         WHERE i.key = ?1
           AND (?2 IS NULL OR u.day >= ?2)
           AND u.day < date(?3, '+1 day')
         GROUP BY u.model, u.provider"
    ))
    .bind(key)
    .bind(&window.from)
    .bind(&window.to)
    .fetch_all(db)
    .await?;

    let agent_rows: Vec<AgentRow> = sqlx::query_as(&format!(
        "SELECT {RESOLVED_AGENT_SQL} AS agent_name, u.provider,
                CAST(COALESCE(SUM(u.input_tokens), 0) AS INTEGER) AS input_tokens,
                CAST(COALESCE(SUM(u.output_tokens), 0) AS INTEGER) AS output_tokens,
                CAST(COALESCE(SUM(u.reasoning_tokens), 0) AS INTEGER) AS reasoning_tokens,
                CAST(COALESCE(SUM(u.cache_read_tokens), 0) AS INTEGER) AS cache_read_tokens,
                CAST(COALESCE(SUM(u.cache_write_tokens), 0) AS INTEGER) AS cache_write_tokens,
                CAST(COALESCE(SUM(u.cost), 0.0) AS REAL) AS cost,
                CAST(COALESCE(SUM({ZERO_COST_TOKENS_SQL}), 0) AS INTEGER) AS zero_cost_tokens
         FROM usage_sessions u
         JOIN usage_issue_links c ON c.session_id = u.session_id
         JOIN issues i ON i.id = c.issue_id
         WHERE i.key = ?1
           AND (?2 IS NULL OR u.day >= ?2)
           AND u.day < date(?3, '+1 day')
         GROUP BY {RESOLVED_AGENT_SQL}, u.provider"
    ))
    .bind(key)
    .bind(&window.from)
    .bind(&window.to)
    .fetch_all(db)
    .await?;

    let now = Utc::now();
    let month_from = bucket_start(now - chrono::Duration::days(30));
    let allowance_rows =
        allowance_buckets(db, UsageScope::Issue(key), billing, &month_from).await?;
    let rollup = rollup_allowance(allowance_rows, billing, window, now, &month_from);

    let totals = totals_from_rows(
        &totals_rows,
        billing,
        session_count,
        rollup.range_usd,
        rollup.range_unrated,
    );
    let basis = share_basis(totals.cost, totals.allowance_usd);
    let total_dollars = totals.cost + totals.allowance_usd;
    let models = model_shares(
        model_rows,
        &rollup,
        billing,
        basis,
        total_dollars,
        totals.total_tokens,
    );
    let agents = agent_shares(
        agent_rows,
        &rollup,
        billing,
        basis,
        total_dollars,
        totals.total_tokens,
    );
    let sessions = session_rows(db, None, Some(key), window, billing).await?;

    Ok(UsageSummary {
        subject: key.to_string(),
        kind: "issue".into(),
        range: window.label.clone(),
        from: window.from.clone(),
        to: window.to.clone(),
        over_threshold: threshold > 0.0 && totals.cost > threshold,
        spend_warn_usd: threshold,
        totals,
        share_basis: basis,
        models,
        sessions,
        allowances: rollup.allowances,
        over_allowance: rollup.over_allowance,
        agents,
    })
}

#[derive(Debug, Clone, Copy)]
enum UsageScope<'a> {
    Agent(&'a str),
    Issue(&'a str),
}

/// Providers configured with allowance billing, lowercased for a SQL `IN`.
fn allowance_provider_keys(billing: &UsageConfig) -> Vec<String> {
    let mut keys: Vec<String> = billing
        .providers
        .iter()
        .filter(|(_, cfg)| cfg.mode == BillingMode::Allowance)
        .map(|(key, _)| key.trim().to_ascii_lowercase())
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// Allowance-provider buckets since the start of the rolling monthly cycle.
/// Billing is computed in Rust so a config change re-interprets history with no
/// re-import.
async fn allowance_buckets(
    db: &sqlx::SqlitePool,
    scope: UsageScope<'_>,
    billing: &UsageConfig,
    month_from: &str,
) -> AppResult<Vec<AllowanceBucketRow>> {
    let providers = allowance_provider_keys(billing);
    if providers.is_empty() {
        return Ok(Vec::new());
    }
    let list = (3..3 + providers.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sums = "CAST(COALESCE(SUM(u.input_tokens), 0) AS INTEGER) AS input_tokens,
            CAST(COALESCE(SUM(u.output_tokens), 0) AS INTEGER) AS output_tokens,
            CAST(COALESCE(SUM(u.reasoning_tokens), 0) AS INTEGER) AS reasoning_tokens,
            CAST(COALESCE(SUM(u.cache_read_tokens), 0) AS INTEGER) AS cache_read_tokens,
            CAST(COALESCE(SUM(u.cache_write_tokens), 0) AS INTEGER) AS cache_write_tokens";
    let (sql, subject) = match scope {
        UsageScope::Agent(name) => (
            format!(
                "SELECT {RESOLVED_AGENT_SQL} AS agent_name, u.provider, u.model, u.day, {sums}
                   FROM usage_sessions u
                  WHERE lower({RESOLVED_AGENT_SQL}) = lower(?1)
                    AND u.day >= ?2
                    AND lower(u.provider) IN ({list})
                  GROUP BY {RESOLVED_AGENT_SQL}, u.provider, u.model, u.day"
            ),
            name.to_string(),
        ),
        UsageScope::Issue(key) => (
            format!(
                "SELECT {RESOLVED_AGENT_SQL} AS agent_name, u.provider, u.model, u.day, {sums}
                   FROM usage_sessions u
                   JOIN usage_issue_links c ON c.session_id = u.session_id
                   JOIN issues i ON i.id = c.issue_id
                  WHERE i.key = ?1
                    AND u.day >= ?2
                    AND lower(u.provider) IN ({list})
                  GROUP BY {RESOLVED_AGENT_SQL}, u.provider, u.model, u.day"
            ),
            key.to_string(),
        ),
    };
    let mut query = sqlx::query_as::<_, AllowanceBucketRow>(&sql)
        .bind(subject)
        .bind(month_from);
    for provider in &providers {
        query = query.bind(provider);
    }
    Ok(query.fetch_all(db).await?)
}

#[derive(Debug, Clone, Default)]
struct AllowanceModelAcc {
    provider: String,
    model: String,
    range_usd: f64,
    range_unrated: i64,
    range_tokens: i64,
    five_usd: f64,
    week_usd: f64,
    month_usd: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct AgentAllowanceAcc {
    usd: f64,
    unrated_tokens: i64,
}

#[derive(Debug, Clone, Default)]
struct AllowanceRollup {
    range_usd: f64,
    range_unrated: i64,
    by_model: HashMap<(String, String), AllowanceModelAcc>,
    by_agent: HashMap<String, AgentAllowanceAcc>,
    allowances: Vec<ModelAllowance>,
    over_allowance: bool,
}

fn rollup_allowance(
    rows: Vec<AllowanceBucketRow>,
    billing: &UsageConfig,
    window: &Window,
    now: chrono::DateTime<Utc>,
    month_from: &str,
) -> AllowanceRollup {
    let week_from = bucket_start(now - chrono::Duration::days(7));
    let five_from = bucket_start(now - chrono::Duration::hours(5));
    let now_bucket = bucket_start(now);
    let to_exclusive = next_day(&window.to);

    let mut by_model: HashMap<(String, String), AllowanceModelAcc> = HashMap::new();
    let mut by_agent: HashMap<String, AgentAllowanceAcc> = HashMap::new();
    let mut order: Vec<(String, String)> = Vec::new();

    for row in rows {
        let Some(cfg) = billing.provider_for(row.provider.as_deref()) else {
            continue;
        };
        if cfg.mode != BillingMode::Allowance {
            continue;
        }
        let provider = row.provider.clone().unwrap_or_default();
        let key = (provider.to_ascii_lowercase(), row.model.clone());
        if !by_model.contains_key(&key) {
            order.push(key.clone());
        }
        let tokens = row.split();
        let charge = allowance_charge(cfg, &row.model, &row.day, tokens);
        let in_range = bucket_in_range(&row.day, window.from.as_deref(), &to_exclusive);

        let acc = by_model.entry(key).or_insert_with(|| AllowanceModelAcc {
            provider,
            model: row.model.clone(),
            ..AllowanceModelAcc::default()
        });
        acc.range_tokens += tokens.total();
        if in_range {
            acc.range_usd += charge.usd;
            acc.range_unrated += charge.unrated_tokens;
        }
        if row.day >= five_from {
            acc.five_usd += charge.usd;
        }
        if row.day >= week_from {
            acc.week_usd += charge.usd;
        }
        acc.month_usd += charge.usd;

        if in_range {
            let agent = by_agent.entry(row.agent_name.clone()).or_default();
            agent.usd += charge.usd;
            agent.unrated_tokens += charge.unrated_tokens;
        }
    }

    let mut allowances = Vec::with_capacity(order.len());
    let mut over_allowance = false;
    for key in &order {
        let acc = &by_model[key];
        let cfg = billing.provider_for(Some(&acc.provider));
        let allowance = cfg
            .and_then(|c| c.model_for(&acc.model))
            .map(|m| m.allowance_usd)
            .unwrap_or(0.0);
        let plan_price = cfg.map(|c| c.plan_price_usd).unwrap_or(0.0);
        // Leverage of the allowance over the plan price, not a price multiplier.
        let multiplier = if plan_price > 0.0 {
            (allowance / plan_price * 100.0).round() / 100.0
        } else {
            0.0
        };
        let windows = vec![
            allowance_window("5h", acc.five_usd, allowance, 0.20, &five_from, &now_bucket),
            allowance_window(
                "weekly",
                acc.week_usd,
                allowance,
                0.50,
                &week_from,
                &now_bucket,
            ),
            allowance_window(
                "monthly",
                acc.month_usd,
                allowance,
                1.0,
                month_from,
                &now_bucket,
            ),
        ];
        over_allowance |= windows.iter().any(|w| w.over_limit);
        allowances.push(ModelAllowance {
            provider: acc.provider.clone(),
            model: acc.model.clone(),
            allowance_usd: round_cost(allowance),
            plan_price_usd: round_cost(plan_price),
            multiplier,
            used_usd: round_cost(acc.range_usd),
            unrated_tokens: acc.range_unrated,
            windows,
        });
    }
    allowances.sort_by(|a, b| {
        b.used_usd
            .partial_cmp(&a.used_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.model.cmp(&b.model))
    });

    let range_usd = by_model.values().map(|acc| acc.range_usd).sum();
    let range_unrated = by_model.values().map(|acc| acc.range_unrated).sum();
    AllowanceRollup {
        range_usd: round_cost(range_usd),
        range_unrated,
        by_model,
        by_agent,
        allowances,
        over_allowance,
    }
}

fn allowance_window(
    label: &str,
    used: f64,
    allowance: f64,
    fraction: f64,
    from: &str,
    to: &str,
) -> AllowanceWindow {
    let limit = allowance * fraction;
    AllowanceWindow {
        label: label.to_string(),
        used_usd: round_cost(used),
        limit_usd: round_cost(limit),
        over_limit: limit > 0.0 && used > limit,
        from: from.to_string(),
        to: to.to_string(),
    }
}

/// Cost of one bucket at that provider's rates. A model with no allowance entry
/// cannot be priced; its tokens are reported as unrated rather than guessed.
fn allowance_charge(
    provider: &ProviderBillingConfig,
    model: &str,
    bucket: &str,
    tokens: TokenSplit,
) -> AllowanceCharge {
    let Some(model_cfg) = provider.model_for(model) else {
        return AllowanceCharge {
            usd: 0.0,
            unrated_tokens: tokens.total(),
        };
    };
    let rates = effective_rates(provider, &model_cfg.rates, bucket);
    let usd = (tokens.input as f64 * rates.input
        + (tokens.output + tokens.reasoning) as f64 * rates.output
        + tokens.cache_read as f64 * rates.cache_read
        + tokens.cache_write as f64 * rates.cache_write)
        / 1_000_000.0;
    AllowanceCharge {
        usd,
        unrated_tokens: 0,
    }
}

/// Peak rates unless the bucket falls inside the provider's off-peak window and
/// that provider has an off-peak table. Buckets without a time of day (rows
/// imported before 30-minute buckets existed) are charged at peak rates, the
/// conservative choice.
fn effective_rates<'a>(
    provider: &ProviderBillingConfig,
    peak: &'a TokenRates,
    bucket: &str,
) -> &'a TokenRates {
    let Some(at) = bucket_minutes(bucket) else {
        return peak;
    };
    let (Some(start), Some(end)) = (
        parse_hhmm(provider.off_peak_start_utc.as_deref()),
        parse_hhmm(provider.off_peak_end_utc.as_deref()),
    ) else {
        return peak;
    };
    if in_window(at, start, end) {
        peak.off_peak.as_deref().unwrap_or(peak)
    } else {
        peak
    }
}

fn bucket_minutes(bucket: &str) -> Option<u32> {
    let (_, time) = bucket.split_once('T')?;
    let (hour, minute) = time.split_once(':')?;
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.get(..2)?.parse().ok()?;
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}

fn parse_hhmm(value: Option<&str>) -> Option<u32> {
    let (hour, minute) = value?.trim().split_once(':')?;
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.parse().ok()?;
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}

fn in_window(at: u32, start: u32, end: u32) -> bool {
    if start <= end {
        at >= start && at < end
    } else {
        at >= start || at < end
    }
}

/// 30-minute UTC bucket, matching how the importer stores `usage_sessions.day`.
fn bucket_start(at: chrono::DateTime<Utc>) -> String {
    let minute = if at.minute() < 30 { 0 } else { 30 };
    format!(
        "{}T{:02}:{:02}",
        at.date_naive().format("%Y-%m-%d"),
        at.hour(),
        minute
    )
}

fn next_day(day: &str) -> String {
    NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .map(|d| {
            (d + chrono::Duration::days(1))
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|_| day.to_string())
}

fn bucket_in_range(bucket: &str, from: Option<&str>, to_exclusive: &str) -> bool {
    from.is_none_or(|from| bucket >= from) && bucket < to_exclusive
}

async fn session_rows(
    db: &sqlx::SqlitePool,
    agent: Option<&str>,
    issue_key: Option<&str>,
    window: &Window,
    billing: &UsageConfig,
) -> AppResult<Vec<SessionUsage>> {
    let rows: Vec<SessionBucketRow> = sqlx::query_as(&format!(
        "SELECT u.session_id,
                MIN({RESOLVED_AGENT_SQL}) AS agent_name,
                u.provider,
                u.model,
                u.day,
                CAST(COALESCE(SUM(u.input_tokens), 0) AS INTEGER) AS input_tokens,
                CAST(COALESCE(SUM(u.output_tokens), 0) AS INTEGER) AS output_tokens,
                CAST(COALESCE(SUM(u.reasoning_tokens), 0) AS INTEGER) AS reasoning_tokens,
                CAST(COALESCE(SUM(u.cache_read_tokens), 0) AS INTEGER) AS cache_read_tokens,
                CAST(COALESCE(SUM(u.cache_write_tokens), 0) AS INTEGER) AS cache_write_tokens,
                CAST(COALESCE(SUM(u.cost), 0.0) AS REAL) AS cost,
                CAST(COALESCE(SUM({ZERO_COST_TOKENS_SQL}), 0) AS INTEGER) AS zero_cost_tokens
         FROM usage_sessions u
         WHERE (?1 IS NULL OR lower({RESOLVED_AGENT_SQL}) = lower(?1))
           AND (?2 IS NULL OR u.session_id IN (
                SELECT DISTINCT l.session_id FROM usage_issue_links l
                JOIN issues i ON i.id = l.issue_id
                WHERE i.key = ?2))
           AND (?3 IS NULL OR u.day >= ?3)
           AND u.day < date(?4, '+1 day')
         GROUP BY u.session_id, u.provider, u.model, u.day"
    ))
    .bind(agent)
    .bind(issue_key)
    .bind(&window.from)
    .bind(&window.to)
    .fetch_all(db)
    .await?;

    let mut order: Vec<String> = Vec::new();
    let mut merged: HashMap<String, SessionAcc> = HashMap::new();
    for row in rows {
        if !merged.contains_key(&row.session_id) {
            order.push(row.session_id.clone());
        }
        let acc = merged
            .entry(row.session_id.clone())
            .or_insert_with(|| SessionAcc {
                session_id: row.session_id.clone(),
                ..SessionAcc::default()
            });
        acc.add(&row, billing);
    }
    let mut out: Vec<SessionUsage> = order
        .into_iter()
        .filter_map(|session_id| merged.remove(&session_id))
        .map(|acc| SessionUsage {
            session_id: acc.session_id,
            agent_name: acc.agent_name,
            cost: round_cost(acc.cost),
            allowance_usd: round_cost(acc.allowance_usd),
            unpriced_tokens: acc.unpriced_tokens,
            unrated_tokens: acc.unrated_tokens,
            total_tokens: acc.total_tokens,
            first_day: acc.first_day,
            last_day: acc.last_day,
            models: acc.models.into_iter().collect(),
        })
        .collect();
    out.sort_by(|a, b| {
        b.last_day
            .cmp(&a.last_day)
            .then(b.total_tokens.cmp(&a.total_tokens))
            .then(a.session_id.cmp(&b.session_id))
    });
    out.truncate(50);
    Ok(out)
}

#[derive(Debug, Default)]
struct SessionAcc {
    session_id: String,
    agent_name: Option<String>,
    cost: f64,
    allowance_usd: f64,
    unpriced_tokens: i64,
    unrated_tokens: i64,
    total_tokens: i64,
    first_day: String,
    last_day: String,
    models: std::collections::BTreeSet<String>,
}

impl SessionAcc {
    fn add(&mut self, row: &SessionBucketRow, billing: &UsageConfig) {
        if self.agent_name.is_none() {
            self.agent_name.clone_from(&row.agent_name);
        }
        let day = row.day.get(..10).unwrap_or(&row.day);
        if self.first_day.is_empty() || day < self.first_day.as_str() {
            self.first_day = day.to_string();
        }
        if day > self.last_day.as_str() {
            self.last_day = day.to_string();
        }
        let tokens = row.split();
        match billing.billing_for(row.provider.as_deref()) {
            BillingMode::Metered => {
                self.cost += row.cost;
                self.unpriced_tokens += row.zero_cost_tokens;
            }
            BillingMode::Allowance => match billing.provider_for(row.provider.as_deref()) {
                Some(cfg) => {
                    let charge = allowance_charge(cfg, &row.model, &row.day, tokens);
                    self.allowance_usd += charge.usd;
                    self.unrated_tokens += charge.unrated_tokens;
                }
                None => self.unrated_tokens += tokens.total(),
            },
        }
        self.total_tokens += tokens.total();
        self.models.insert(row.model.clone());
    }
}

fn model_shares(
    rows: Vec<ModelRow>,
    allowance: &AllowanceRollup,
    billing: &UsageConfig,
    basis: ShareBasis,
    total_dollars: f64,
    total_tokens: i64,
) -> Vec<ModelShare> {
    let mut order: Vec<(String, String)> = Vec::new();
    let mut merged: HashMap<(String, String), ModelShare> = HashMap::new();

    for row in rows {
        let mode = billing.billing_for(row.provider.as_deref());
        let tokens = row.split().total();
        let (cost, unpriced_tokens) = if mode == BillingMode::Metered {
            (row.cost, row.zero_cost_tokens)
        } else {
            (0.0, 0)
        };
        let key = (
            row.provider
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase(),
            row.model.clone(),
        );
        if !merged.contains_key(&key) {
            order.push(key.clone());
        }
        merged.insert(
            key,
            ModelShare {
                model: row.model,
                provider: row.provider,
                billing: mode,
                cost: round_cost(cost),
                allowance_usd: 0.0,
                unpriced_tokens,
                unrated_tokens: 0,
                total_tokens: tokens,
                share: 0.0,
            },
        );
    }

    for (key, acc) in &allowance.by_model {
        if let Some(share) = merged.get_mut(key) {
            share.billing = BillingMode::Allowance;
            share.allowance_usd = round_cost(acc.range_usd);
            share.unrated_tokens = acc.range_unrated;
            continue;
        }
        order.push(key.clone());
        merged.insert(
            key.clone(),
            ModelShare {
                model: acc.model.clone(),
                provider: Some(acc.provider.clone()),
                billing: BillingMode::Allowance,
                cost: 0.0,
                allowance_usd: round_cost(acc.range_usd),
                unpriced_tokens: 0,
                unrated_tokens: acc.range_unrated,
                total_tokens: acc.range_tokens,
                share: 0.0,
            },
        );
    }

    let mut out: Vec<ModelShare> = order
        .into_iter()
        .filter_map(|key| merged.remove(&key))
        .map(|mut row| {
            row.share = share(
                row.cost + row.allowance_usd,
                row.total_tokens,
                basis,
                total_dollars,
                total_tokens,
            );
            row
        })
        .collect();
    out.sort_by(|a, b| {
        b.share
            .partial_cmp(&a.share)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.total_tokens.cmp(&a.total_tokens))
            .then(a.model.cmp(&b.model))
    });
    out
}

fn agent_shares(
    rows: Vec<AgentRow>,
    allowance: &AllowanceRollup,
    billing: &UsageConfig,
    basis: ShareBasis,
    total_dollars: f64,
    total_tokens: i64,
) -> Vec<AgentShare> {
    let mut order: Vec<String> = Vec::new();
    struct Acc {
        cost: f64,
        allowance_usd: f64,
        unpriced_tokens: i64,
        unrated_tokens: i64,
        total_tokens: i64,
    }
    let mut merged: HashMap<String, Acc> = HashMap::new();
    for row in rows {
        let agent_name = row.agent_name.clone();
        let tokens = row.split().total();
        if !merged.contains_key(&agent_name) {
            order.push(agent_name.clone());
        }
        let acc = merged.entry(agent_name).or_insert(Acc {
            cost: 0.0,
            allowance_usd: 0.0,
            unpriced_tokens: 0,
            unrated_tokens: 0,
            total_tokens: 0,
        });
        if billing.billing_for(row.provider.as_deref()) == BillingMode::Metered {
            acc.cost += row.cost;
            acc.unpriced_tokens += row.zero_cost_tokens;
        }
        acc.total_tokens += tokens;
    }
    for (agent_name, allowance_acc) in &allowance.by_agent {
        if !merged.contains_key(agent_name) {
            order.push(agent_name.clone());
        }
        let acc = merged.entry(agent_name.clone()).or_insert(Acc {
            cost: 0.0,
            allowance_usd: 0.0,
            unpriced_tokens: 0,
            unrated_tokens: 0,
            total_tokens: 0,
        });
        acc.allowance_usd += allowance_acc.usd;
        acc.unrated_tokens += allowance_acc.unrated_tokens;
    }
    let mut out: Vec<AgentShare> = order
        .into_iter()
        .filter_map(|agent_name| {
            merged.remove(&agent_name).map(|acc| AgentShare {
                agent_name,
                cost: round_cost(acc.cost),
                allowance_usd: round_cost(acc.allowance_usd),
                unpriced_tokens: acc.unpriced_tokens,
                unrated_tokens: acc.unrated_tokens,
                total_tokens: acc.total_tokens,
                share: share(
                    acc.cost + acc.allowance_usd,
                    acc.total_tokens,
                    basis,
                    total_dollars,
                    total_tokens,
                ),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.share
            .partial_cmp(&a.share)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.total_tokens.cmp(&a.total_tokens))
            .then(a.agent_name.cmp(&b.agent_name))
    });
    out
}

/// Cost share is only meaningful while every dollar in the subject is metered
/// reported cost. Allowance consumption is a different dollar basis, so a mix
/// is labelled explicitly rather than pretending the two are the same number.
fn share_basis(total_cost: f64, total_allowance: f64) -> ShareBasis {
    match (total_cost > 0.0, total_allowance > 0.0) {
        (true, true) => ShareBasis::Dollars,
        (false, true) => ShareBasis::Allowance,
        (true, false) => ShareBasis::Cost,
        (false, false) => ShareBasis::Tokens,
    }
}

fn share(
    dollars: f64,
    tokens: i64,
    basis: ShareBasis,
    total_dollars: f64,
    total_tokens: i64,
) -> f64 {
    let value = match basis {
        ShareBasis::Cost | ShareBasis::Allowance | ShareBasis::Dollars if total_dollars > 0.0 => {
            dollars / total_dollars
        }
        _ if total_tokens > 0 => tokens as f64 / total_tokens as f64,
        _ => 0.0,
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
    fn off_peak_window_wraps_midnight() {
        // 16:30-00:30 UTC.
        assert!(in_window(17 * 60, 16 * 60 + 30, 30));
        assert!(in_window(15, 16 * 60 + 30, 30));
        assert!(!in_window(10 * 60, 16 * 60 + 30, 30));
        assert!(!in_window(16 * 60 + 29, 16 * 60 + 30, 30));
        assert!(!in_window(30, 16 * 60 + 30, 30));
    }

    #[test]
    fn allowance_charges_use_peak_rates_without_a_time_of_day() {
        let provider = ProviderBillingConfig {
            mode: BillingMode::Allowance,
            plan_price_usd: 10.0,
            off_peak_start_utc: Some("16:30".into()),
            off_peak_end_utc: Some("00:30".into()),
            models: HashMap::from([(
                "deepseek-v4.1-flash".to_string(),
                crate::config::ModelAllowanceConfig {
                    allowance_usd: 100.0,
                    rates: TokenRates {
                        input: 0.30,
                        output: 1.20,
                        cache_read: 0.006,
                        cache_write: 0.30,
                        off_peak: Some(Box::new(TokenRates {
                            input: 0.15,
                            output: 0.60,
                            cache_read: 0.003,
                            cache_write: 0.15,
                            off_peak: None,
                        })),
                    },
                },
            )]),
        };
        let tokens = TokenSplit {
            input: 1_000_000,
            ..TokenSplit::default()
        };
        let peak = allowance_charge(&provider, "deepseek-v4.1-flash", "2026-11-10T10:00", tokens);
        assert!((peak.usd - 0.30).abs() < 1e-9, "peak: {}", peak.usd);
        let off = allowance_charge(&provider, "deepseek-v4.1-flash", "2026-11-10T18:00", tokens);
        assert!((off.usd - 0.15).abs() < 1e-9, "off-peak: {}", off.usd);
        // Rows imported before 30-minute buckets existed carry only a date; the
        // conservative choice is peak rates.
        let legacy = allowance_charge(&provider, "deepseek-v4.1-flash", "2026-11-10", tokens);
        assert!((legacy.usd - 0.30).abs() < 1e-9, "legacy: {}", legacy.usd);
        let unlisted = allowance_charge(&provider, "other-model", "2026-11-10T10:00", tokens);
        assert_eq!(unlisted.usd, 0.0);
        assert_eq!(unlisted.unrated_tokens, 1_000_000);
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
