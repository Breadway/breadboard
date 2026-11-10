-- One session holds exactly one agent name (BB-188), the mirror of 0028.
--
-- 0028 made a live name belong to one session. Nothing stopped the same
-- session from accumulating extra names across restarts, so a nameless
-- registration kept assigning a fresh curated name while the previous row
-- stayed registered and pollable. Registration now reuses a session's existing
-- name and refuses an explicit different name with a 409 that points at
-- rename. This migration resolves the rows that were already created.
--
-- For every session with more than one name, the kept name is the one that
-- holds claims; otherwise the most recently active across its session row and
-- claim heartbeats; the name itself is the deterministic tie-break. The other
-- session rows are deleted only when that name holds no claims, because a
-- claim must never be left without its owning session. Every resolved extra is
-- recorded in `agent_session_conflicts` with whether it was dropped, so the
-- cleanup is auditable instead of silent. Profiles, hidden flags and chat
-- state are deliberately untouched: they are user data, and a name without a
-- session row is no longer polled.

-- The reuse lookup runs on every registration and auto-registration.
CREATE INDEX idx_agent_sessions_session ON agent_sessions(session_id);

-- Audit only. One row per extra name that a multi-name session used to hold.
CREATE TABLE agent_session_conflicts (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id        TEXT    NOT NULL,
    dropped_name      TEXT    NOT NULL,
    kept_name         TEXT    NOT NULL,
    claims_on_dropped INTEGER NOT NULL DEFAULT 0,
    dropped           INTEGER NOT NULL DEFAULT 0,
    recorded_at       TEXT    NOT NULL
);

CREATE INDEX idx_agent_session_conflicts_session
    ON agent_session_conflicts(session_id);
CREATE INDEX idx_agent_session_conflicts_at
    ON agent_session_conflicts(recorded_at);

-- Every name row with the activity that decides its rank. Claim count is by
-- name because a name is held by one session after 0028; claim heartbeats are
-- part of "most recently active" just as they are in 0028.
CREATE TEMP TABLE agent_session_name_candidates AS
SELECT
    session_id,
    name,
    claim_count,
    CASE
        WHEN julianday(claim_at) > julianday(last_seen) THEN claim_at
        ELSE last_seen
    END AS active_at
FROM (
    SELECT
        s.session_id,
        s.agent_name AS name,
        s.last_seen,
        (SELECT COUNT(*)
           FROM agent_claims c
          WHERE lower(c.agent_name) = lower(s.agent_name)) AS claim_count,
        (SELECT MAX(c.last_heartbeat)
           FROM agent_claims c
          WHERE lower(c.agent_name) = lower(s.agent_name)) AS claim_at
      FROM agent_sessions s
     WHERE s.session_id IS NOT NULL
);

CREATE TEMP TABLE agent_session_name_ranked AS
SELECT
    session_id,
    name,
    claim_count,
    ROW_NUMBER() OVER (
        PARTITION BY session_id
        ORDER BY
            CASE WHEN claim_count > 0 THEN 1 ELSE 0 END DESC,
            julianday(active_at) DESC,
            name ASC
    ) AS rank
FROM agent_session_name_candidates;

INSERT INTO agent_session_conflicts
    (session_id, dropped_name, kept_name, claims_on_dropped, dropped, recorded_at)
SELECT
    extra.session_id,
    extra.name,
    kept.name,
    extra.claim_count,
    CASE WHEN extra.claim_count = 0 THEN 1 ELSE 0 END,
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_session_name_ranked extra
JOIN agent_session_name_ranked kept
  ON kept.session_id = extra.session_id
 AND kept.rank = 1
WHERE extra.rank > 1;

-- Only claimless extras can be removed without orphaning work.
DELETE FROM agent_sessions
 WHERE agent_name IN (
    SELECT extra.name
      FROM agent_session_name_ranked extra
     WHERE extra.rank > 1
       AND extra.claim_count = 0
 );

DROP TABLE agent_session_name_ranked;
DROP TABLE agent_session_name_candidates;
