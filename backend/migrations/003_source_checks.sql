ALTER TABLE listings
    ADD COLUMN source_check_status TEXT CHECK (source_check_status IN ('active', 'inactive', 'unknown')),
    ADD COLUMN source_checked_at TIMESTAMPTZ,
    ADD COLUMN source_check_http_status INTEGER,
    ADD COLUMN source_check_reason TEXT;

CREATE INDEX listings_source_check_queue ON listings (source_checked_at NULLS FIRST, id) WHERE is_active = true;
