-- Subagent links: the session that spawned this agent, nullable.
--
-- The breadboard-chat plugin knows the OpenCode parentID and reports it on
-- registration. Resolution to an orchestrator name happens in the roster, so
-- an unregistered parent still keeps its raw session id instead of losing the
-- link. No new table: the session row already has the identity.

ALTER TABLE agent_sessions ADD COLUMN parent_session_id TEXT;

CREATE INDEX idx_agent_sessions_parent ON agent_sessions(parent_session_id);
