-- Human-readable session title on the agent roster (BB-189).
--
-- The OpenCode session title ("Implement BB-170 name uniqueness") is reported
-- by the breadboard-chat plugin on registration. It is display-only: the full
-- session id stays the identity used for ownership and parent resolution.
--
-- Registration only replaces the title when it receives a new non-empty value.
-- An omitted or empty title, a rename, or a heartbeat never clears a stored
-- one, so the roster keeps showing the last meaningful name for a session.

ALTER TABLE agent_sessions ADD COLUMN session_title TEXT;
