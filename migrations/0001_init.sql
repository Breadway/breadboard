-- breadboard initial schema.
--
-- SQLite with WAL. Timestamps are RFC3339 text (UTC). Enums are stored as the
-- snake_case strings that `breadboard-core`'s serde derives produce.

CREATE TABLE users (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    forgejo_id   INTEGER NOT NULL UNIQUE,
    login        TEXT    NOT NULL,
    display_name TEXT    NOT NULL,
    avatar_url   TEXT
);

CREATE TABLE projects (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    key          TEXT    NOT NULL UNIQUE,          -- e.g. BREAD, BAR, ROAD
    name         TEXT    NOT NULL,
    forgejo_repo TEXT    UNIQUE,                   -- owner/repo, NULL = native project
    next_number  INTEGER NOT NULL DEFAULT 1,       -- per-key issue counter
    archived     INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE epics (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    key         TEXT    NOT NULL UNIQUE,           -- e.g. ROAD-4
    title       TEXT    NOT NULL,
    description TEXT,
    project_id  INTEGER REFERENCES projects(id) ON DELETE SET NULL,
    archived    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT    NOT NULL,
    updated_at  TEXT    NOT NULL
);

CREATE TABLE cycles (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    name      TEXT    NOT NULL,
    starts_on TEXT    NOT NULL,                    -- date
    ends_on   TEXT    NOT NULL,                    -- date
    closed    INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE issues (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    key            TEXT    NOT NULL UNIQUE,        -- e.g. BREAD-42
    project_id     INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    title          TEXT    NOT NULL,
    body           TEXT    NOT NULL DEFAULT '',
    status         TEXT    NOT NULL DEFAULT 'backlog',
    priority       TEXT    NOT NULL DEFAULT 'none',
    type           TEXT    NOT NULL DEFAULT 'issue',   -- issue | pr
    estimate       INTEGER,
    assignee_id    INTEGER REFERENCES users(id) ON DELETE SET NULL,
    reporter_id    INTEGER REFERENCES users(id) ON DELETE SET NULL,
    epic_id        INTEGER REFERENCES epics(id) ON DELETE SET NULL,
    cycle_id       INTEGER REFERENCES cycles(id) ON DELETE SET NULL,
    -- Forgejo linkage. NULL repo => breadboard-native issue.
    forgejo_repo   TEXT,
    forgejo_number INTEGER,
    created_at     TEXT    NOT NULL,
    updated_at     TEXT    NOT NULL,
    UNIQUE (forgejo_repo, forgejo_number)
);

CREATE INDEX idx_issues_project ON issues(project_id);
CREATE INDEX idx_issues_status  ON issues(status);
CREATE INDEX idx_issues_cycle   ON issues(cycle_id);
CREATE INDEX idx_issues_epic    ON issues(epic_id);

CREATE TABLE issue_labels (
    issue_id INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    label    TEXT    NOT NULL,
    PRIMARY KEY (issue_id, label)
);

CREATE TABLE issue_links (
    issue_id  INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    target_id INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    kind      TEXT    NOT NULL,                    -- blocks | blocked-by | relates-to
    PRIMARY KEY (issue_id, target_id, kind)
);

CREATE TABLE comments (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id    INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    author_id   INTEGER REFERENCES users(id) ON DELETE SET NULL,
    body        TEXT    NOT NULL,
    external_id INTEGER,                           -- Forgejo comment id when synced
    created_at  TEXT    NOT NULL,
    updated_at  TEXT    NOT NULL,
    UNIQUE (issue_id, external_id)
);

-- Immutable audit trail: one row per field-level change from either side.
CREATE TABLE sync_log (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id  INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    field     TEXT    NOT NULL,
    old_value TEXT,
    new_value TEXT,
    source    TEXT    NOT NULL,                    -- breadboard | forgejo_webhook | forgejo_reconcile
    actor_id  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    at        TEXT    NOT NULL
);

CREATE INDEX idx_sync_log_issue ON sync_log(issue_id, at);

-- Per-repo sync bookkeeping for the reconcile pass.
CREATE TABLE repo_sync_state (
    forgejo_repo   TEXT PRIMARY KEY,
    last_synced_at TEXT,
    enabled        INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE notifications (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind       TEXT    NOT NULL,                   -- assigned | mention | cycle_started | cycle_ended
    issue_id   INTEGER REFERENCES issues(id) ON DELETE CASCADE,
    body       TEXT    NOT NULL,
    read       INTEGER NOT NULL DEFAULT 0,
    created_at TEXT    NOT NULL
);

CREATE INDEX idx_notifications_user ON notifications(user_id, read);

-- CLI/TUI tokens (PAT-style). The web client uses a session cookie instead.
CREATE TABLE api_tokens (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    token_hash TEXT    NOT NULL UNIQUE,
    created_at TEXT    NOT NULL,
    last_used  TEXT
);
