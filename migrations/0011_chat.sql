-- Agent chat: a global lobby, arbitrary extra rooms, and DMs.
--
-- Members use prefixed keys ("agent:<name>" / "human:<login>") so an agent
-- name and a human login cannot collide. A DM's canonical key is the sorted
-- pair of those members; shared rooms have a unique name (and an optional
-- description) instead. Humans can read and post to every room; agents only
-- see shared rooms and DMs they belong to.

CREATE TABLE chat_rooms (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    kind        TEXT    NOT NULL CHECK (kind IN ('room', 'dm')),
    name        TEXT,
    description TEXT,
    dm_key      TEXT    UNIQUE,
    created_at  TEXT    NOT NULL,
    CHECK (
        (kind = 'room' AND name IS NOT NULL AND dm_key IS NULL) OR
        (kind = 'dm' AND name IS NULL AND dm_key IS NOT NULL)
    )
);

CREATE UNIQUE INDEX idx_chat_room_name ON chat_rooms(name) WHERE kind = 'room';

INSERT INTO chat_rooms (kind, name, description, created_at)
VALUES ('room', 'lobby', 'The global room. Everyone lands here.', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));

CREATE TABLE chat_room_members (
    room_id INTEGER NOT NULL REFERENCES chat_rooms(id) ON DELETE CASCADE,
    member  TEXT    NOT NULL,
    PRIMARY KEY (room_id, member)
);

CREATE INDEX idx_chat_room_members_member ON chat_room_members(member);

CREATE TABLE chat_messages (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    room_id        INTEGER NOT NULL REFERENCES chat_rooms(id) ON DELETE CASCADE,
    author_member  TEXT    NOT NULL,
    author_display TEXT    NOT NULL,
    author_kind    TEXT    NOT NULL CHECK (author_kind IN ('agent', 'human')),
    body           TEXT    NOT NULL,
    created_at     TEXT    NOT NULL
);

CREATE INDEX idx_chat_messages_room ON chat_messages(room_id, id);
