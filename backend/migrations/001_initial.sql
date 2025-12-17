-- Image download status enum
CREATE TYPE image_status AS ENUM ('pending', 'downloaded', 'not_found', 'gone', 'failed');

-- Collection configs (для возможности собирать из разных регионов в будущем)
CREATE TABLE collection_configs (
    id SERIAL PRIMARY KEY,
    city TEXT NOT NULL,
    category_id INTEGER NOT NULL DEFAULT 2,
    is_active BOOLEAN DEFAULT true,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- Основная таблица объявлений
CREATE TABLE listings (
    id BIGINT PRIMARY KEY,
    source TEXT NOT NULL,
    url TEXT NOT NULL,
    title TEXT,
    description TEXT,
    price BIGINT,
    city TEXT NOT NULL,
    address TEXT,
    lat DOUBLE PRECISION,
    lng DOUBLE PRECISION,
    rooms INTEGER,
    area DOUBLE PRECISION,
    floor INTEGER,
    floors_total INTEGER,
    person_type INTEGER,
    params JSONB,
    time_created TIMESTAMPTZ,
    time_updated TIMESTAMPTZ,
    fetched_at TIMESTAMPTZ NOT NULL,
    is_active BOOLEAN DEFAULT true,

    -- Listing-level AI analysis (second layer - gemini-2.5-flash-lite)
    analysis_condition INTEGER,              -- 0-100 overall condition
    analysis_is_shell BOOLEAN,               -- likely bare shell?
    analysis_move_in_ready BOOLEAN,          -- can move in immediately?
    analysis_red_flags TEXT[],               -- ["общежитие", "mold", ...]
    analysis_summary TEXT,                   -- Russian summary from AI
    analysis_model TEXT,                     -- model used for analysis
    analyzed_at TIMESTAMPTZ
);

CREATE INDEX idx_listings_city ON listings(city);
CREATE INDEX idx_listings_price ON listings(price);
CREATE INDEX idx_listings_fetched ON listings(fetched_at);
CREATE INDEX idx_listings_active ON listings(is_active);
CREATE INDEX idx_listings_condition ON listings(analysis_condition) WHERE analysis_condition IS NOT NULL;
CREATE INDEX idx_listings_not_analyzed ON listings(id) WHERE analyzed_at IS NULL;

-- Images table (source of truth for all image data)
CREATE TABLE images (
    id BIGSERIAL PRIMARY KEY,
    listing_id BIGINT NOT NULL REFERENCES listings(id) ON DELETE CASCADE,
    image_index INTEGER NOT NULL,
    source_url TEXT NOT NULL,

    -- Download status
    status image_status NOT NULL DEFAULT 'pending',
    s3_key TEXT,
    http_status INTEGER,
    download_attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    downloaded_at TIMESTAMPTZ,

    -- Per-image AI analysis (first layer - gemma-3-4b-it)
    ai_image_type TEXT,                      -- interior|exterior|floor_plan|other
    ai_room TEXT,                            -- bathroom|kitchen|living|bedroom|hallway|null
    ai_condition INTEGER,                    -- 0-100
    ai_confidence INTEGER,                   -- 0-100
    ai_issues TEXT[],                        -- ["water_damage", "mold"]
    ai_description TEXT,                     -- one sentence
    ai_model TEXT,                           -- model used
    ai_analyzed_at TIMESTAMPTZ,

    created_at TIMESTAMPTZ DEFAULT NOW(),

    UNIQUE(listing_id, image_index)
);

CREATE INDEX idx_images_listing ON images(listing_id);
CREATE INDEX idx_images_pending_download ON images(status) WHERE status = 'pending';
CREATE INDEX idx_images_retry_download ON images(status) WHERE status = 'not_found' AND download_attempts < 2;
CREATE INDEX idx_images_not_analyzed ON images(listing_id) WHERE ai_analyzed_at IS NULL AND status = 'downloaded';

-- Состояние сбора данных (для инкрементальной загрузки через startid)
CREATE TABLE collection_state (
    id SERIAL PRIMARY KEY,
    last_listing_id BIGINT,
    last_collection_at TIMESTAMPTZ
);

INSERT INTO collection_state (id) VALUES (1);
