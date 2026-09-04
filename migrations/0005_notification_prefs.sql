-- Per-user notification preferences. A missing row means "all defaults on".

CREATE TABLE notification_prefs (
    user_id  INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    assigned INTEGER NOT NULL DEFAULT 1,   -- issue assigned to me
    mention  INTEGER NOT NULL DEFAULT 1,   -- @me in a comment / description
    cycle    INTEGER NOT NULL DEFAULT 1,   -- a cycle I have issues in starts / ends
    digest   INTEGER NOT NULL DEFAULT 0    -- daily summary of unread
);
