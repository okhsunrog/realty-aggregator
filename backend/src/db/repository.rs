use sqlx::PgPool;

use super::models::{
    CollectionConfig, CollectionState, DbImage, DbListing, ImageAnalysisResult, ImageForAnalysis,
    ImageForDownload, ImageStatus, ListingAnalysisResult, ListingForAnalysis, NewCollectionConfig,
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

    // ==================== Collection configs ====================

    pub async fn get_active_configs(&self) -> Result<Vec<CollectionConfig>, sqlx::Error> {
        sqlx::query_as!(
            CollectionConfig,
            r#"
            SELECT id, city, category_id, is_active, created_at
            FROM collection_configs WHERE is_active = true
            "#
        )
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_all_configs(&self) -> Result<Vec<CollectionConfig>, sqlx::Error> {
        sqlx::query_as!(
            CollectionConfig,
            r#"
            SELECT id, city, category_id, is_active, created_at
            FROM collection_configs ORDER BY created_at DESC
            "#
        )
        .fetch_all(&self.pool)
        .await
    }

    pub async fn create_config(
        &self,
        config: NewCollectionConfig,
    ) -> Result<CollectionConfig, sqlx::Error> {
        sqlx::query_as!(
            CollectionConfig,
            r#"
            INSERT INTO collection_configs (city, category_id)
            VALUES ($1, $2)
            RETURNING id, city, category_id, is_active, created_at
            "#,
            &config.city,
            config.category_id.unwrap_or(2)
        )
        .fetch_one(&self.pool)
        .await
    }

    pub async fn delete_config(&self, id: i32) -> Result<bool, sqlx::Error> {
        let result = sqlx::query!("DELETE FROM collection_configs WHERE id = $1", id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn toggle_config(
        &self,
        id: i32,
        is_active: bool,
    ) -> Result<Option<CollectionConfig>, sqlx::Error> {
        sqlx::query_as!(
            CollectionConfig,
            r#"
            UPDATE collection_configs SET is_active = $2 WHERE id = $1
            RETURNING id, city, category_id, is_active, created_at
            "#,
            id,
            is_active
        )
        .fetch_optional(&self.pool)
        .await
    }

    // ==================== Listings ====================

    pub async fn upsert_listing(&self, listing: &DbListing) -> Result<bool, sqlx::Error> {
        let result = sqlx::query!(
            r#"
            INSERT INTO listings (
                id, source, url, title, description, price, city, address,
                lat, lng, rooms, area, floor, floors_total, person_type,
                params, time_created, time_updated, fetched_at, is_active
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20
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
                params = EXCLUDED.params,
                time_created = EXCLUDED.time_created,
                time_updated = EXCLUDED.time_updated,
                fetched_at = EXCLUDED.fetched_at,
                is_active = EXCLUDED.is_active
            "#,
            listing.id,
            &listing.source,
            &listing.url,
            listing.title.as_deref(),
            listing.description.as_deref(),
            listing.price,
            &listing.city,
            listing.address.as_deref(),
            listing.lat,
            listing.lng,
            listing.rooms,
            listing.area,
            listing.floor,
            listing.floors_total,
            listing.person_type,
            listing.params.as_ref(),
            listing.time_created,
            listing.time_updated,
            listing.fetched_at,
            listing.is_active
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn upsert_listing_with_images(
        &self,
        listing: &DbListing,
        photo_urls: &[String],
    ) -> Result<(), sqlx::Error> {
        self.upsert_listing(listing).await?;

        // Insert images (ignore conflicts for existing images)
        for (index, url) in photo_urls.iter().enumerate() {
            sqlx::query!(
                r#"
                INSERT INTO images (listing_id, image_index, source_url)
                VALUES ($1, $2, $3)
                ON CONFLICT (listing_id, image_index) DO NOTHING
                "#,
                listing.id,
                index as i32,
                url
            )
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    pub async fn mark_listing_inactive(&self, id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query!("UPDATE listings SET is_active = false WHERE id = $1", id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn get_listings_count(&self) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar!(r#"SELECT COUNT(*) AS "count!" FROM listings"#)
            .fetch_one(&self.pool)
            .await
    }

    pub async fn get_active_listings_count(&self) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar!(r#"SELECT COUNT(*) AS "count!" FROM listings WHERE is_active = true"#)
            .fetch_one(&self.pool)
            .await
    }

    /// Get old active listings for actuality checking
    pub async fn get_old_active_listings(
        &self,
        min_age_days: i32,
        limit: i64,
    ) -> Result<Vec<super::models::OldListingInfo>, sqlx::Error> {
        sqlx::query_as!(
            super::models::OldListingInfo,
            r#"
            SELECT id, price, time_created as created_at
            FROM listings
            WHERE is_active = true
              AND time_created < NOW() - INTERVAL '1 day' * $1::int
            ORDER BY time_created ASC
            LIMIT $2
            "#,
            min_age_days,
            limit
        )
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_listing_for_analysis(
        &self,
        listing_id: i64,
    ) -> Result<Option<ListingForAnalysis>, sqlx::Error> {
        sqlx::query_as!(
            ListingForAnalysis,
            r#"
            SELECT id, title, description, price, area, rooms, floor, floors_total,
                   address, source, lat, lng
            FROM listings WHERE id = $1
            "#,
            listing_id
        )
        .fetch_optional(&self.pool)
        .await
    }

    // ==================== Collection state ====================

    pub async fn get_collection_state(&self) -> Result<CollectionState, sqlx::Error> {
        sqlx::query_as!(
            CollectionState,
            "SELECT id, last_listing_id, last_collection_at FROM collection_state WHERE id = 1"
        )
        .fetch_one(&self.pool)
        .await
    }

    pub async fn update_collection_state(&self, last_listing_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
            UPDATE collection_state SET last_listing_id = $1, last_collection_at = NOW()
            WHERE id = 1
            "#,
            last_listing_id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_collection_time(&self) -> Result<(), sqlx::Error> {
        sqlx::query!("UPDATE collection_state SET last_collection_at = NOW() WHERE id = 1")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ==================== Images ====================

    /// Get images that need to be downloaded (pending or not_found with < 2 attempts)
    pub async fn get_images_for_download(
        &self,
        limit: i64,
    ) -> Result<Vec<ImageForDownload>, sqlx::Error> {
        sqlx::query_as!(
            ImageForDownload,
            r#"
            SELECT id, listing_id, image_index, source_url,
                   status AS "status: ImageStatus", download_attempts
            FROM images
            WHERE status = 'pending'
               OR (status = 'not_found' AND download_attempts < 2)
            ORDER BY listing_id, image_index
            LIMIT $1
            "#,
            limit
        )
        .fetch_all(&self.pool)
        .await
    }

    /// Update image after download attempt
    pub async fn update_image_download(
        &self,
        image_id: i64,
        status: ImageStatus,
        s3_key: Option<&str>,
        http_status: Option<i32>,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let downloaded_at = if status == ImageStatus::Downloaded {
            Some(chrono::Utc::now())
        } else {
            None
        };

        sqlx::query!(
            r#"
            UPDATE images
            SET status = $2,
                s3_key = COALESCE($3, s3_key),
                http_status = COALESCE($4, http_status),
                last_error = $5,
                download_attempts = download_attempts + 1,
                downloaded_at = COALESCE($6, downloaded_at)
            WHERE id = $1
            "#,
            image_id,
            status as ImageStatus,
            s3_key,
            http_status,
            error,
            downloaded_at
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get downloaded images that haven't been analyzed yet
    pub async fn get_images_for_ai_analysis(
        &self,
        limit: i64,
    ) -> Result<Vec<ImageForAnalysis>, sqlx::Error> {
        sqlx::query_as!(
            ImageForAnalysis,
            r#"
            SELECT id, listing_id, image_index, source_url, s3_key
            FROM images
            WHERE status = 'downloaded'
              AND ai_analyzed_at IS NULL
            ORDER BY listing_id, image_index
            LIMIT $1
            "#,
            limit
        )
        .fetch_all(&self.pool)
        .await
    }

    /// Save per-image AI analysis result
    pub async fn save_image_analysis(
        &self,
        image_id: i64,
        analysis: &ImageAnalysisResult,
        model: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
            UPDATE images
            SET ai_image_type = $2,
                ai_room = $3,
                ai_condition = $4,
                ai_confidence = $5,
                ai_issues = $6,
                ai_description = $7,
                ai_model = $8,
                ai_analyzed_at = NOW()
            WHERE id = $1
            "#,
            image_id,
            &analysis.image_type,
            analysis.room.as_deref(),
            analysis.condition,
            analysis.confidence,
            &analysis.issues,
            &analysis.description,
            model
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get all analyzed images for a listing (for second-layer analysis)
    pub async fn get_listing_image_analyses(
        &self,
        listing_id: i64,
    ) -> Result<Vec<DbImage>, sqlx::Error> {
        sqlx::query_as!(
            DbImage,
            r#"
            SELECT id, listing_id, image_index, source_url,
                   status AS "status: ImageStatus", s3_key, http_status,
                   download_attempts, last_error, downloaded_at, ai_image_type, ai_room,
                   ai_condition, ai_confidence, ai_issues, ai_description, ai_model,
                   ai_analyzed_at, created_at
            FROM images
            WHERE listing_id = $1
              AND ai_analyzed_at IS NOT NULL
            ORDER BY image_index
            "#,
            listing_id
        )
        .fetch_all(&self.pool)
        .await
    }

    /// Get listings that need second-layer analysis:
    /// 1. All downloaded images are analyzed, OR
    /// 2. No downloaded images exist (all failed/not_found/gone/pending)
    pub async fn get_listings_for_second_layer_analysis(
        &self,
        limit: i64,
    ) -> Result<Vec<i64>, sqlx::Error> {
        let rows = sqlx::query_scalar!(
            r#"
            SELECT DISTINCT l.id
            FROM listings l
            WHERE l.analyzed_at IS NULL
              AND l.is_active = true
              AND (
                  -- Case 1: Has analyzed images and no pending analysis
                  (
                      EXISTS (
                          SELECT 1 FROM images i
                          WHERE i.listing_id = l.id
                            AND i.ai_analyzed_at IS NOT NULL
                      )
                      AND NOT EXISTS (
                          SELECT 1 FROM images i
                          WHERE i.listing_id = l.id
                            AND i.status = 'downloaded'
                            AND i.ai_analyzed_at IS NULL
                      )
                  )
                  OR
                  -- Case 2: No downloaded images at all (treat as "no photos")
                  NOT EXISTS (
                      SELECT 1 FROM images i
                      WHERE i.listing_id = l.id
                        AND i.status = 'downloaded'
                  )
              )
            LIMIT $1
            "#,
            limit
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Save listing-level AI analysis result
    pub async fn save_listing_analysis(
        &self,
        listing_id: i64,
        analysis: &ListingAnalysisResult,
        model: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
            UPDATE listings
            SET analysis_condition = $2,
                analysis_is_shell = $3,
                analysis_move_in_ready = $4,
                analysis_red_flags = $5,
                analysis_summary = $6,
                analysis_model = $7,
                analyzed_at = NOW()
            WHERE id = $1
            "#,
            listing_id,
            analysis.overall_condition,
            analysis.is_shell,
            analysis.move_in_ready,
            &analysis.red_flags,
            &analysis.summary,
            model
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ==================== Stats ====================

    pub async fn get_images_stats(&self) -> Result<ImageStats, sqlx::Error> {
        sqlx::query_as!(
            ImageStats,
            r#"
            SELECT
                COUNT(*) AS "total!",
                COUNT(*) FILTER (WHERE status = 'pending') AS "pending!",
                COUNT(*) FILTER (WHERE status = 'downloaded') AS "downloaded!",
                COUNT(*) FILTER (WHERE status = 'not_found') AS "not_found!",
                COUNT(*) FILTER (WHERE status = 'gone') AS "gone!",
                COUNT(*) FILTER (WHERE status = 'failed') AS "failed!"
            FROM images
            "#
        )
        .fetch_one(&self.pool)
        .await
    }

    pub async fn get_analysis_stats(&self) -> Result<AnalysisStats, sqlx::Error> {
        let images_row = sqlx::query!(r#"
            SELECT
                COUNT(*) FILTER (WHERE ai_analyzed_at IS NOT NULL) AS "analyzed!",
                COUNT(*) FILTER (WHERE status = 'downloaded' AND ai_analyzed_at IS NULL) AS "pending!"
            FROM images
            "#)
        .fetch_one(&self.pool)
        .await?;

        let listings_row = sqlx::query!(
            r#"
            SELECT
                COUNT(*) FILTER (WHERE analyzed_at IS NOT NULL) AS "analyzed!",
                COUNT(*) FILTER (WHERE analyzed_at IS NULL AND is_active = true) AS "pending!"
            FROM listings
            "#
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(AnalysisStats {
            images_analyzed: images_row.analyzed,
            images_pending: images_row.pending,
            listings_analyzed: listings_row.analyzed,
            listings_pending: listings_row.pending,
        })
    }

    pub async fn get_image_count_for_listing(&self, listing_id: i64) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "count!" FROM images WHERE listing_id = $1"#,
            listing_id
        )
        .fetch_one(&self.pool)
        .await
    }

    /// Mark image as failed because it's missing from S3
    pub async fn mark_image_s3_missing(&self, image_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
            UPDATE images
            SET status = 'failed', last_error = 'Not found in S3'
            WHERE id = $1
            "#,
            image_id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get paginated listings with filters
    #[allow(clippy::too_many_arguments)]
    pub async fn get_listings_paginated(
        &self,
        offset: i64,
        limit: i64,
        min_price: Option<i64>,
        max_price: Option<i64>,
        rooms: Option<i32>,
        analyzed_only: bool,
        hide_shell: bool,
        active_only: bool,
        property_type: Option<String>,
        is_shell: Option<bool>,
        min_condition: Option<i32>,
        max_condition: Option<i32>,
    ) -> Result<(Vec<DbListing>, i64), sqlx::Error> {
        let listings = sqlx::query_as!(
            DbListing,
            r#"
            SELECT id, source, url, title, description, price, city, address,
                   lat, lng, rooms, area, floor, floors_total, person_type, params,
                   time_created, time_updated, fetched_at, is_active,
                   analysis_condition, analysis_is_shell, analysis_move_in_ready,
                   analysis_red_flags, analysis_summary, analysis_model,
                   analyzed_at AS analysis_analyzed_at
            FROM listings
            WHERE ($1::bigint IS NULL OR price >= $1)
              AND ($2::bigint IS NULL OR price <= $2)
              AND ($3::int IS NULL OR rooms = $3)
              AND ($4 = false OR analysis_condition IS NOT NULL)
              AND ($5 = false OR analysis_is_shell IS NOT TRUE)
              AND ($6 = false OR is_active = true)
              AND ($7::text IS NULL OR params->>'Вид объекта' = $7)
              AND ($8::bool IS NULL OR analysis_is_shell = $8)
              AND ($9::int IS NULL OR analysis_condition >= $9)
              AND ($10::int IS NULL OR analysis_condition <= $10)
            ORDER BY fetched_at DESC
            LIMIT $11 OFFSET $12
            "#,
            min_price,
            max_price,
            rooms,
            analyzed_only,
            hide_shell,
            active_only,
            property_type.as_deref(),
            is_shell,
            min_condition,
            max_condition,
            limit,
            offset
        )
        .fetch_all(&self.pool)
        .await?;

        let total = sqlx::query_scalar!(
            r#"
            SELECT COUNT(*) AS "count!" FROM listings
            WHERE ($1::bigint IS NULL OR price >= $1)
              AND ($2::bigint IS NULL OR price <= $2)
              AND ($3::int IS NULL OR rooms = $3)
              AND ($4 = false OR analysis_condition IS NOT NULL)
              AND ($5 = false OR analysis_is_shell IS NOT TRUE)
              AND ($6 = false OR is_active = true)
              AND ($7::text IS NULL OR params->>'Вид объекта' = $7)
              AND ($8::bool IS NULL OR analysis_is_shell = $8)
              AND ($9::int IS NULL OR analysis_condition >= $9)
              AND ($10::int IS NULL OR analysis_condition <= $10)
            "#,
            min_price,
            max_price,
            rooms,
            analyzed_only,
            hide_shell,
            active_only,
            property_type.as_deref(),
            is_shell,
            min_condition,
            max_condition
        )
        .fetch_one(&self.pool)
        .await?;

        Ok((listings, total))
    }

    /// Get listing by ID
    pub async fn get_listing_by_id(&self, id: i64) -> Result<Option<DbListing>, sqlx::Error> {
        sqlx::query_as!(
            DbListing,
            r#"
            SELECT id, source, url, title, description, price, city, address,
                   lat, lng, rooms, area, floor, floors_total, person_type, params,
                   time_created, time_updated, fetched_at, is_active,
                   analysis_condition, analysis_is_shell, analysis_move_in_ready,
                   analysis_red_flags, analysis_summary, analysis_model,
                   analyzed_at AS analysis_analyzed_at
            FROM listings WHERE id = $1
            "#,
            id
        )
        .fetch_optional(&self.pool)
        .await
    }

    /// Get images for a listing
    pub async fn get_images_for_listing(
        &self,
        listing_id: i64,
    ) -> Result<Vec<DbImage>, sqlx::Error> {
        sqlx::query_as!(
            DbImage,
            r#"
            SELECT id, listing_id, image_index, source_url,
                   status AS "status: ImageStatus", s3_key, http_status,
                   download_attempts, last_error, downloaded_at, ai_image_type, ai_room,
                   ai_condition, ai_confidence, ai_issues, ai_description, ai_model,
                   ai_analyzed_at, created_at
            FROM images
            WHERE listing_id = $1
            ORDER BY image_index
            "#,
            listing_id
        )
        .fetch_all(&self.pool)
        .await
    }
}

#[derive(Debug, serde::Serialize)]
pub struct ImageStats {
    pub total: i64,
    pub pending: i64,
    pub downloaded: i64,
    pub not_found: i64,
    pub gone: i64,
    pub failed: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct AnalysisStats {
    pub images_analyzed: i64,
    pub images_pending: i64,
    pub listings_analyzed: i64,
    pub listings_pending: i64,
}
