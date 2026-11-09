-- Per-agent activity timeline: typed events written at the point an action
-- already happens (claim, release, state change, PR update, merge, chat), not
-- derived later by polling. Each row is one feed entry for the Agents modal.
--
-- `issue_id` / `room_id` / `message_id` are nullable links with ON DELETE SET
-- NULL so the timeline survives the deletion of the object it points at;
-- `issue_key` and the summary keep the entry readable when that happens.
-- `agent_name` is the lowercased name at the time of the event, matching
-- agent_claims and agent_sessions. Renaming an agent does not rewrite history.
--
-- Retention: events are pruned after 90 days (AGENT_EVENT_RETENTION_DAYS in
-- agent_events.rs), which comfortably answers "what did this agent do
-- yesterday" without keeping a permanent chat/claim archive.

CREATE TABLE agent_events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_name TEXT    NOT NULL,
    kind       TEXT    NOT NULL,
    issue_id   INTEGER REFERENCES issues(id) ON DELETE SET NULL,
    issue_key  TEXT,
    pr_url     TEXT,
    pr_number  INTEGER,
    room_id    INTEGER REFERENCES chat_rooms(id) ON DELETE SET NULL,
    message_id INTEGER REFERENCES chat_messages(id) ON DELETE SET NULL,
    summary    TEXT,
    detail     TEXT,
    actor_id   INTEGER REFERENCES users(id) ON DELETE SET NULL,
    at         TEXT    NOT NULL
);

CREATE INDEX idx_agent_events_agent_at ON agent_events(agent_name, at DESC, id DESC);
CREATE INDEX idx_agent_events_kind ON agent_events(kind, at DESC);
CREATE INDEX idx_agent_events_at ON agent_events(at);
