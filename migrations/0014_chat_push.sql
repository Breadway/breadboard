-- Chat push: per-agent subscriptions for group rooms, plus a push inbox.
--
-- DMs and @pings always create pushes; group rooms only for members with a
-- subscription row. `member` uses the same "agent:<name>" / "human:<login>"
-- keys as the rest of chat, and (member, message_id) is unique so a ping in a
-- subscribed room does not duplicate.

CREATE TABLE chat_push_subs (
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    member     TEXT    NOT NULL,
    room_id    INTEGER NOT NULL REFERENCES chat_rooms(id) ON DELETE CASCADE,
    created_at TEXT    NOT NULL,
    PRIMARY KEY (user_id, member, room_id)
);

CREATE TABLE chat_pushes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    member     TEXT    NOT NULL,
    room_id    INTEGER NOT NULL REFERENCES chat_rooms(id) ON DELETE CASCADE,
    message_id INTEGER NOT NULL REFERENCES chat_messages(id) ON DELETE CASCADE,
    kind       TEXT    NOT NULL CHECK (kind IN ('dm', 'ping', 'room')),
    created_at TEXT    NOT NULL,
    UNIQUE (member, message_id)
);

CREATE INDEX idx_chat_pushes_member ON chat_pushes(member, id);
