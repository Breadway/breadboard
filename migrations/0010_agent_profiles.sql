-- Per-agent one-liner, independent of any single issue claim.
--
-- Identity is (user, agent name), matching how the Agents tab groups claims.
-- An empty message clears the row. Claims join this table so the message
-- travels with every claim the agent holds.

CREATE TABLE agent_profiles (
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    agent_name TEXT    NOT NULL,
    message    TEXT    NOT NULL,
    updated_at TEXT    NOT NULL,
    PRIMARY KEY (user_id, agent_name)
);

CREATE INDEX idx_agent_profiles_name ON agent_profiles(agent_name);
