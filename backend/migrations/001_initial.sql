-- Vision analysis status enum
CREATE TYPE vision_status AS ENUM ('pending', 'done', 'not_needed', 'failed');

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
    photos JSONB,
    params JSONB,
    time_created TIMESTAMPTZ,
    time_updated TIMESTAMPTZ,
    fetched_at TIMESTAMPTZ NOT NULL,
    is_active BOOLEAN DEFAULT true,

    -- Image storage
    images_stored BOOLEAN NOT NULL DEFAULT false,
    images_count INTEGER NOT NULL DEFAULT 0,

    -- Vision analysis
    vision_status vision_status NOT NULL DEFAULT 'pending',
    vision_analysis JSONB,
    vision_analyzed_at TIMESTAMPTZ,
    vision_model TEXT,
    vision_attempts INTEGER NOT NULL DEFAULT 0,
    vision_last_error TEXT
);

CREATE INDEX idx_listings_city ON listings(city);
CREATE INDEX idx_listings_price ON listings(price);
CREATE INDEX idx_listings_fetched ON listings(fetched_at);
CREATE INDEX idx_listings_active ON listings(is_active);
CREATE INDEX idx_listings_vision_status ON listings(vision_status) WHERE vision_status = 'pending';
CREATE INDEX idx_listings_images_stored ON listings(images_stored) WHERE images_stored = false;

-- Состояние сбора данных (для инкрементальной загрузки через startid)
CREATE TABLE collection_state (
    id SERIAL PRIMARY KEY,
    last_listing_id BIGINT,
    last_collection_at TIMESTAMPTZ
);

INSERT INTO collection_state (id) VALUES (1);
