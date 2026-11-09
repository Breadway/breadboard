-- Review requests as first-class records. A request ties an issue (and the PR
-- under review) to one reviewer. Humans and agents use the same table; the
-- reviewer kind only decides which notification path is used.
--
-- `changes_requested` is an outcome but not a resolution: only `approved`
-- clears the outstanding flag that gates `done`.

CREATE TABLE review_requests (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id      INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    -- The PR under review. At least one of pr_url / pr_number is required.
    pr_url        TEXT,
    pr_number     INTEGER,
    pr_repo       TEXT,
    requested_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    -- For human reviewers this is their user id when they are known; for
    -- agent reviewers it is the user that owns the agent, when resolvable.
    reviewer_id   INTEGER REFERENCES users(id) ON DELETE SET NULL,
    reviewer      TEXT NOT NULL,
    reviewer_kind TEXT NOT NULL DEFAULT 'human'
                  CHECK (reviewer_kind IN ('human', 'agent')),
    state         TEXT NOT NULL DEFAULT 'requested'
                  CHECK (state IN ('requested', 'accepted', 'changes_requested', 'approved')),
    note          TEXT,
    requested_at  TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    responded_at  TEXT,
    -- Set once when the request has sat unresolved past the escalation
    -- threshold. Cleared on a state change so a re-sit can escalate again.
    escalated_at  TEXT
);

CREATE INDEX idx_review_requests_issue ON review_requests(issue_id);
CREATE INDEX idx_review_requests_reviewer ON review_requests(lower(reviewer), state);
CREATE INDEX idx_review_requests_outstanding ON review_requests(issue_id, state);

-- One active request per (issue, reviewer). Approved rows are history; a new
-- round of review can create a fresh request.
CREATE UNIQUE INDEX idx_review_requests_active
    ON review_requests(issue_id, lower(reviewer))
    WHERE state IN ('requested', 'accepted', 'changes_requested');
