//! Periodic roster hygiene: release stale claims, then remove agent rows
//! nobody is using.
//!
//! A finished subagent leaves an `agent_sessions` row behind, and nothing used
//! to remove it after its OpenCode session disappeared. The same is true of a
//! crashed MCP process, whose claim used to pin the row forever. An MCP client
//! that cannot report an OpenCode session id leaves a sessionless row on every
//! session, so the roster accumulates one-shot ghosts even when the client is
//! old. This pass first reaps claims past `agents.claim_reap_after_hours` (see
//! [`crate::claim_reap`]), then deletes a row only when every signal says it is
//! abandoned:
//!
//! - no exclusive or intent claim,
//! - no chat room membership or DM,
//! - no open review request as the reviewer,
//! - not paused by hand,
//! - no heartbeat inside the row's idle window.
//!
//! The idle window depends on whether the row has an OpenCode session id. A
//! sessionless row is a one-shot identity, so it goes after
//! `agents.ephemeral_prune_after_hours` (default 2) unless that is `0`. A
//! session-bound row keeps its history and parent links for
//! `agents.prune_after_hours` (default 24). When the OpenCode database is
//! readable, the audit reason also records whether the row's session id still
//! exists there. Every removal writes one `agent_removals` row with the name
//! and the reason, so the cleanup is auditable after the roster row is gone.
//!
//! The same pass runs once at startup and then every
//! `agents.maintenance_interval_mins` (default 60, `0` disables the periodic
//! runs but keeps the startup pass).

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

/// A cutoff no real heartbeat can fall before, used when a window is
/// disabled. It keeps the candidate and delete SQL identical for every
/// combination of enabled windows.
const NEVER_CUTOFF: &str = "0001-01-01T00:00:00Z";

/// Rows with an OpenCode session id use the long window, because subagent
/// history and parent links are worth keeping. Sessionless rows use the short
/// ephemeral window. Each window is independent: `0` disables removal for its
/// category, and `None` means this row can never be pruned.
pub fn prune_window_hours(
    sessionless: bool,
    prune_after_hours: u64,
    ephemeral_prune_after_hours: u64,
) -> Option<u64> {
    let hours = if sessionless {
        ephemeral_prune_after_hours
    } else {
        prune_after_hours
    };
    if hours == 0 {
        None
    } else {
        Some(hours)
    }
}

