-- Agent claims: exclusive, server-side presence for agentic workflows.
--
-- One row per claimed issue (issue_id is the PK: first claim wins, second
-- gets 409 until release). Heartbeats refresh `last_heartbeat`; the Agents
-- page treats a claim as stale after 5 minutes without one, but rows never
-- auto-expire: only an explicit release (or a new claim after release)
-- removes them. `state` mirrors the opencode session lifecycle
-- (planned/implementing/awaiting-review/reworking/awaiting-ci/
-- awaiting-merge/verifying/blocked/needs-user/done) so the server roll-up
-- reads like the bb-status widget.

CREATE TABLE agent_claims (
    issue_id       INTEGER PRIMARY KEY REFERENCES issues(id) ON DELETE CASCADE,
    claimed_by     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    agent_name     TEXT    NOT NULL,
    session_id     TEXT,
    state          TEXT    NOT NULL DEFAULT 'implementing',
    note           TEXT,
    branch         TEXT,
    worktree       TEXT,
    repo           TEXT,
    pr_url         TEXT,
    pr_number      INTEGER,
    head_sha       TEXT,
    board_status   TEXT    NOT NULL DEFAULT 'doing',
    claimed_at     TEXT    NOT NULL,
    updated_at     TEXT    NOT NULL,
    last_heartbeat TEXT    NOT NULL
);

CREATE INDEX idx_agent_claims_heartbeat ON agent_claims(last_heartbeat);
CREATE INDEX idx_agent_claims_session ON agent_claims(session_id);
