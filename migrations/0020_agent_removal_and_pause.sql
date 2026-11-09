-- Kill becomes removal, and pause becomes a separate lifecycle action.
--
-- A killed name is no longer a tombstone: kill deletes the session row, the
-- profile, the hidden flag, every claim and the agent's chat notification
-- state, then records one server-side audit row. The audit table is not part
-- of the roster or any web view. A name removed this way is immediately free
-- for a fresh registration.
--
-- Pause is the non-destructive alternative: it keeps the session row, profile
-- and claims (so work is not released), while rejecting new claims and
-- suppressing chat pushes so an injector cannot wake the agent.

ALTER TABLE agent_sessions ADD COLUMN paused INTEGER NOT NULL DEFAULT 0;
ALTER TABLE agent_sessions ADD COLUMN paused_by INTEGER REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE agent_sessions ADD COLUMN paused_at TEXT;

CREATE INDEX idx_agent_sessions_paused ON agent_sessions(paused);

-- Server-side audit only. Deliberately not joined into agent_roster.
CREATE TABLE agent_removals (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_name      TEXT    NOT NULL,
    removed_by      INTEGER REFERENCES users(id) ON DELETE SET NULL,
    removed_at      TEXT    NOT NULL,
    reason          TEXT    NOT NULL DEFAULT 'kill',
    claims_released INTEGER NOT NULL DEFAULT 0,
    profile_removed INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_agent_removals_name ON agent_removals(agent_name);
CREATE INDEX idx_agent_removals_at ON agent_removals(removed_at);

-- Pre-existing killed rows from BB-143/BB-146 are removed, not kept. They were
-- tombstones that the human explicitly asked to stop rendering; keeping them
-- as hidden "killed" rows would keep the roster inconsistent with the new
-- model. Preserve one audit row per removed name first.
INSERT INTO agent_removals (agent_name, removed_by, removed_at, reason, profile_removed)
SELECT
    s.agent_name,
    s.killed_by,
    COALESCE(s.killed_at, s.last_seen),
    'migration: pre-existing killed row removed',
    EXISTS (SELECT 1 FROM agent_profiles p WHERE p.agent_name = s.agent_name)
FROM agent_sessions s
WHERE s.killed = 1;

DELETE FROM chat_pushes
 WHERE member IN ('agent:' || (SELECT agent_name FROM agent_sessions WHERE killed = 1));

DELETE FROM chat_push_subs
 WHERE member IN ('agent:' || (SELECT agent_name FROM agent_sessions WHERE killed = 1));

DELETE FROM chat_reads
 WHERE member IN ('agent:' || (SELECT agent_name FROM agent_sessions WHERE killed = 1));

DELETE FROM agent_hidden
 WHERE lower(agent_name) IN (SELECT agent_name FROM agent_sessions WHERE killed = 1);

DELETE FROM agent_claims
 WHERE lower(agent_name) IN (SELECT agent_name FROM agent_sessions WHERE killed = 1);

DELETE FROM agent_profiles
 WHERE lower(agent_name) IN (SELECT agent_name FROM agent_sessions WHERE killed = 1);

DELETE FROM agent_sessions WHERE killed = 1;
