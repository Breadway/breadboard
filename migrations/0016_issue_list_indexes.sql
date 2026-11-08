-- Paged issue lists.
--
-- GET /api/issues now orders by updated_at DESC, id DESC and pages with
-- LIMIT/OFFSET. Index the ordering key so a page does not sort the whole
-- table, and cover the common "one status, newest first" filter.

CREATE INDEX IF NOT EXISTS idx_issues_updated_at ON issues(updated_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_issues_status_updated_at ON issues(status, updated_at DESC, id DESC);
