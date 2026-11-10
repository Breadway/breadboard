//! Daily roster hygiene: remove agent rows nobody is using.
//!
//! A finished subagent leaves an `agent_sessions` row behind, and nothing used
//! to remove it after its OpenCode session disappeared. This pass deletes a
//! row only when every signal says it is abandoned:
//!
//! - no exclusive or intent claim,
//! - no chat room membership or DM,
//! - no open review request as the reviewer,
//! - not paused by hand,
//! - no heartbeat inside the configured `agents.prune_after_hours` window.
//!
//! A sessionless row (no `session_id`) is covered by the same rule. When the
//! OpenCode database is readable, the audit reason also records whether the
//! row's session id still exists there. Every removal writes one
//! `agent_removals` row with the name and the reason, so the cleanup is
//! auditable after the roster row is gone.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{FromRow, SqlitePool};

use crate::error::AppResult;
use crate::AppState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrunedAgent {
    pub agent_name: String,
    pub reason: String,
}

#[derive(Debug, FromRow)]
struct Candidate {
    agent_name: String,
    session_id: Option<String>,
    last_seen: String,
}

fn parse_ts(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// True when this row would be pruned under the four protections above. The
/// SQL candidate query applies the same rule; this helper exists for the
/// roster flag and for tests.
pub fn is_abandoned(
    claim_count: i64,
    paused: bool,
    has_chat: bool,
    has_open_review: bool,
    last_seen: Option<DateTime<Utc>>,
    prune_after_hours: u64,
    now: DateTime<Utc>,
) -> bool {
    if prune_after_hours == 0 || paused || claim_count > 0 || has_chat || has_open_review {
        return false;
    }
    let window = (prune_after_hours as i64).saturating_mul(3600);
    last_seen
        .map(|at| (now - at).num_seconds() >= window)
        .unwrap_or(false)
}

/// Remove every abandoned session row and return what was pruned. The caller
/// supplies the OpenCode session ids when they are readable; they are only
/// used to make the audit reason precise, never to weaken the idle window.
pub async fn prune_abandoned_agents(
    db: &SqlitePool,
    prune_after_hours: u64,
    opencode_sessions: Option<&HashSet<String>>,
) -> AppResult<Vec<PrunedAgent>> {
    if prune_after_hours == 0 {
        return Ok(Vec::new());
    }
    let now = Utc::now();
    let cutoff = (now - chrono::Duration::hours(prune_after_hours as i64)).to_rfc3339();

    let candidates: Vec<Candidate> = sqlx::query_as(
        "SELECT s.agent_name, s.session_id, s.last_seen
           FROM agent_sessions s
          WHERE s.paused = 0
            AND julianday(s.last_seen) < julianday(?1)
            AND NOT EXISTS (
                SELECT 1 FROM agent_claims c
                 WHERE lower(c.agent_name) = s.agent_name
            )
            AND NOT EXISTS (
                SELECT 1 FROM chat_room_members m
                 WHERE m.member = 'agent:' || s.agent_name
            )
            AND NOT EXISTS (
                SELECT 1 FROM review_requests r
                 WHERE lower(r.reviewer) = s.agent_name
                   AND r.state != 'approved'
            )
          ORDER BY s.agent_name",
    )
    .bind(&cutoff)
    .fetch_all(db)
    .await?;

    let mut pruned = Vec::new();
    for candidate in candidates {
        let idle_hours = parse_ts(&candidate.last_seen)
            .map(|at| (now - at).num_hours().max(0))
            .unwrap_or(0);
        let session_note = match (&candidate.session_id, opencode_sessions) {
            (None, _) => "sessionless",
            (Some(id), Some(sessions)) if !sessions.contains(id) => "session missing in OpenCode",
            (Some(_), Some(_)) => "session present in OpenCode",
            (Some(_), None) => "OpenCode session store unavailable",
        };
        let reason =
            format!("prune: no claims, chat or reviews; idle {idle_hours}h; {session_note}");

        // Re-check the protections in the write transaction. A claim, DM or
        // review that landed after the select makes this delete a no-op.
        let mut tx = crate::db::begin_write(db).await?;
        let deleted = sqlx::query(
            "DELETE FROM agent_sessions
              WHERE agent_name = ?1
                AND paused = 0
                AND julianday(last_seen) < julianday(?2)
                AND NOT EXISTS (
                    SELECT 1 FROM agent_claims c
                     WHERE lower(c.agent_name) = agent_sessions.agent_name
                )
                AND NOT EXISTS (
                    SELECT 1 FROM chat_room_members m
                     WHERE m.member = 'agent:' || agent_sessions.agent_name
                )
                AND NOT EXISTS (
                    SELECT 1 FROM review_requests r
                     WHERE lower(r.reviewer) = agent_sessions.agent_name
                       AND r.state != 'approved'
                )",
        )
        .bind(&candidate.agent_name)
        .bind(&cutoff)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if deleted == 0 {
            tx.rollback().await?;
            continue;
        }

        let profile_removed =
            sqlx::query("DELETE FROM agent_profiles WHERE lower(agent_name) = ?1")
                .bind(&candidate.agent_name)
                .execute(&mut *tx)
                .await?
                .rows_affected()
                > 0;
        sqlx::query("DELETE FROM agent_hidden WHERE lower(agent_name) = ?1")
            .bind(&candidate.agent_name)
            .execute(&mut *tx)
            .await?;
        let member = format!("agent:{}", candidate.agent_name);
        sqlx::query("DELETE FROM chat_pushes WHERE member = ?1")
            .bind(&member)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM chat_push_subs WHERE member = ?1")
            .bind(&member)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM chat_reads WHERE member = ?1")
            .bind(&member)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM chat_room_members WHERE member = ?1")
            .bind(&member)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO agent_removals
                (agent_name, removed_by, removed_at, reason, claims_released, profile_removed)
             VALUES (?1, NULL, ?2, ?3, 0, ?4)",
        )
        .bind(&candidate.agent_name)
        .bind(now.to_rfc3339())
        .bind(&reason)
        .bind(i64::from(profile_removed))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        crate::agent_events::record_best_effort(
            db,
            crate::agent_events::AgentEventInsert::new(
                &candidate.agent_name,
                breadboard_core::AgentEventKind::Removed,
            )
            .summary("Pruned as abandoned by roster maintenance")
            .detail(reason.clone()),
        )
        .await;

        pruned.push(PrunedAgent {
            agent_name: candidate.agent_name,
            reason,
        });
    }
    Ok(pruned)
}

