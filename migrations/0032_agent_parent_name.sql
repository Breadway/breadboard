-- Explicit parent edge by agent name.
--
-- The agent hierarchy is organisational, not a property of the OpenCode
-- session: a sessionless agent can still be placed under another, and a
-- sessionless orchestrator can still have children. Keep the edge on the
-- existing registry row because every agent already has exactly one row here
-- (session id may be NULL); a separate hierarchy table would only add a join
-- and a second update path for the same one-parent-per-agent rule.
--
-- When this is set it wins over parent_session_id in roster and tree
-- resolution. parent_session_id stays as the raw identity the chat plugin
-- reports, so unregistered plugin parents keep working. No new table.

ALTER TABLE agent_sessions ADD COLUMN parent_agent_name TEXT;

CREATE INDEX idx_agent_sessions_parent_name ON agent_sessions(lower(parent_agent_name));
