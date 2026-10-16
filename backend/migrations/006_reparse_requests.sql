CREATE TABLE listing_reparse_requests (
    listing_id BIGINT PRIMARY KEY REFERENCES listings(id) ON DELETE CASCADE,
    requested_at TIMESTAMPTZ NOT NULL,
    queued BOOLEAN NOT NULL
);