/// Session ids that still exist in the configured OpenCode database. `None`
/// means the store is absent, unreadable or not configured, so the audit
/// reason cannot claim a session is missing.
pub async fn opencode_session_ids(state: &AppState) -> Option<HashSet<String>> {
    let path = state.config.usage.opencode_db.as_ref()?;
    read_opencode_session_ids(path).await
}

async fn read_opencode_session_ids(path: &Path) -> Option<HashSet<String>> {
    if !path.exists() {
        return None;
    }
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .busy_timeout(StdDuration::from_secs(2));
    let source = match SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
    {
        Ok(pool) => pool,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "agent prune: could not read OpenCode database");
            return None;
        }
    };
    let ids: Result<Vec<String>, _> = sqlx::query_scalar("SELECT id FROM session_v2")
        .fetch_all(&source)
        .await;
    source.close().await;
    match ids {
        Ok(ids) => Some(ids.into_iter().collect()),
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "agent prune: could not list OpenCode sessions");
            None
        }
    }
}

/// Run once at startup and then once a day. Cheap: one indexed select and no
/// writes when nothing qualifies.
pub fn spawn_pruner(state: AppState) {
    tokio::spawn(async move {
        prune_once(&state, "startup").await;
        let mut tick = tokio::time::interval(StdDuration::from_secs(24 * 60 * 60));
        tick.tick().await; // the first tick is immediate; startup already ran
        loop {
            tick.tick().await;
            prune_once(&state, "daily").await;
        }
    });
}

async fn prune_once(state: &AppState, reason: &str) {
    let window = state.config.agents.prune_after_hours;
    if window == 0 {
        tracing::info!(reason, "abandoned agent pruning disabled");
        return;
    }
    let sessions = opencode_session_ids(state).await;
    match prune_abandoned_agents(&state.db, window, sessions.as_ref()).await {
        Ok(pruned) if pruned.is_empty() => {}
        Ok(pruned) => {
            tracing::info!(
                count = pruned.len(),
                window_hours = window,
                reason,
                "pruned abandoned agents"
            );
        }
        Err(error) => {
            tracing::warn!(%error, reason, "pruning abandoned agents failed");
        }
    }
}
