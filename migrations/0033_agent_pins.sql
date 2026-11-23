-- Pin an agent row so roster maintenance never removes it automatically.
--
-- Pause is a lifecycle action: it keeps claims but refuses new work. Pin is
-- narrower: it only excludes the row from the automatic prune (the normal
-- abandoned pass and the ephemeral sessionless pass). Claims, chat, heartbeats
-- and renaming keep working, so a pinned agent is still expected to age out
-- of work and can be unpinned to let the row be removed later.
--
-- Registration upserts only its listed columns, so these columns survive a
-- re-registration, a heartbeat and a session-id change. Rename updates the
-- primary key name on the same row, so the pin follows the identity.
--
-- Kill is explicit and is not a maintenance pass: it still removes a pinned
-- row. Unpinning is the only way a pin lets a row age out.

ALTER TABLE agent_sessions ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
ALTER TABLE agent_sessions ADD COLUMN pinned_by INTEGER REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE agent_sessions ADD COLUMN pinned_at TEXT;

CREATE INDEX idx_agent_sessions_pinned ON agent_sessions(pinned);
