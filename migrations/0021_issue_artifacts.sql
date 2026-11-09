-- Typed evidence attached to an issue: benches, screenshots, CI logs, diffs,
-- traces and anything else. Files live under the upload dir (the data volume
-- in the container) and `path` is the server-generated filename inside that
-- dir, never a user-supplied path.
--
-- Rows are immutable once written. The trigger below makes an accidental
-- UPDATE fail instead of silently rewriting evidence; expiry deletes rows and
-- files, which is intentional. `comment_id` optionally ties the artifact to a
-- comment; `review_ref` is a forward-compatible reference (key or URL) to a
-- review request record once that lands.

CREATE TABLE issue_artifacts (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id       INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    kind           TEXT    NOT NULL CHECK (kind IN ('bench', 'screenshot', 'ci-log', 'diff', 'trace', 'other')),
    comment_id     INTEGER REFERENCES comments(id) ON DELETE SET NULL,
    review_ref     TEXT,
    filename       TEXT    NOT NULL,
    mime           TEXT    NOT NULL,
    size           INTEGER NOT NULL,
    path           TEXT    NOT NULL,
    uploader_agent TEXT    NOT NULL,
    session_id     TEXT,
    created_at     TEXT    NOT NULL,
    expires_at     TEXT    NOT NULL
);

CREATE INDEX idx_issue_artifacts_issue ON issue_artifacts(issue_id, created_at);
CREATE INDEX idx_issue_artifacts_expiry ON issue_artifacts(expires_at);

CREATE TRIGGER issue_artifacts_immutable
BEFORE UPDATE ON issue_artifacts
BEGIN
    SELECT RAISE(ABORT, 'issue_artifacts rows are immutable');
END;
