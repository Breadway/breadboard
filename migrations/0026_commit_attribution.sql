-- Per-commit provenance for agent-produced commits.
--
-- A commit is keyed by its repository and full SHA. The row records which
-- agent and session produced it, the orchestrator resolved from that agent's
-- parent session, and the model that was actually reported for this exact
-- session. The model is never inferred from whichever agent was last active:
-- it is either supplied by the agent making the commit, or read from
-- `usage_sessions` rows that match this exact session id and agree on a single
-- model. Otherwise it stays NULL with `model_source = 'unknown'`.
--
-- Rewrites: `patch_id` (`git patch-id --stable`) survives a rebase, and
-- `tree_sha` identifies the tree a squash of the branch tip produced. Both are
-- optional and supplied by the caller that can compute them. A lookup for a
-- rewritten SHA can therefore report which original attribution it matched,
-- or report that the commit has no known attribution at all.
--
-- `issue_key` is a snapshot for display. The foreign key is what makes an
-- issue's commit list survive a key change; `ON DELETE SET NULL` keeps the
-- commit record if its issue is removed.

CREATE TABLE commit_attribution (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    repo                    TEXT NOT NULL COLLATE NOCASE,
    sha                     TEXT NOT NULL,
    issue_id                INTEGER REFERENCES issues(id) ON DELETE SET NULL,
    issue_key               TEXT,
    agent_name              TEXT NOT NULL,
    session_id              TEXT,
    orchestrator            TEXT,
    orchestrator_session_id TEXT,
    model                   TEXT,
    provider                TEXT,
    -- 'reported' (agent supplied the model), 'session' (one unambiguous model
    -- in usage_sessions for this exact session), or 'unknown'.
    model_source            TEXT NOT NULL DEFAULT 'unknown'
                            CHECK (model_source IN ('reported', 'session', 'unknown')),
    patch_id                TEXT,
    tree_sha                TEXT,
    note                    TEXT,
    recorded_by             INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    UNIQUE(repo, sha)
);

CREATE INDEX idx_commit_attribution_issue ON commit_attribution(issue_id, created_at);
CREATE INDEX idx_commit_attribution_patch ON commit_attribution(repo, patch_id);
CREATE INDEX idx_commit_attribution_tree ON commit_attribution(repo, tree_sha);
CREATE INDEX idx_commit_attribution_session ON commit_attribution(session_id);
