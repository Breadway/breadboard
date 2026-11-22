//! Release claims whose heartbeat has gone cold, so dead agents stop pinning
//! the roster.
//!
//! A claim never expired on its own before BB-230: a crashed MCP process left
//! its claim behind, and the daily prune correctly refused to remove an agent
//! that still held one. The result was a roster of stale cards that could
//! never be cleaned up. This pass releases a claim only when every signal says
//! its owner is gone:
//!
//! - the claim's own `last_heartbeat` is older than
//!   `agents.claim_reap_after_hours` (default 72, `0` disables),
//! - no `agent_sessions` row for that name is paused by hand, and
//! - the claim's own `session_id` is null or does not match the live session
//!   id for that name.
//!
//! The live-session guard is claim-aware (BB-232). A live session protects
//! only the claims that carry its own `session_id`; it does not pin claims
//! left behind by older sessions that reused the same name. A claim whose
//! `session_id` matches a live session survives even when the claim row itself
//! was not refreshed, because the live session owns it. A sessionless claim
//! (the common dead-MCP case) has no heartbeat to protect it and is reaped on
//! the claim cutoff alone.
//!
//! Each release keeps the claim history rather than erasing it:
//!
//! - one `handoff_notes` row records the owner, mode, state and last heartbeat
//!   with the reaper's explanation, so the board card and a resuming session
//!   can see why the tint disappeared,
//! - one `agent_events` row (`released`) appears on the agent timeline,
//! - one `sync_log` row (`agent_reap`) is the issue-level audit entry naming
//!   the previous owner.
//!
//! The pass runs immediately before the existing roster prune, so the released
//! agent can be removed in the same maintenance run.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, SqlitePool};

use breadboard_core::ClaimMode;

use crate::error::AppResult;

/// One claim released by the reaper, for logging and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReapedClaim {
    pub agent_name: String,
    pub issue_id: i64,
    pub issue_key: String,
    pub mode: ClaimMode,
    pub state: String,
    pub idle_hours: i64,
}

#[derive(Debug, FromRow)]
struct Candidate {
    id: i64,
    issue_id: i64,
    issue_key: String,
    agent_name: String,
    mode: String,
    state: String,
    session_id: Option<String>,
    last_heartbeat: String,
}

