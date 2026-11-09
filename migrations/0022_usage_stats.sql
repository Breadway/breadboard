-- Token and cost usage, imported from OpenCode's local session store.
--
-- OpenCode already persists one row per session in its `session_v2` table and
-- per-assistant-message token counts in `session_message.data`. This table is
-- breadboard's own roll-up so stats survive OpenCode log retention and can be
-- queried per agent and per issue without reading a second database at request
-- time.
--
-- One row is one (session, model, UTC day) bucket. Re-importing a session
-- replaces its rows, so the table always reflects the latest values OpenCode
-- reported and a growing session does not double count.
--
-- `agent_name` is resolved from `agent_sessions` / `agent_claims` by session
-- id at import time. A session with no registered or claimed mapping is
-- skipped entirely, so this table only holds usage the board can attribute.
-- The column stays nullable so a future source can record unattributed usage
-- explicitly.

CREATE TABLE usage_sessions (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id         TEXT    NOT NULL,
    agent_name         TEXT,
    provider           TEXT,
    model              TEXT    NOT NULL,
    day                TEXT    NOT NULL,
    input_tokens       INTEGER NOT NULL DEFAULT 0,
    output_tokens      INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens   INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    cost               REAL    NOT NULL DEFAULT 0,
    source             TEXT    NOT NULL DEFAULT 'opencode',
    imported_at        TEXT    NOT NULL,
    UNIQUE(session_id, model, day, source)
);

CREATE INDEX idx_usage_sessions_session ON usage_sessions(session_id);
CREATE INDEX idx_usage_sessions_agent_day ON usage_sessions(agent_name, day);
CREATE INDEX idx_usage_sessions_day ON usage_sessions(day);

-- Which issues a session touched. Recorded when the claim is created and
-- backfilled from live claims on each import, so an issue's total survives the
-- claim being released. A session that worked several issues counts in each.
CREATE TABLE usage_issue_links (
    session_id TEXT    NOT NULL,
    issue_id   INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    first_seen TEXT    NOT NULL,
    last_seen  TEXT    NOT NULL,
    PRIMARY KEY (session_id, issue_id)
);

CREATE INDEX idx_usage_issue_links_issue ON usage_issue_links(issue_id);

-- Singleton settings row. The spend warning is global: the Agents page and the
-- issue drawer read the same threshold.
CREATE TABLE usage_settings (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    spend_warn_usd REAL    NOT NULL DEFAULT 5.0,
    updated_at     TEXT    NOT NULL
);
