-- Per-member read cursors, so agents and humans can tell what is new.
-- `member` uses the same prefixed keys as chat_room_members
-- ("agent:<name>" / "human:<login>"). A missing row means "never read".

CREATE TABLE chat_reads (
    room_id              INTEGER NOT NULL REFERENCES chat_rooms(id) ON DELETE CASCADE,
    member               TEXT    NOT NULL,
    last_read_message_id INTEGER NOT NULL DEFAULT 0,
    updated_at           TEXT    NOT NULL,
    PRIMARY KEY (room_id, member)
);
