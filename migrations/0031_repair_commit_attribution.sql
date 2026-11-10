-- Repair the one commit attribution written under a borrowed session
-- (BB-215, root cause BB-196).
--
-- A shared breadboard MCP process used to send a session id captured at
-- startup, so `record_commit` could attach a commit to another conversation's
-- agent and session. The client now sends the calling conversation's id on
-- every MCP request. This migration corrects the record that was written
-- before the fix; the model and provider were already right and stay.
--
-- The update matches every identifying field of the known bad row, not just
-- its id, so it can never rewrite an unrelated record with the same row id on
-- another database.
--
-- Row 3 claimed `ciabatta captain` on ses_e3af688a... for
-- Breadway/breadboard@9e7dfdd..., but BB-199 shows the work was made by
-- `buttered crumpet` on ses_e3afa697.... `study steward` was that other
-- agent's orchestrator, so the borrowed orchestrator is cleared.

UPDATE commit_attribution
   SET agent_name = 'buttered crumpet',
       session_id = 'ses_e3afa697affepTJpGLIu9XZcvz',
       orchestrator = NULL,
       orchestrator_session_id = NULL,
       updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
 WHERE id = 3
   AND repo = 'Breadway/breadboard'
   AND sha = '9e7dfddb0d4a0866e682f5cb578ce54b722fa0e2'
   AND agent_name = 'ciabatta captain'
   AND session_id = 'ses_e3af688a2ffeVHhZrE50ZBqNu6';

-- The activity timeline is written from the same attribution, so leaving it
-- on the old name would keep the drawer lying after the row is corrected.
UPDATE sync_log
   SET new_value = replace(new_value, 'by ciabatta captain', 'by buttered crumpet')
 WHERE field = 'commit'
   AND issue_id = (SELECT id FROM issues WHERE key = 'BB-199')
   AND new_value LIKE '%9e7dfddb0d%'
   AND new_value LIKE '%by ciabatta captain%';
