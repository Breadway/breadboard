-- Session linkage and required handoff notes (BB-162).
--
-- `issue_sessions` records which sessions touched an issue and when. It is an
-- append-only-by-claim link: the row is created on claim, refreshed on every
-- heartbeat and on release, and deliberately never deleted by a release. That
-- lets a fresh session see which sessions worked the issue and pick up from
-- the last state instead of rediscovering it. Session ids are the same
-- identifiers the roster (`agent_sessions.session_id`) and usage stats
-- (`usage_sessions.session_id`) already use.
--
-- `handoff_notes` is the release gate. A release must carry one line covering
-- what is done, what is next and what is risky. The note is stored here (not
-- only on the transient claim row) so it survives the claim, shows on the
-- board card, appears in the agent activity timeline as a `handoff` event, and
-- is returned as context when a session resumes the issue. A release that
-- never held work may use an explicit `no_note` instead; that path writes no
-- row here.

CREATE TABLE issue_sessions (
    issue_id   INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    session_id TEXT    NOT NULL,
    agent_name TEXT    NOT NULL,
    first_seen TEXT    NOT NULL,
    last_seen  TEXT    NOT NULL,
    last_state TEXT,
    PRIMARY KEY (issue_id, session_id)
);

CREATE INDEX idx_issue_sessions_session ON issue_sessions(session_id, last_seen DESC);

-- Backfill from live claims so the first release after deploy already has a
-- usable history. `first_seen` is the claim time; `last_seen` the heartbeat.
INSERT OR REPLACE INTO issue_sessions
    (issue_id, session_id, agent_name, first_seen, last_seen, last_state)
SELECT c.issue_id,
       c.session_id,
       lower(c.agent_name),
       c.claimed_at,
       c.last_heartbeat,
       c.state
  FROM agent_claims c
 WHERE c.session_id IS NOT NULL AND trim(c.session_id) <> '';

CREATE TABLE handoff_notes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id   INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    session_id TEXT,
    agent_name TEXT    NOT NULL,
    mode       TEXT    NOT NULL,
    state      TEXT    NOT NULL,
    note       TEXT    NOT NULL,
    created_at TEXT    NOT NULL
);

CREATE INDEX idx_handoff_notes_issue ON handoff_notes(issue_id, created_at DESC, id DESC);
CREATE INDEX idx_handoff_notes_session ON handoff_notes(session_id, created_at DESC);
