use sqlx::PgPool;

use super::models::{
    CollectionConfig, CollectionState, DbListing, ListingForAnalysis, NewCollectionConfig,
    VisionStatus,
};

pub struct Repository {
    pool: PgPool,
}

impl Repository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    // Collection configs
    pub async fn get_active_configs(&self) -> Result<Vec<CollectionConfig>, sqlx::Error> {
        sqlx::query_as::<_, CollectionConfig>(
            "SELECT id, city, category_id, is_active, created_at FROM collection_configs WHERE is_active = true"
        )
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_all_configs(&self) -> Result<Vec<CollectionConfig>, sqlx::Error> {
        sqlx::query_as::<_, CollectionConfig>(
            "SELECT id, city, category_id, is_active, created_at FROM collection_configs ORDER BY created_at DESC"
        )
        .fetch_all(&self.pool)
        .await
    }

    pub async fn create_config(
        &self,
        config: NewCollectionConfig,
    ) -> Result<CollectionConfig, sqlx::Error> {
        sqlx::query_as::<_, CollectionConfig>(
            r#"
            INSERT INTO collection_configs (city, category_id)
            VALUES ($1, $2)
            RETURNING id, city, category_id, is_active, created_at
            "#,
        )
        .bind(&config.city)
        .bind(config.category_id.unwrap_or(2))
        .fetch_one(&self.pool)
        .await
    }

    pub async fn delete_config(&self, id: i32) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM collection_configs WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn toggle_config(
        &self,
        id: i32,
        is_active: bool,
    ) -> Result<Option<CollectionConfig>, sqlx::Error> {
        sqlx::query_as::<_, CollectionConfig>(
            r#"
            UPDATE collection_configs SET is_active = $2 WHERE id = $1
            RETURNING id, city, category_id, is_active, created_at
            "#,
        )
        .bind(id)
        .bind(is_active)
        .fetch_optional(&self.pool)
        .await
    }

    // Listings
    pub async fn upsert_listing(&self, listing: &DbListing) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO listings (
                id, source, url, title, description, price, city, address,
                lat, lng, rooms, area, floor, floors_total, person_type,
                photos, params, time_created, time_updated, fetched_at, is_active
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21
            )
            ON CONFLICT (id) DO UPDATE SET
                source = EXCLUDED.source,
                url = EXCLUDED.url,
                title = EXCLUDED.title,
                description = EXCLUDED.description,
                price = EXCLUDED.price,
                city = EXCLUDED.city,
                address = EXCLUDED.address,
                lat = EXCLUDED.lat,
                lng = EXCLUDED.lng,
                rooms = EXCLUDED.rooms,
                area = EXCLUDED.area,
                floor = EXCLUDED.floor,
                floors_total = EXCLUDED.floors_total,
                person_type = EXCLUDED.person_type,
                photos = EXCLUDED.photos,
                params = EXCLUDED.params,
                time_created = EXCLUDED.time_created,
                time_updated = EXCLUDED.time_updated,
                fetched_at = EXCLUDED.fetched_at,
                is_active = EXCLUDED.is_active
            "#
        )
        .bind(listing.id)
        .bind(&listing.source)
        .bind(&listing.url)
        .bind(&listing.title)
        .bind(&listing.description)
        .bind(listing.price)
        .bind(&listing.city)
        .bind(&listing.address)
        .bind(listing.lat)
        .bind(listing.lng)
        .bind(listing.rooms)
        .bind(listing.area)
        .bind(listing.floor)
        .bind(listing.floors_total)
        .bind(listing.person_type)
        .bind(&listing.photos)
        .bind(&listing.params)
        .bind(listing.time_created)
        .bind(listing.time_updated)
        .bind(listing.fetched_at)
        .bind(listing.is_active)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_listings(&self, listings: &[DbListing]) -> Result<usize, sqlx::Error> {
        let mut count = 0;
        for listing in listings {
            self.upsert_listing(listing).await?;
            count += 1;
        }
        Ok(count)
    }

    pub async fn mark_listing_inactive(&self, id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("UPDATE listings SET is_active = false WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn get_listings_count(&self) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM listings")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    pub async fn get_active_listings_count(&self) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM listings WHERE is_active = true")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    pub async fn get_last_listing_id(
        &self,
        city: &str,
        category_id: i32,
    ) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as("SELECT MAX(id) FROM listings WHERE city LIKE $1")
            .bind(format!("%{}%", city))
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.0))
    }

    // Collection state (для инкрементальной загрузки)
    pub async fn get_collection_state(&self) -> Result<CollectionState, sqlx::Error> {
        sqlx::query_as::<_, CollectionState>(
            "SELECT id, last_listing_id, last_collection_at FROM collection_state WHERE id = 1",
        )
        .fetch_one(&self.pool)
        .await
    }

    pub async fn update_collection_state(&self, last_listing_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE collection_state SET last_listing_id = $1, last_collection_at = NOW() WHERE id = 1"
        )
        .bind(last_listing_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // Vision analysis
    pub async fn save_vision_analysis(
        &self,
        listing_id: i64,
        analysis: &serde_json::Value,
        model: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE listings
            SET vision_analysis = $2,
                vision_analyzed_at = NOW(),
                vision_model = $3,
                vision_status = 'done',
                vision_attempts = vision_attempts + 1
            WHERE id = $1
            "#,
        )
        .bind(listing_id)
        .bind(analysis)
        .bind(model)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_vision_status(
        &self,
        listing_id: i64,
        status: VisionStatus,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE listings
            SET vision_status = $2,
                vision_last_error = $3,
                vision_attempts = vision_attempts + 1
            WHERE id = $1
            "#,
        )
        .bind(listing_id)
        .bind(status)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_vision_status_not_needed(
        &self,
        listing_id: i64,
        reason: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE listings
            SET vision_status = 'not_needed', vision_last_error = $2
            WHERE id = $1
            "#,
        )
        .bind(listing_id)
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_listings_for_analysis(
        &self,
        limit: i64,
    ) -> Result<Vec<ListingForAnalysis>, sqlx::Error> {
        sqlx::query_as::<_, ListingForAnalysis>(
            r#"
            SELECT id, photos, price, area, source, lat, lng,
                   images_stored, images_count, vision_status, vision_attempts
            FROM listings
            WHERE vision_status = 'pending'
              AND is_active = true
            ORDER BY fetched_at DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_listings_needing_images(
        &self,
        limit: i64,
    ) -> Result<Vec<ListingForAnalysis>, sqlx::Error> {
        sqlx::query_as::<_, ListingForAnalysis>(
            r#"
            SELECT id, photos, price, area, source, lat, lng,
                   images_stored, images_count, vision_status, vision_attempts
            FROM listings
            WHERE images_stored = false
              AND photos IS NOT NULL
              AND is_active = true
            ORDER BY fetched_at DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn mark_images_stored(&self, listing_id: i64, count: i32) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE listings SET images_stored = true, images_count = $2 WHERE id = $1")
            .bind(listing_id)
            .bind(count)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_pending_analysis_count(&self) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM listings WHERE vision_status = 'pending' AND is_active = true",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn get_vision_stats(&self) -> Result<VisionStats, sqlx::Error> {
        let row: (i64, i64, i64, i64) = sqlx::query_as(
            r#"
            SELECT
                COUNT(*) FILTER (WHERE vision_status = 'pending') as pending,
                COUNT(*) FILTER (WHERE vision_status = 'done') as done,
                COUNT(*) FILTER (WHERE vision_status = 'not_needed') as not_needed,
                COUNT(*) FILTER (WHERE vision_status = 'failed') as failed
            FROM listings
            WHERE is_active = true
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(VisionStats {
            pending: row.0,
            done: row.1,
            not_needed: row.2,
            failed: row.3,
        })
    }

    pub async fn get_listing_by_id(
        &self,
        id: i64,
    ) -> Result<Option<ListingForAnalysis>, sqlx::Error> {
        sqlx::query_as::<_, ListingForAnalysis>(
            r#"
            SELECT id, photos, price, area, source, lat, lng,
                   images_stored, images_count, vision_status, vision_attempts
            FROM listings WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    // Legacy method for compatibility
    pub async fn get_unanalyzed_count(&self) -> Result<i64, sqlx::Error> {
        self.get_pending_analysis_count().await
    }
}

#[derive(Debug, serde::Serialize)]
pub struct VisionStats {
    pub pending: i64,
    pub done: i64,
    pub not_needed: i64,
    pub failed: i64,
}
