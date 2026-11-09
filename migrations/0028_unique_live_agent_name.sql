-- One live agent name belongs to exactly one session (BB-170).
--
-- The roster is still keyed by lowercased name, but a name may only be held
-- by one session while that session is live. "Live" is the same 120 second
-- heartbeat window the roster already uses (AGENT_LIVE_SECS): a registration,
-- claim or chat write from a different session id is refused with a 409 while
-- the holder is inside the window. Once the holder stops heartbeating, the
-- name is reclaimable by overwriting the stale row.
--
-- Existing duplicates are resolved here. A name's holder is the session with
-- the most recent activity across the session row and its claims. Other
-- session ids found on the same name are not deleted and their claims are not
-- touched; they are recorded as displaced, unregistered sessions so they stay
-- discoverable for audit.
--
-- `agent_sessions` keeps its name primary key: the current session row is the
-- holder. A stale holder can be replaced in place, and every claim that
-- carried the old session id remains in `agent_claims` with that id.

CREATE TABLE agent_name_conflicts (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_name        TEXT    NOT NULL,
    session_id        TEXT    NOT NULL,
    holder_session_id TEXT,
    first_seen        TEXT,
    last_seen         TEXT,
    recorded_at       TEXT    NOT NULL
);

CREATE INDEX idx_agent_name_conflicts_name ON agent_name_conflicts(agent_name);
CREATE INDEX idx_agent_name_conflicts_at ON agent_name_conflicts(recorded_at);

-- Every session id ever associated with a name, with its latest activity.
-- The session row can carry one id and claims can carry others; both count.
CREATE TEMP TABLE agent_name_candidates AS
SELECT
    name,
    session_id,
    MAX(at) AS at
FROM (
    SELECT agent_name AS name, session_id, last_seen AS at
      FROM agent_sessions
     WHERE session_id IS NOT NULL
    UNION ALL
    SELECT lower(agent_name) AS name, session_id, last_heartbeat AS at
      FROM agent_claims
     WHERE session_id IS NOT NULL
)
GROUP BY name, session_id;

-- The holder is the most recently active session for each name. Tie-break on
-- the session id so the result is deterministic.
CREATE TEMP TABLE agent_name_holders AS
SELECT name, session_id, at
FROM (
    SELECT
        name,
        session_id,
        at,
        ROW_NUMBER() OVER (
            PARTITION BY name
            ORDER BY julianday(at) DESC, session_id DESC
        ) AS rank
    FROM agent_name_candidates
)
WHERE rank = 1;

INSERT INTO agent_name_conflicts
    (agent_name, session_id, holder_session_id, first_seen, last_seen, recorded_at)
SELECT
    c.name,
    c.session_id,
    h.session_id,
    c.at,
    c.at,
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_name_candidates c
JOIN agent_name_holders h ON h.name = c.name
WHERE c.session_id <> h.session_id;

-- Make the most recently active session the holder and move last_seen forward
-- when a claim was newer than the registration row. Claims are untouched.
UPDATE agent_sessions
   SET session_id = (
           SELECT h.session_id
             FROM agent_name_holders h
            WHERE h.name = agent_sessions.agent_name
       ),
       last_seen = (
           SELECT CASE
                      WHEN julianday(h.at) > julianday(agent_sessions.last_seen)
                      THEN h.at
                      ELSE agent_sessions.last_seen
                  END
             FROM agent_name_holders h
            WHERE h.name = agent_sessions.agent_name
       )
 WHERE EXISTS (
           SELECT 1
             FROM agent_name_holders h
            WHERE h.name = agent_sessions.agent_name
       );

DROP TABLE agent_name_holders;
DROP TABLE agent_name_candidates;
