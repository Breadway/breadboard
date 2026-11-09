-- Hidden agents: a per-name display flag for the Agents page.
--
-- Hiding is not deleting: claims, chat identity and history stay untouched,
-- and any device sees the same flag. The roster still returns hidden rows so
-- the page can count them and reveal them on demand.

CREATE TABLE agent_hidden (
    agent_name TEXT PRIMARY KEY,
    hidden_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    hidden_at  TEXT    NOT NULL
);

CREATE INDEX idx_agent_hidden_at ON agent_hidden(hidden_at);
