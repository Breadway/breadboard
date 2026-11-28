-- Phase 3 approvals operations: per-agent decision cursors, Gotify delivery
-- dedup and the approval notification opt-out.

-- An agent's own read cursor for decisions on its asks. The operator cursor
-- stays in approval_reads; this table is keyed by the agent identity and only
-- advances when the agent reads the ask through `?agent=`.
CREATE TABLE approval_agent_reads (
    ask_id             INTEGER NOT NULL REFERENCES approval_asks(id) ON DELETE CASCADE,
    agent_name         TEXT    NOT NULL,
    last_seen_event_id INTEGER NOT NULL DEFAULT 0,
    updated_at         TEXT    NOT NULL,
    PRIMARY KEY (ask_id, agent_name)
);

CREATE INDEX idx_approval_agent_reads_agent
    ON approval_agent_reads(replace(lower(agent_name), ' ', '-'));

-- One row per (ask, event) that was actually delivered to Gotify. Inserting
-- before the send makes a transition at-most-once even under a race; a failed
-- send is logged and not retried, so a broken Gotify cannot loop the server.
CREATE TABLE approval_notifications (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ask_id     INTEGER NOT NULL REFERENCES approval_asks(id) ON DELETE CASCADE,
    event_id   INTEGER NOT NULL REFERENCES approval_events(id) ON DELETE CASCADE,
    kind       TEXT    NOT NULL,
    created_at TEXT    NOT NULL,
    UNIQUE (ask_id, event_id)
);

-- Opt-out for Gotify approval pings, default on for existing users. The in-app
-- badge is unaffected: it reflects asks that still need a decision.
ALTER TABLE notification_prefs ADD COLUMN approvals INTEGER NOT NULL DEFAULT 1;
