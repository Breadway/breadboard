-- Agent roster: every session that touches breadboard, plus name-level kill.
--
-- Identity is the lowercased agent name, matching agent_claims and
-- agent_profiles. A session registration upserts one row per name and refreshes
-- `last_seen`; the Agents page is the union of this table, agent_profiles and
-- agent_claims. A name with no heartbeat for 120 seconds is stale; a killed
-- name is neither live nor writable until explicitly revived.
--
-- `killed_by` records who asked for the kill. Registration never clears it:
-- reviving is an explicit operator action.

CREATE TABLE agent_sessions (
    agent_name TEXT PRIMARY KEY,
    session_id TEXT,
    host       TEXT,
    pid        INTEGER,
    note       TEXT,
    user_id    INTEGER REFERENCES users(id) ON DELETE SET NULL,
    first_seen TEXT    NOT NULL,
    last_seen  TEXT    NOT NULL,
    killed     INTEGER NOT NULL DEFAULT 0,
    killed_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    killed_at  TEXT
);

CREATE INDEX idx_agent_sessions_last_seen ON agent_sessions(last_seen);
CREATE INDEX idx_agent_sessions_killed ON agent_sessions(killed);

-- Backfill from existing claims so agents that already passed a session_id
-- show up with it immediately. Profile-only names still appear through the
-- roster union; their session id arrives with their next registration.
INSERT OR IGNORE INTO agent_sessions
    (agent_name, session_id, user_id, first_seen, last_seen, killed, killed_by, killed_at)
SELECT
    lower(c.agent_name),
    c.session_id,
    c.claimed_by,
    c.claimed_at,
    c.last_heartbeat,
    0,
    NULL,
    NULL
FROM agent_claims c
JOIN (
    SELECT lower(agent_name) AS name, MAX(last_heartbeat) AS heartbeat
    FROM agent_claims
    GROUP BY lower(agent_name)
) latest
  ON lower(c.agent_name) = latest.name
 AND c.last_heartbeat = latest.heartbeat
GROUP BY lower(c.agent_name);
