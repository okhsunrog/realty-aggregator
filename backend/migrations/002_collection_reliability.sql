ALTER TABLE listings
    ADD COLUMN source_time TIMESTAMPTZ,
    ADD COLUMN analysis_attempts INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN analysis_retry_at TIMESTAMPTZ,
    ADD COLUMN analysis_last_error TEXT,
    ADD COLUMN actuality_checked_at TIMESTAMPTZ,
    ADD COLUMN actuality_origin TEXT NOT NULL DEFAULT 'source_feed';

ALTER TABLE images
    ADD COLUMN ai_attempts INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN ai_retry_at TIMESTAMPTZ,
    ADD COLUMN ai_last_error TEXT;

ALTER TABLE collection_state
    ADD COLUMN query_fingerprint TEXT,
    ADD COLUMN last_sync_at TIMESTAMPTZ;

-- Earlier versions encoded lack of evidence as a negative condition assessment.
UPDATE listings SET analysis_condition = NULL, analysis_is_shell = NULL,
    analysis_move_in_ready = NULL, analysis_red_flags = NULL, analysis_summary = NULL,
    analysis_model = NULL, analyzed_at = NULL
WHERE analysis_model = 'none'
   OR analysis_red_flags && ARRAY['Нет фото', 'Нет фото интерьера'];

CREATE TABLE listing_price_history (
    listing_id BIGINT NOT NULL REFERENCES listings(id) ON DELETE CASCADE,
    observed_at TIMESTAMPTZ NOT NULL,
    price BIGINT NOT NULL,
    PRIMARY KEY (listing_id, observed_at)
);

INSERT INTO listing_price_history (listing_id, observed_at, price)
SELECT id, fetched_at, price FROM listings WHERE price IS NOT NULL;

CREATE TABLE listing_reviews (
    listing_id BIGINT PRIMARY KEY REFERENCES listings(id) ON DELETE CASCADE,
    favorite BOOLEAN NOT NULL DEFAULT false,
    hidden BOOLEAN NOT NULL DEFAULT false,
    stage TEXT NOT NULL DEFAULT 'new'
        CHECK (stage IN ('new', 'contact', 'visit', 'documents', 'approved', 'rejected')),
    notes TEXT NOT NULL DEFAULT ''
);
