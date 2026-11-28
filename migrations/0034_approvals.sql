-- Approval asks from agents to the operator, plus maintenance windows and
-- change freezes. One ask row is the unit the operator sees; its conversation
-- is `approval_events`, its screenshots are links to existing issue artifacts,
-- and a maintenance ask owns at most one `maintenance_windows` row.
--
-- Product rules live here and in the server, not the UI:
--   * an agent opens, revises, comments on and withdraws its own ask; only a
--     human operator records a decision,
--   * a decision is one-shot: pending is the only status that can be decided,
--   * `changes` is not an outcome: the owning agent resubmits the same row,
--     which increments `revision` and returns it to `pending`,
--   * `answered` (send info / instructions) closes the ask,
--   * a pending ask past `expires_at` becomes `expired` and can no longer be
--     decided.

CREATE TABLE approval_asks (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id       INTEGER REFERENCES projects(id) ON DELETE SET NULL,
    issue_id         INTEGER REFERENCES issues(id) ON DELETE SET NULL,
    -- The owning agent identity. Spaces and hyphens are equivalent.
    agent_name       TEXT    NOT NULL,
    session_id       TEXT,
    kind             TEXT    NOT NULL DEFAULT 'approval'
                     CHECK (kind IN ('approval', 'info', 'maintenance')),
    urgency          TEXT    NOT NULL DEFAULT 'normal'
                     CHECK (urgency IN ('low', 'normal', 'high')),
    title            TEXT    NOT NULL,
    summary          TEXT    NOT NULL DEFAULT '',
    body_md          TEXT    NOT NULL DEFAULT '',
    environment      TEXT,
    status           TEXT    NOT NULL DEFAULT 'pending'
                     CHECK (status IN ('pending', 'approved', 'declined', 'changes',
                                       'answered', 'withdrawn', 'expired')),
    -- Incremented by each resubmission after a changes request.
    revision         INTEGER NOT NULL DEFAULT 1,
    -- "What the agent needs from you" checklist, JSON array of strings.
    checklist_json   TEXT    NOT NULL DEFAULT '[]',
    risk_md          TEXT,
    rollback_md      TEXT,
    -- Optional one-shot expiry. A pending ask past this instant is expired.
    expires_at       TEXT,
    -- Human display deadline ("today 18:00", "before the freeze").
    deadline         TEXT,
    decided_by       INTEGER REFERENCES users(id) ON DELETE SET NULL,
    decided_by_login TEXT,
    decided_at       TEXT,
    decision_note_md TEXT,
    created_at       TEXT    NOT NULL,
    updated_at       TEXT    NOT NULL
);

CREATE INDEX idx_approval_asks_status ON approval_asks(status, created_at);
CREATE INDEX idx_approval_asks_agent ON approval_asks(replace(lower(agent_name), ' ', '-'), created_at);
CREATE INDEX idx_approval_asks_project ON approval_asks(project_id, created_at);
CREATE INDEX idx_approval_asks_expiry ON approval_asks(status, expires_at);

CREATE TABLE approval_events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ask_id     INTEGER NOT NULL REFERENCES approval_asks(id) ON DELETE CASCADE,
    kind       TEXT    NOT NULL
               CHECK (kind IN ('created', 'revised', 'comment', 'approved', 'declined',
                               'changes', 'info', 'withdrawn', 'expired')),
    actor_kind TEXT    NOT NULL DEFAULT 'agent'
               CHECK (actor_kind IN ('agent', 'human', 'system')),
    actor_name TEXT    NOT NULL,
    body_md    TEXT    NOT NULL DEFAULT '',
    created_at TEXT    NOT NULL,
    -- Free-form context: revision number, decision window, etc.
    meta_json  TEXT    NOT NULL DEFAULT '{}'
);

CREATE INDEX idx_approval_events_ask ON approval_events(ask_id, id);

-- Screenshots reuse issue_artifacts: same upload, retention and auth. The link
-- only records order and the caption shown under the image.
CREATE TABLE approval_attachments (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ask_id      INTEGER NOT NULL REFERENCES approval_asks(id) ON DELETE CASCADE,
    artifact_id INTEGER NOT NULL REFERENCES issue_artifacts(id) ON DELETE CASCADE,
    caption     TEXT    NOT NULL DEFAULT '',
    sort        INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT    NOT NULL,
    UNIQUE (ask_id, artifact_id)
);

CREATE INDEX idx_approval_attachments_ask ON approval_attachments(ask_id, sort, id);

CREATE TABLE maintenance_windows (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ask_id      INTEGER REFERENCES approval_asks(id) ON DELETE CASCADE,
    project_id  INTEGER REFERENCES projects(id) ON DELETE SET NULL,
    agent_name  TEXT    NOT NULL,
    title       TEXT    NOT NULL,
    starts_at   TEXT    NOT NULL,
    ends_at     TEXT    NOT NULL,
    timezone    TEXT    NOT NULL DEFAULT 'UTC',
    services_json TEXT  NOT NULL DEFAULT '[]',
    reason      TEXT    NOT NULL DEFAULT '',
    risks       TEXT    NOT NULL DEFAULT '',
    rollback    TEXT    NOT NULL DEFAULT '',
    runbook_md  TEXT    NOT NULL DEFAULT '',
    status      TEXT    NOT NULL DEFAULT 'proposed'
                CHECK (status IN ('proposed', 'approved', 'in_progress',
                                  'completed', 'cancelled')),
    decided_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    decided_at  TEXT,
    created_at  TEXT    NOT NULL,
    updated_at  TEXT    NOT NULL
);

CREATE INDEX idx_maintenance_windows_range ON maintenance_windows(starts_at, ends_at);
CREATE INDEX idx_maintenance_windows_ask ON maintenance_windows(ask_id);
CREATE UNIQUE INDEX idx_maintenance_windows_ask_unique ON maintenance_windows(ask_id)
    WHERE ask_id IS NOT NULL;

-- Change freezes and blackout periods. Conflicts with a window are shown in
-- the calendar but never block a proposal or a decision.
CREATE TABLE maintenance_freezes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER REFERENCES projects(id) ON DELETE SET NULL,
    label      TEXT    NOT NULL,
    starts_at  TEXT    NOT NULL,
    ends_at    TEXT    NOT NULL,
    created_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at TEXT    NOT NULL
);

CREATE INDEX idx_maintenance_freezes_range ON maintenance_freezes(starts_at, ends_at);

-- Per-operator read cursor. An ask is unread while its newest event is newer
-- than this operator's cursor (or the operator has never opened it).
CREATE TABLE approval_reads (
    ask_id             INTEGER NOT NULL REFERENCES approval_asks(id) ON DELETE CASCADE,
    user_id            INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    last_seen_event_id INTEGER NOT NULL DEFAULT 0,
    updated_at         TEXT    NOT NULL,
    PRIMARY KEY (ask_id, user_id)
);
