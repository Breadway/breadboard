-- Work budgets (BB-154): every exclusive claim check scans live claims by
-- agent name before deciding whether the per-agent or per-orchestrator budget
-- is full. The limits themselves live in `breadboard.toml` (defaults 10 and
-- 100), so no table or column is needed here; this index keeps the count
-- cheap on a busy board.
--
-- Names are lowercased on write, so the index matches the equality lookups the
-- store uses for a single agent. Whole-tree checks still group by name, which
-- this index can serve as a covering scan over `mode = 'exclusive'`.

CREATE INDEX IF NOT EXISTS idx_agent_claims_mode_agent
    ON agent_claims(mode, agent_name);