fn parse_ts(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// Release every stale claim and return what was reaped. `live_secs` is the
/// same 120-second window the roster uses for "live"; a live session protects
/// a claim only when the claim's `session_id` is that session's id.
pub async fn reap_stale_claims(
    db: &SqlitePool,
    reap_after_hours: u64,
    live_secs: i64,
) -> AppResult<Vec<ReapedClaim>> {
    if reap_after_hours == 0 {
        return Ok(Vec::new());
    }
    let now = Utc::now();
    let cutoff = (now - chrono::Duration::hours(reap_after_hours as i64)).to_rfc3339();
    let live_cutoff = (now - chrono::Duration::seconds(live_secs.max(0))).to_rfc3339();

    let candidates: Vec<Candidate> = sqlx::query_as(
        "SELECT c.id, c.issue_id, i.key AS issue_key, c.agent_name, c.mode, c.state,
                c.session_id, c.last_heartbeat
           FROM agent_claims c
           JOIN issues i ON i.id = c.issue_id
          WHERE julianday(c.last_heartbeat) < julianday(?1)
            AND NOT EXISTS (
                SELECT 1 FROM agent_sessions s
                 WHERE lower(s.agent_name) = lower(c.agent_name)
                   AND s.paused = 1
            )
            AND NOT EXISTS (
                SELECT 1 FROM agent_sessions s
                 WHERE lower(s.agent_name) = lower(c.agent_name)
                   AND s.session_id IS NOT NULL
                   AND c.session_id IS NOT NULL
                   AND s.session_id = c.session_id
                   AND julianday(s.last_seen) >= julianday(?2)
            )
          ORDER BY c.last_heartbeat, c.id",
    )
    .bind(&cutoff)
    .bind(&live_cutoff)
    .fetch_all(db)
    .await?;

    let mut reaped = Vec::new();
    for candidate in candidates {
        let mode = ClaimMode::parse(&candidate.mode).unwrap_or(ClaimMode::Exclusive);
        let idle_hours = parse_ts(&candidate.last_heartbeat)
            .map(|at| (now - at).num_hours().max(0))
            .unwrap_or(0);
        let owner = format!("{} ({})", candidate.agent_name, mode.as_str());
        let reason = format!(
            "reaped: no heartbeat for {idle_hours}h (window {reap_after_hours}h); previous owner {owner}"
        );

        let mut tx = crate::db::begin_write(db).await?;
        // Re-check every exclusion in the write transaction. A heartbeat, a
        // pause, or a matching live session that landed after the select makes
        // this a no-op instead of a wrong release.
        let deleted = sqlx::query(
            "DELETE FROM agent_claims
              WHERE id = ?1
                AND julianday(last_heartbeat) < julianday(?2)
                AND NOT EXISTS (
                    SELECT 1 FROM agent_sessions s
                     WHERE lower(s.agent_name) = lower(agent_claims.agent_name)
                       AND s.paused = 1
                )
                AND NOT EXISTS (
                    SELECT 1 FROM agent_sessions s
                     WHERE lower(s.agent_name) = lower(agent_claims.agent_name)
                       AND s.session_id IS NOT NULL
                       AND agent_claims.session_id IS NOT NULL
                       AND s.session_id = agent_claims.session_id
                       AND julianday(s.last_seen) >= julianday(?3)
                )",
        )
        .bind(candidate.id)
        .bind(&cutoff)
        .bind(&live_cutoff)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if deleted == 0 {
            tx.rollback().await?;
            continue;
        }

        // Claim history survives the release: the handoff row holds the
        // previous owner, mode and state, and it is the same store a manual
        // release uses, so the board card and a resuming session see it.
        sqlx::query(
            "INSERT INTO handoff_notes
                (issue_id, session_id, agent_name, mode, state, note, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(candidate.issue_id)
        .bind(candidate.session_id.as_deref())
        .bind(&candidate.agent_name)
        .bind(mode.as_str())
        .bind(&candidate.state)
        .bind(&reason)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await?;
        if let Some(session) = candidate.session_id.as_deref() {
            // Preserve the issue/session link the same way a manual release
            // does, so the issue still lists the session that touched it.
            sqlx::query(
                "UPDATE issue_sessions SET last_seen = ?3, last_state = ?4
                  WHERE issue_id = ?1 AND session_id = ?2",
            )
            .bind(candidate.issue_id)
            .bind(session)
            .bind(now.to_rfc3339())
            .bind(&candidate.state)
            .execute(&mut *tx)
            .await?;
        }
        // Issue-level audit entry, naming the previous owner.
        crate::store::log_claim_event(
            &mut *tx,
            candidate.issue_id,
            "agent_reap",
            Some(&owner),
            Some(&reason),
            None,
        )
        .await?;
        tx.commit().await?;

        crate::agent_events::record_best_effort(
            db,
            crate::agent_events::AgentEventInsert::new(
                &candidate.agent_name,
                breadboard_core::AgentEventKind::Released,
            )
            .issue(candidate.issue_id, &candidate.issue_key)
            .summary(format!(
                "Reaped stale claim on {} after {idle_hours}h without heartbeat",
                candidate.issue_key
            ))
            .detail(format!(
                "Previous owner {owner}; last heartbeat {}; reaped by roster maintenance (window {reap_after_hours}h)",
                candidate.last_heartbeat
            )),
        )
        .await;

        reaped.push(ReapedClaim {
            agent_name: candidate.agent_name,
            issue_id: candidate.issue_id,
            issue_key: candidate.issue_key,
            mode,
            state: candidate.state,
            idle_hours,
        });
    }
    Ok(reaped)
}
