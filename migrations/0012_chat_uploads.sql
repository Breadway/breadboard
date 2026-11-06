-- Chat attachments. Files live under the upload dir (the data volume in the
-- container) and are linked to one message once it is posted. `path` is the
-- stored filename inside that dir, never a user-supplied path.

CREATE TABLE chat_uploads (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id       INTEGER REFERENCES chat_messages(id) ON DELETE CASCADE,
    uploader_user_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
    filename         TEXT    NOT NULL,
    mime             TEXT    NOT NULL,
    size             INTEGER NOT NULL,
    path             TEXT    NOT NULL,
    created_at       TEXT    NOT NULL
);

CREATE INDEX idx_chat_uploads_message ON chat_uploads(message_id);
CREATE INDEX idx_chat_uploads_uploader ON chat_uploads(uploader_user_id);
