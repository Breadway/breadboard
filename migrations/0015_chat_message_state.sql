-- Message soft delete and room compaction.
--
-- Deleted messages stay in the table for audit but disappear from lists,
-- unread counts, pushes, and chat_wait. A compaction inserts one summary row
-- and marks every visible message it replaces with compacted_at, so the room
-- shows the summary instead of the back and forth.

ALTER TABLE chat_messages ADD COLUMN message_kind TEXT NOT NULL DEFAULT 'message';
ALTER TABLE chat_messages ADD COLUMN deleted_at TEXT;
ALTER TABLE chat_messages ADD COLUMN deleted_by TEXT;
ALTER TABLE chat_messages ADD COLUMN compacted_at TEXT;

CREATE INDEX idx_chat_messages_visible
    ON chat_messages(room_id, id)
    WHERE deleted_at IS NULL AND compacted_at IS NULL;