/// True when this row would be pruned under the protections above. The SQL
/// candidate query applies the same rule; this helper exists for the roster
/// flag and for tests.
#[allow(clippy::too_many_arguments)]
pub fn is_abandoned(
    claim_count: i64,
    paused: bool,
    has_chat: bool,
    has_open_review: bool,
    sessionless: bool,
    last_seen: Option<DateTime<Utc>>,
    prune_after_hours: u64,
    ephemeral_prune_after_hours: u64,
    now: DateTime<Utc>,
) -> bool {
    if paused || claim_count > 0 || has_chat || has_open_review {
        return false;
    }
    let Some(window_hours) =
        prune_window_hours(sessionless, prune_after_hours, ephemeral_prune_after_hours)
    else {
        return false;
    };
    let window = (window_hours as i64).saturating_mul(3600);
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
    ephemeral_prune_after_hours: u64,
    opencode_sessions: Option<&HashSet<String>>,
) -> AppResult<Vec<PrunedAgent>> {
    if prune_after_hours == 0 && ephemeral_prune_after_hours == 0 {
        return Ok(Vec::new());
    }
    let now = Utc::now();
    let ephemeral_cutoff = if ephemeral_prune_after_hours > 0 {
        (now - chrono::Duration::hours(ephemeral_prune_after_hours as i64)).to_rfc3339()
    } else {
        NEVER_CUTOFF.to_string()
    };
    let prune_cutoff = if prune_after_hours > 0 {
        (now - chrono::Duration::hours(prune_after_hours as i64)).to_rfc3339()
    } else {
        NEVER_CUTOFF.to_string()
    };

    let candidates: Vec<Candidate> = sqlx::query_as(
        "SELECT s.agent_name, s.session_id, s.last_seen
           FROM agent_sessions s
          WHERE s.paused = 0
            AND julianday(s.last_seen) < CASE
                  WHEN s.session_id IS NULL THEN julianday(?1)
                  ELSE julianday(?2)
                END
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
    .bind(&ephemeral_cutoff)
    .bind(&prune_cutoff)
    .fetch_all(db)
    .await?;

    let mut pruned = Vec::new();
    for candidate in candidates {
        let idle_hours = parse_ts(&candidate.last_seen)
            .map(|at| (now - at).num_hours().max(0))
            .unwrap_or(0);
        let ephemeral_removal = candidate.session_id.is_none() && ephemeral_prune_after_hours > 0;
        let reason = if ephemeral_removal {
            format!(
                "prune: ephemeral sessionless removal; no claims, chat or reviews; idle {idle_hours}h"
            )
        } else {
            let session_note = match (&candidate.session_id, opencode_sessions) {
                (None, _) => "sessionless",
                (Some(id), Some(sessions)) if !sessions.contains(id) => {
                    "session missing in OpenCode"
                }
                (Some(_), Some(_)) => "session present in OpenCode",
                (Some(_), None) => "OpenCode session store unavailable",
            };
            format!("prune: no claims, chat or reviews; idle {idle_hours}h; {session_note}")
        };

        // Re-check the protections in the write transaction. A claim, DM or
        // review that landed after the select makes this delete a no-op.
        let mut tx = crate::db::begin_write(db).await?;
        let deleted = sqlx::query(
            "DELETE FROM agent_sessions
              WHERE agent_name = ?1
                AND paused = 0
                AND julianday(last_seen) < CASE
                      WHEN session_id IS NULL THEN julianday(?2)
                      ELSE julianday(?3)
                    END
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
        .bind(&ephemeral_cutoff)
        .bind(&prune_cutoff)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if deleted == 0 {
            tx.rollback().await?;
            continue;
        }

        // A pruned name is documented as free for reuse. Detach its children
        // before deleting the row, for both the explicit name edge and the raw
        // session edge, so a fresh agent that takes the name inherits nobody.
        sqlx::query(
            "UPDATE agent_sessions
                SET parent_agent_name = NULL
              WHERE lower(parent_agent_name) = ?1",
        )
        .bind(&candidate.agent_name)
        .execute(&mut *tx)
        .await?;
        if let Some(session) = candidate.session_id.as_deref() {
            sqlx::query(
                "UPDATE agent_sessions
                    SET parent_session_id = NULL
                  WHERE parent_session_id = ?1",
            )
            .bind(session)
            .execute(&mut *tx)
            .await?;
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

/// The periodic maintenance interval, or `None` when the periodic pass is
/// disabled by `agents.maintenance_interval_mins = 0`. The startup pass ignores
/// this and always runs.
pub fn maintenance_interval(mins: u64) -> Option<StdDuration> {
    if mins == 0 {
        None
    } else {
        Some(StdDuration::from_secs(mins.saturating_mul(60)))
    }
}

/// Run once at startup and then every `agents.maintenance_interval_mins`.
/// Cheap: one indexed select and no writes when nothing qualifies. Setting the
/// interval to `0` disables only the periodic runs; the startup pass still
/// brings a restarted server back to a clean roster.
pub fn spawn_pruner(state: AppState) {
    tokio::spawn(async move {
        run_maintenance(&state, "startup").await;
        let Some(period) = maintenance_interval(state.config.agents.maintenance_interval_mins)
        else {
            tracing::info!("periodic agent roster maintenance disabled; startup pass already ran");
            return;
        };
        let mut tick = tokio::time::interval(period);
        tick.tick().await; // the first tick is immediate; startup already ran
        loop {
            tick.tick().await;
            run_maintenance(&state, "periodic").await;
        }
    });
}

/// One maintenance pass: reap stale claims, then prune the agents they were
/// pinning. Reaping first is what lets a dead agent disappear in the same run
/// instead of waiting for the next pass.
pub async fn run_maintenance(state: &AppState, reason: &str) {
    let reap_window = state.config.agents.claim_reap_after_hours;
    if reap_window == 0 {
        tracing::info!(reason, "stale claim reaping disabled");
    } else {
        match crate::claim_reap::reap_stale_claims(
            &state.db,
            reap_window,
            crate::store::AGENT_LIVE_SECS,
        )
        .await
        {
            Ok(reaped) if reaped.is_empty() => {}
            Ok(reaped) => {
                tracing::info!(
                    count = reaped.len(),
                    window_hours = reap_window,
                    reason,
                    "reaped stale agent claims"
                );
            }
            Err(error) => {
                tracing::warn!(%error, reason, "reaping stale agent claims failed");
            }
        }
    }
    prune_once(state, reason).await;
}

async fn prune_once(state: &AppState, reason: &str) {
    let prune_window = state.config.agents.prune_after_hours;
    let ephemeral_window = state.config.agents.ephemeral_prune_after_hours;
    if prune_window == 0 && ephemeral_window == 0 {
        tracing::info!(reason, "abandoned agent pruning disabled");
        return;
    }
    let sessions = opencode_session_ids(state).await;
    match prune_abandoned_agents(&state.db, prune_window, ephemeral_window, sessions.as_ref()).await
    {
        Ok(pruned) if pruned.is_empty() => {}
        Ok(pruned) => {
            tracing::info!(
                count = pruned.len(),
                window_hours = prune_window,
                ephemeral_window_hours = ephemeral_window,
                reason,
                "pruned abandoned agents"
            );
        }
        Err(error) => {
            tracing::warn!(%error, reason, "pruning abandoned agents failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;

    fn idle(now: DateTime<Utc>, hours: i64, minutes: i64) -> Option<DateTime<Utc>> {
        Some(now - TimeDelta::hours(hours) - TimeDelta::minutes(minutes))
    }

    #[test]
    fn ephemeral_boundary_is_two_hours_exactly() {
        let now = Utc::now();
        // Exactly 2h qualifies; one second inside does not.
        assert!(is_abandoned(
            0,
            false,
            false,
            false,
            true,
            idle(now, 2, 0),
            24,
            2,
            now
        ));
        assert!(!is_abandoned(
            0,
            false,
            false,
            false,
            true,
            Some(now - TimeDelta::hours(2) + TimeDelta::seconds(1)),
            24,
            2,
            now
        ));
        assert!(!is_abandoned(
            0,
            false,
            false,
            false,
            true,
            idle(now, 1, 0),
            24,
            2,
            now
        ));
    }

    #[test]
    fn session_bound_rows_use_the_long_window() {
        let now = Utc::now();
        assert!(!is_abandoned(
            0,
            false,
            false,
            false,
            false,
            idle(now, 2, 0),
            24,
            2,
            now
        ));
        assert!(is_abandoned(
            0,
            false,
            false,
            false,
            false,
            idle(now, 24, 0),
            24,
            2,
            now
        ));
    }

    #[test]
    fn protections_win_at_any_idle_time() {
        let now = Utc::now();
        let very_old = idle(now, 1000, 0);
        for (claim, paused, chat, review) in [
            (1, false, false, false),
            (0, true, false, false),
            (0, false, true, false),
            (0, false, false, true),
        ] {
            assert!(!is_abandoned(
                claim, paused, chat, review, true, very_old, 24, 2, now
            ));
        }
    }

    #[test]
    fn disabled_windows_never_remove_their_category() {
        let now = Utc::now();
        // ephemeral 0 disables removal of sessionless rows at any age.
        assert!(!is_abandoned(
            0,
            false,
            false,
            false,
            true,
            idle(now, 5, 0),
            24,
            0,
            now
        ));
        assert!(!is_abandoned(
            0,
            false,
            false,
            false,
            true,
            idle(now, 25, 0),
            24,
            0,
            now
        ));
        // prune 0 means a session-bound row is never removed.
        assert!(!is_abandoned(
            0,
            false,
            false,
            false,
            false,
            very_old(),
            0,
            2,
            now
        ));
        // both 0 removes nothing.
        assert!(!is_abandoned(
            0,
            false,
            false,
            false,
            true,
            very_old(),
            0,
            0,
            now
        ));
    }

    fn very_old() -> Option<DateTime<Utc>> {
        idle(Utc::now(), 1000, 0)
    }
}
