-- Track the last daily digest sent per user so the digest window neither
-- overlaps nor repeats. NULL = no digest sent yet (first run covers all
-- currently-unread items).

ALTER TABLE notification_prefs ADD COLUMN last_digest_at TEXT;
