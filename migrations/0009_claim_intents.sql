-- Claims gain an id and a mode so one exclusive claim and any number of soft
-- intents can coexist per issue (the old table keyed on issue_id alone).
--
-- exclusive: at most one per issue (partial unique index) and the only mode
--            that moves the card to doing / assigns it.
-- intent:    a reservation that shows interest in queued work, never blocks
--            another agent, and is unique per (issue, user, agent).

CREATE TABLE agent_claims_new (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id       INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    claimed_by     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    agent_name     TEXT    NOT NULL,
    mode           TEXT    NOT NULL DEFAULT 'exclusive',
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

INSERT INTO agent_claims_new
    (issue_id, claimed_by, agent_name, mode, session_id, state, note, branch,
     worktree, repo, pr_url, pr_number, head_sha, board_status, claimed_at,
     updated_at, last_heartbeat)
SELECT
    issue_id, claimed_by, agent_name, 'exclusive', session_id, state, note,
    branch, worktree, repo, pr_url, pr_number, head_sha, board_status,
    claimed_at, updated_at, last_heartbeat
FROM agent_claims;

DROP TABLE agent_claims;
ALTER TABLE agent_claims_new RENAME TO agent_claims;

CREATE UNIQUE INDEX idx_agent_claim_exclusive
    ON agent_claims(issue_id) WHERE mode = 'exclusive';
CREATE UNIQUE INDEX idx_agent_claim_intent
    ON agent_claims(issue_id, claimed_by, agent_name) WHERE mode = 'intent';
CREATE INDEX idx_agent_claims_heartbeat ON agent_claims(last_heartbeat);
CREATE INDEX idx_agent_claims_session ON agent_claims(session_id);
CREATE INDEX idx_agent_claims_issue ON agent_claims(issue_id);
