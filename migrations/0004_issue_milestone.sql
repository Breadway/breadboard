-- Forgejo milestone title, synced inbound onto the issue. breadboard has no
-- milestone concept of its own (cycles are the sprint primitive), so this is
-- just a passthrough label for filtering / display.

ALTER TABLE issues ADD COLUMN milestone TEXT;
