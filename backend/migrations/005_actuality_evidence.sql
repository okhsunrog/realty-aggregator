ALTER TABLE listings
    ADD COLUMN feed_is_actual INTEGER,
    ADD COLUMN feed_observed_at TIMESTAMPTZ,
    ADD COLUMN feed_history_synced_at TIMESTAMPTZ,
    ADD COLUMN source_confirmed_status TEXT CHECK (source_confirmed_status IN ('active', 'inactive')),
    ADD COLUMN source_confirmed_at TIMESTAMPTZ;

-- An unsuccessful later attempt must not erase the last definitive observation.
UPDATE listings SET source_confirmed_status = CASE WHEN is_active THEN 'active' ELSE 'inactive' END,
    source_confirmed_at = actuality_checked_at
WHERE actuality_origin = 'source_page' AND actuality_checked_at IS NOT NULL;

CREATE TABLE source_check_hosts (
    source TEXT PRIMARY KEY,
    failures INTEGER NOT NULL DEFAULT 0,
    retry_after TIMESTAMPTZ,
    last_error TEXT,
    checked_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX listings_feed_history_pending ON listings (id)
    WHERE feed_history_synced_at IS NULL;
