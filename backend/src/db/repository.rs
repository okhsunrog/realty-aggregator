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
        self.ingest_listing(listing, None).await
    }

    pub async fn upsert_listing_with_images(
        &self,
        listing: &DbListing,
        urls: &[String],
    ) -> Result<(), sqlx::Error> {
        self.ingest_listing(listing, Some(urls)).await?;
        Ok(())
    }

    async fn ingest_listing(
        &self,
        l: &DbListing,
        photos: Option<&[String]>,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let previous = sqlx::query!(r#"
            SELECT price, (title, description, price, area, rooms, floor, floors_total, address, params, lat, lng)
                IS DISTINCT FROM ($2::text, $3::text, $4::bigint, $5::float8, $6::int,
                    $7::int, $8::int, $9::text, $10::jsonb, $11::float8, $12::float8) AS "changed!"
            FROM listings WHERE id = $1 FOR UPDATE
            "#, l.id, l.title.as_deref(), l.description.as_deref(), l.price, l.area, l.rooms,
            l.floor, l.floors_total, l.address.as_deref(), l.params.as_ref(), l.lat, l.lng)
            .fetch_optional(&mut *tx).await?;
        let accepted = sqlx::query!(r#"
            INSERT INTO listings (id, source, url, title, description, price, city, address,
                lat, lng, rooms, area, floor, floors_total, person_type, params,
                time_created, time_updated, fetched_at, is_active, source_time, feed_is_actual, feed_observed_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23)
            ON CONFLICT (id) DO UPDATE SET source = EXCLUDED.source, url = EXCLUDED.url,
                title = EXCLUDED.title, description = EXCLUDED.description, price = EXCLUDED.price,
                city = EXCLUDED.city, address = EXCLUDED.address, lat = EXCLUDED.lat, lng = EXCLUDED.lng,
                rooms = EXCLUDED.rooms, area = EXCLUDED.area, floor = EXCLUDED.floor,
                floors_total = EXCLUDED.floors_total, person_type = EXCLUDED.person_type,
                params = EXCLUDED.params, time_created = EXCLUDED.time_created,
                time_updated = EXCLUDED.time_updated, fetched_at = EXCLUDED.fetched_at,
                source_time = EXCLUDED.source_time,
                feed_is_actual = EXCLUDED.feed_is_actual, feed_observed_at = EXCLUDED.feed_observed_at,
                is_active = CASE WHEN listings.actuality_checked_at IS NOT NULL AND
                    (EXCLUDED.source_time IS NULL OR listings.actuality_checked_at >= EXCLUDED.source_time)
                    THEN listings.is_active ELSE EXCLUDED.is_active END
            WHERE listings.source_time IS NULL OR EXCLUDED.source_time >= listings.source_time
            "#, l.id, &l.source, &l.url, l.title.as_deref(), l.description.as_deref(), l.price,
            &l.city, l.address.as_deref(), l.lat, l.lng, l.rooms, l.area, l.floor, l.floors_total,
            l.person_type, l.params.as_ref(), l.time_created, l.time_updated, l.fetched_at,
            l.is_active, l.source_time, l.feed_is_actual, l.feed_observed_at).execute(&mut *tx).await?.rows_affected() > 0;
        if !accepted {
            tx.commit().await?;
            return Ok(false);
        }
        let mut changed = previous.as_ref().is_none_or(|p| p.changed);
        if let Some(price) = l.price
            && previous.as_ref().is_none_or(|p| p.price != Some(price))
        {
            sqlx::query!(
                r#"INSERT INTO listing_price_history (listing_id, observed_at, price)
                    VALUES ($1,$2,$3) ON CONFLICT DO NOTHING"#,
                l.id,
                l.source_time.unwrap_or(l.fetched_at),
                price
            )
            .execute(&mut *tx)
            .await?;
        }
        if let Some(urls) = photos {
            let old = sqlx::query_scalar!(
                "SELECT source_url FROM images WHERE listing_id = $1 ORDER BY image_index",
                l.id
            )
            .fetch_all(&mut *tx)
            .await?;
            changed |= old != urls;
            sqlx::query!(
                "DELETE FROM images WHERE listing_id = $1 AND image_index >= $2",
                l.id,
                urls.len() as i32
            )
            .execute(&mut *tx)
            .await?;
            for (index, url) in urls.iter().enumerate() {
                sqlx::query!(r#"
                    INSERT INTO images (listing_id, image_index, source_url) VALUES ($1,$2,$3)
                    ON CONFLICT (listing_id, image_index) DO UPDATE SET source_url = EXCLUDED.source_url,
                        status = 'pending', s3_key = NULL, downloaded_at = NULL, http_status = NULL,
                        download_attempts = 0, last_error = NULL, ai_image_type = NULL, ai_room = NULL,
                        ai_condition = NULL, ai_confidence = NULL, ai_issues = NULL, ai_description = NULL,
                        ai_model = NULL, ai_analyzed_at = NULL, ai_attempts = 0, ai_retry_at = NULL, ai_last_error = NULL
                    WHERE images.source_url IS DISTINCT FROM EXCLUDED.source_url
                    "#, l.id, index as i32, url).execute(&mut *tx).await?;
            }
        }
        if changed {
            sqlx::query!(
                r#"UPDATE listings SET analysis_condition = NULL, analysis_is_shell = NULL,
                analysis_move_in_ready = NULL, analysis_red_flags = NULL, analysis_summary = NULL,
                analysis_model = NULL, analyzed_at = NULL, analysis_attempts = 0,
                analysis_retry_at = NULL, analysis_last_error = NULL WHERE id = $1"#,
                l.id
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn mark_listing_inactive(&self, id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query!("UPDATE listings SET is_active = false, actuality_checked_at = NOW(), actuality_origin = 'manual' WHERE id = $1", id)
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
            "SELECT id, last_listing_id, last_collection_at, query_fingerprint, last_sync_at FROM collection_state WHERE id = 1"
        )
        .fetch_one(&self.pool)
        .await
    }

    pub async fn update_collection_checkpoint(
        &self,
        through: chrono::DateTime<chrono::Utc>,
        fingerprint: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!("UPDATE collection_state SET last_collection_at = $1, query_fingerprint = $2 WHERE id = 1", through, fingerprint)
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn get_images_for_download(
        &self,
        limit: i64,
    ) -> Result<Vec<ImageForDownload>, sqlx::Error> {
        self.get_images_for_download_batch(limit, i32::MAX).await
    }
    pub async fn get_images_for_download_batch(
        &self,
        limit: i64,
        max_photos: i32,
    ) -> Result<Vec<ImageForDownload>, sqlx::Error> {
        sqlx::query_as!(
            ImageForDownload,
            r#"
            SELECT id, listing_id, image_index, source_url,
                   status AS "status: ImageStatus", download_attempts
            FROM images
            WHERE (status = 'pending' OR (status = 'not_found' AND download_attempts < 2))
                AND image_index < $2 AND EXISTS (SELECT 1 FROM listings l WHERE l.id = images.listing_id AND l.is_active = true)
            ORDER BY image_index, (SELECT l.fetched_at FROM listings l WHERE l.id = images.listing_id) DESC, listing_id
            LIMIT $1
            "#,
            limit, max_photos
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
        max_attempts: i32,
        max_photos: i32,
        listing_id: Option<i64>,
    ) -> Result<Vec<ImageForAnalysis>, sqlx::Error> {
        sqlx::query_as!(
            ImageForAnalysis,
            r#"
            SELECT id, listing_id, image_index, source_url, s3_key
            FROM images
            WHERE status = 'downloaded'
              AND ai_analyzed_at IS NULL AND ai_attempts < $2
              AND (ai_retry_at IS NULL OR ai_retry_at <= NOW()) AND image_index < $3
              AND ($4::bigint IS NULL OR listing_id = $4)
              AND EXISTS (SELECT 1 FROM listings l WHERE l.id = images.listing_id AND l.is_active = true)
            ORDER BY listing_id, image_index
            LIMIT $1
            "#,
            limit, max_attempts, max_photos, listing_id
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
        let mut tx = self.pool.begin().await?;
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
                ai_analyzed_at = NOW(), ai_retry_at = NULL, ai_last_error = NULL
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
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            r#"UPDATE listings SET analysis_condition = NULL, analysis_is_shell = NULL,
            analysis_move_in_ready = NULL, analysis_red_flags = NULL, analysis_summary = NULL,
            analysis_model = NULL, analyzed_at = NULL, analysis_attempts = 0,
            analysis_retry_at = NULL, analysis_last_error = NULL
            WHERE id = (SELECT listing_id FROM images WHERE id = $1)"#,
            image_id
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
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
                   ai_analyzed_at, ai_attempts, ai_last_error, created_at
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

    pub async fn get_listings_for_second_layer_analysis(
        &self,
        limit: i64,
        max_attempts: i32,
        max_photos: i32,
    ) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar!(r#"
            SELECT l.id FROM listings l WHERE l.analyzed_at IS NULL AND l.is_active = true
                AND l.analysis_attempts < $2 AND (l.analysis_retry_at IS NULL OR l.analysis_retry_at <= NOW())
                AND NOT EXISTS (SELECT 1 FROM images i WHERE i.listing_id = l.id AND i.image_index < $3 AND (
                    i.status = 'pending' OR (i.status = 'not_found' AND i.download_attempts < 2)
                    OR (i.status = 'downloaded' AND i.ai_analyzed_at IS NULL AND i.ai_attempts < $2)))
            ORDER BY l.id LIMIT $1
            "#, limit, max_attempts, max_photos).fetch_all(&self.pool).await
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
                analyzed_at = NOW(), analysis_retry_at = NULL, analysis_last_error = NULL
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
    pub async fn get_listings_paginated(
        &self,
        offset: i64,
        limit: i64,
        filters: ListingsFilter,
    ) -> Result<(Vec<DbListing>, i64), sqlx::Error> {
        let ListingsFilter {
            min_price,
            max_price,
            rooms,
            analyzed_only,
            hide_shell,
            active_only,
            property_type,
            is_shell,
            min_condition,
            max_condition,
            min_area,
            move_in_ready,
            favorites_only,
            hide_rejected,
            verified_after,
        } = filters;
        let listings = sqlx::query_as!(
            DbListing,
            r#"
            SELECT id, source, url, title, description, price, city, address,
                   lat, lng, rooms, area, floor, floors_total, person_type, params,
                   time_created, time_updated, fetched_at, is_active,
                   analysis_condition, analysis_is_shell, analysis_move_in_ready,
                   analysis_red_flags, analysis_summary, analysis_model,
                   analyzed_at AS analysis_analyzed_at, source_time, analysis_attempts, analysis_last_error,
                   actuality_checked_at, actuality_origin, source_check_status, source_checked_at, source_check_http_status, source_check_reason,
                   feed_is_actual, feed_observed_at, source_confirmed_status, source_confirmed_at
            FROM listings
            WHERE ($1::bigint IS NULL OR price >= $1)
              AND ($2::bigint IS NULL OR price <= $2)
              AND ($3::int IS NULL OR rooms = $3)
              AND ($4 = false OR analyzed_at IS NOT NULL)
              AND ($5 = false OR analysis_is_shell IS NOT TRUE)
              AND ($6 = false OR is_active = true)
              AND ($7::text IS NULL OR params->>'Вид объекта' = $7)
              AND ($8::bool IS NULL OR analysis_is_shell = $8)
              AND ($9::int IS NULL OR analysis_condition >= $9)
              AND ($10::int IS NULL OR analysis_condition <= $10)
              AND ($13::float8 IS NULL OR area >= $13)
              AND ($14::bool IS NULL OR analysis_move_in_ready = $14)
              AND ($15 = false OR EXISTS (SELECT 1 FROM listing_reviews r WHERE r.listing_id = listings.id AND r.favorite))
              AND ($16 = false OR NOT EXISTS (SELECT 1 FROM listing_reviews r WHERE r.listing_id = listings.id AND r.hidden))

              AND ($17::timestamptz IS NULL OR (is_active = true AND actuality_checked_at >= $17 AND actuality_origin IN ('source_page', 'check_feed', 'manual') AND (source_time IS NULL OR actuality_checked_at >= source_time)))
            ORDER BY fetched_at DESC, id DESC
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
            offset, min_area, move_in_ready, favorites_only, hide_rejected, verified_after
        )
        .fetch_all(&self.pool)
        .await?;

        let total = sqlx::query_scalar!(
            r#"
            SELECT COUNT(*) AS "count!" FROM listings
            WHERE ($1::bigint IS NULL OR price >= $1)
              AND ($2::bigint IS NULL OR price <= $2)
              AND ($3::int IS NULL OR rooms = $3)
              AND ($4 = false OR analyzed_at IS NOT NULL)
              AND ($5 = false OR analysis_is_shell IS NOT TRUE)
              AND ($6 = false OR is_active = true)
              AND ($7::text IS NULL OR params->>'Вид объекта' = $7)
              AND ($8::bool IS NULL OR analysis_is_shell = $8)
              AND ($9::int IS NULL OR analysis_condition >= $9)
              AND ($10::int IS NULL OR analysis_condition <= $10)
              AND ($11::float8 IS NULL OR area >= $11)
              AND ($12::bool IS NULL OR analysis_move_in_ready = $12)
              AND ($13 = false OR EXISTS (SELECT 1 FROM listing_reviews r WHERE r.listing_id = listings.id AND r.favorite))
              AND ($14 = false OR NOT EXISTS (SELECT 1 FROM listing_reviews r WHERE r.listing_id = listings.id AND r.hidden))

              AND ($15::timestamptz IS NULL OR (is_active = true AND actuality_checked_at >= $15 AND actuality_origin IN ('source_page', 'check_feed', 'manual') AND (source_time IS NULL OR actuality_checked_at >= source_time)))
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
            max_condition, min_area, move_in_ready, favorites_only, hide_rejected, verified_after
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
                   analyzed_at AS analysis_analyzed_at, source_time, analysis_attempts, analysis_last_error,
                   actuality_checked_at, actuality_origin, source_check_status, source_checked_at, source_check_http_status, source_check_reason,
                   feed_is_actual, feed_observed_at, source_confirmed_status, source_confirmed_at
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
                   ai_analyzed_at, ai_attempts, ai_last_error, created_at
            FROM images
            WHERE listing_id = $1
            ORDER BY image_index
            "#,
            listing_id
        )
        .fetch_all(&self.pool)
        .await
    }
    pub async fn begin_image_attempt(&self, id: i64) -> Result<(), sqlx::Error> {
        sqlx::query!("UPDATE images SET ai_attempts = ai_attempts + 1, ai_retry_at = NOW() + INTERVAL '5 minutes' WHERE id = $1", id)
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn fail_image_analysis(&self, id: i64, error: &str) -> Result<(), sqlx::Error> {
        sqlx::query!("UPDATE images SET ai_last_error = $2, ai_retry_at = NOW() + INTERVAL '1 minute' * power(2, LEAST(ai_attempts, 6)) WHERE id = $1", id, error)
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn begin_listing_attempt(&self, id: i64) -> Result<(), sqlx::Error> {
        sqlx::query!("UPDATE listings SET analysis_attempts = analysis_attempts + 1, analysis_retry_at = NOW() + INTERVAL '5 minutes' WHERE id = $1", id)
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn fail_listing_analysis(&self, id: i64, error: &str) -> Result<(), sqlx::Error> {
        sqlx::query!("UPDATE listings SET analysis_last_error = $2, analysis_retry_at = NOW() + INTERVAL '1 minute' * power(2, LEAST(analysis_attempts, 6)) WHERE id = $1", id, error)
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn reset_analysis(&self, listing_id: i64, images: bool) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query!("UPDATE listings SET analysis_condition = NULL, analysis_is_shell = NULL, analysis_move_in_ready = NULL, analysis_red_flags = NULL, analysis_summary = NULL, analysis_model = NULL, analyzed_at = NULL, analysis_attempts = 0, analysis_retry_at = NULL, analysis_last_error = NULL WHERE id = $1", listing_id)
            .execute(&mut *tx).await?;
        if images {
            sqlx::query!("UPDATE images SET ai_image_type = NULL, ai_room = NULL, ai_condition = NULL, ai_confidence = NULL, ai_issues = NULL, ai_description = NULL, ai_model = NULL, ai_analyzed_at = NULL, ai_attempts = 0, ai_retry_at = NULL, ai_last_error = NULL WHERE listing_id = $1", listing_id)
                .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn apply_actuality(
        &self,
        id: i64,
        active: bool,
        checked_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, sqlx::Error> {
        Ok(sqlx::query!(r#"UPDATE listings SET is_active = $2, actuality_checked_at = $3, actuality_origin = 'check_feed'
            WHERE id = $1 AND (actuality_checked_at IS NULL OR actuality_checked_at <= $3) AND (source_time IS NULL OR source_time <= $3)"#, id, active, checked_at)
            .execute(&self.pool).await?.rows_affected() > 0)
    }

    pub async fn update_sync_checkpoint(
        &self,
        through: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "UPDATE collection_state SET last_sync_at = $1 WHERE id = 1",
            through
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn save_source_check(
        &self,
        id: i64,
        check: &crate::actuality::SourceCheck,
    ) -> Result<(), sqlx::Error> {
        let definitive = check.status != crate::actuality::SourceStatus::Unknown;
        let active = check.status == crate::actuality::SourceStatus::Active;
        sqlx::query!(r#"UPDATE listings SET source_check_status = $2, source_checked_at = $3,
            source_check_http_status = $4, source_check_reason = $5,
            source_confirmed_status = CASE WHEN $6 THEN $2 ELSE source_confirmed_status END,
            source_confirmed_at = CASE WHEN $6 THEN $3 ELSE source_confirmed_at END,
            is_active = CASE WHEN $6 AND (actuality_checked_at IS NULL OR actuality_checked_at <= $3) THEN $7 ELSE is_active END,
            actuality_checked_at = CASE WHEN $6 AND (actuality_checked_at IS NULL OR actuality_checked_at <= $3) THEN $3 ELSE actuality_checked_at END,
            actuality_origin = CASE WHEN $6 AND (actuality_checked_at IS NULL OR actuality_checked_at <= $3) THEN 'source_page' ELSE actuality_origin END
            WHERE id = $1 AND (source_time IS NULL OR source_time <= $3)
                AND (source_checked_at IS NULL OR source_checked_at <= $3)"#,
            id, check.status.as_str(), check.checked_at, check.http_status, &check.reason, definitive, active)
            .execute(&self.pool).await?;
        Ok(())
    }
    pub async fn get_source_check_queue(
        &self,
        source: &str,
        limit: i64,
        recheck_hours: i32,
    ) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar!(
            r#"SELECT id FROM listings WHERE source = $3 AND
            (is_active = true OR EXISTS (SELECT 1 FROM listing_reviews r WHERE r.listing_id = listings.id AND r.favorite))
            AND (source_checked_at IS NULL OR source_checked_at < NOW() - INTERVAL '1 hour' * $2::int
                OR source_time > source_checked_at)
            ORDER BY EXISTS (SELECT 1 FROM listing_reviews r WHERE r.listing_id = listings.id AND r.favorite) DESC,
                source_checked_at NULLS FIRST, source_time DESC NULLS LAST, id DESC LIMIT $1"#,
            limit,
            recheck_hours,
            source
        )
        .fetch_all(&self.pool)
        .await
    }

    pub async fn source_ready(&self, source: &str) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar!(
            r#"SELECT NOT EXISTS (SELECT 1 FROM source_check_hosts
            WHERE source = $1 AND retry_after > NOW()) AS "ready!""#,
            source
        )
        .fetch_one(&self.pool)
        .await
    }

    pub async fn record_source_attempt(
        &self,
        source: &str,
        check: &crate::actuality::SourceCheck,
    ) -> Result<(), sqlx::Error> {
        let delay: i32 = match check.failure_kind {
            Some("blocked") => 21600,
            Some(_) => 900,
            None => 0,
        };
        sqlx::query!(r#"INSERT INTO source_check_hosts (source, failures, retry_after, last_error, checked_at)
            VALUES ($1, CASE WHEN $3 > 0 THEN 1 ELSE 0 END,
                CASE WHEN $3 > 0 THEN $2::timestamptz + $3 * INTERVAL '1 second' ELSE NULL END,
                CASE WHEN $3 > 0 THEN $4::text ELSE NULL END, $2)
            ON CONFLICT (source) DO UPDATE SET
                failures = CASE WHEN $3 > 0 THEN LEAST(source_check_hosts.failures + 1, 10) ELSE 0 END,
                retry_after = CASE WHEN $3 > 0 THEN $2::timestamptz + LEAST(86400, $3 * power(2, LEAST(source_check_hosts.failures, 5))) * INTERVAL '1 second' ELSE NULL END,
                last_error = CASE WHEN $3 > 0 THEN $4 ELSE NULL END, checked_at = $2"#,
            source, check.checked_at, delay, &check.reason).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn feed_sync_ids(&self, pending: bool) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar!(
            "SELECT id FROM listings WHERE (feed_history_synced_at IS NULL) = $1 ORDER BY id",
            pending
        )
        .fetch_all(&self.pool)
        .await
    }

    pub async fn recent_reparse_request(&self, id: i64) -> Result<Option<bool>, sqlx::Error> {
        sqlx::query_scalar!("SELECT queued FROM listing_reparse_requests WHERE listing_id = $1 AND requested_at > NOW() - INTERVAL '1 day'", id)
            .fetch_optional(&self.pool).await
    }

    pub async fn save_reparse_request(&self, id: i64, queued: bool) -> Result<(), sqlx::Error> {
        sqlx::query!("INSERT INTO listing_reparse_requests(listing_id,requested_at,queued) VALUES ($1,NOW(),$2) ON CONFLICT(listing_id) DO UPDATE SET requested_at = NOW(), queued = $2", id, queued)
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn mark_feed_history_synced(
        &self,
        ids: &[i64],
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "UPDATE listings SET feed_history_synced_at = $2 WHERE id = ANY($1)",
            ids,
            at
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_availability_stats(
        &self,
        cutoff: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<AvailabilityStats>, sqlx::Error> {
        sqlx::query_as!(AvailabilityStats, r#"SELECT l.source,
            COUNT(*) AS "total!",
            COUNT(*) FILTER (WHERE l.is_active = false) AS "removed!",
            COUNT(*) FILTER (WHERE l.is_active = true AND l.actuality_checked_at >= $1
                AND l.actuality_origin IN ('source_page', 'check_feed', 'manual')
                AND (l.source_time IS NULL OR l.actuality_checked_at >= l.source_time)) AS "confirmed!",
            COUNT(*) FILTER (WHERE l.source_checked_at IS NULL) AS "unchecked!",
            MAX(h.retry_after) AS retry_after, MAX(h.last_error) AS last_error
            FROM listings l LEFT JOIN source_check_hosts h ON h.source = l.source
            GROUP BY l.source ORDER BY l.source"#, cutoff).fetch_all(&self.pool).await
    }

    pub async fn get_listing_ids(&self) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar!("SELECT id FROM listings ORDER BY id")
            .fetch_all(&self.pool)
            .await
    }

    pub async fn get_image_by_id(&self, id: i64) -> Result<Option<String>, sqlx::Error> {
        Ok(sqlx::query_scalar!(
            "SELECT s3_key FROM images WHERE id = $1 AND status = 'downloaded'",
            id
        )
        .fetch_optional(&self.pool)
        .await?
        .flatten())
    }

    pub async fn get_review(&self, id: i64) -> Result<super::models::ListingReview, sqlx::Error> {
        Ok(sqlx::query_as!(
            super::models::ListingReview,
            "SELECT favorite, hidden, stage, notes FROM listing_reviews WHERE listing_id = $1",
            id
        )
        .fetch_optional(&self.pool)
        .await?
        .unwrap_or_else(|| super::models::ListingReview {
            stage: "new".to_owned(),
            ..Default::default()
        }))
    }

    pub async fn save_review(
        &self,
        id: i64,
        review: &super::models::ListingReview,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(r#"INSERT INTO listing_reviews (listing_id, favorite, hidden, stage, notes) VALUES ($1,$2,$3,$4,$5)
            ON CONFLICT (listing_id) DO UPDATE SET favorite = EXCLUDED.favorite, hidden = EXCLUDED.hidden,
                stage = EXCLUDED.stage, notes = EXCLUDED.notes"#, id, review.favorite, review.hidden, &review.stage, &review.notes)
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn get_price_history(&self, id: i64) -> Result<Vec<serde_json::Value>, sqlx::Error> {
        let rows = sqlx::query!("SELECT observed_at, price FROM listing_price_history WHERE listing_id = $1 ORDER BY observed_at DESC", id)
            .fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(|row| serde_json::json!({"observed_at": row.observed_at, "price": row.price}))
            .collect())
    }
}

#[derive(Debug, serde::Serialize)]
pub struct AvailabilityStats {
    pub source: String,
    pub total: i64,
    pub removed: i64,
    pub confirmed: i64,
    pub unchecked: i64,
    pub retry_after: Option<chrono::DateTime<chrono::Utc>>,
    pub last_error: Option<String>,
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

#[derive(Debug, Default)]
pub struct ListingsFilter {
    pub min_price: Option<i64>,
    pub max_price: Option<i64>,
    pub rooms: Option<i32>,
    pub analyzed_only: bool,
    pub hide_shell: bool,
    pub active_only: bool,
    pub property_type: Option<String>,
    pub is_shell: Option<bool>,
    pub min_condition: Option<i32>,
    pub max_condition: Option<i32>,
    pub min_area: Option<f64>,
    pub move_in_ready: Option<bool>,
    pub favorites_only: bool,
    pub hide_rejected: bool,
    pub verified_after: Option<chrono::DateTime<chrono::Utc>>,
}
