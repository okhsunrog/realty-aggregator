use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use futures::{stream, StreamExt};
use serde::Serialize;
use tokio::sync::{RwLock, broadcast};
use tracing::{debug, error, info, warn};

use crate::ads_api::AdsApiClient;
use crate::ads_api::models::ListingsQuery;
use crate::config::Config;
use crate::db::{DbListing, ImageStatus, Repository};
use crate::storage::StorageClient;
use crate::vision::{ImageAnalyzer, ListingAnalyzer};

/// Текущая фаза работы
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum JobPhase {
    Idle,
    CollectingListings,
    DownloadingImages,
    AnalyzingImages,
    AnalyzingListings,
    SyncingActuality,
}

/// Прогресс текущей задачи
#[derive(Debug, Clone, Serialize)]
pub struct JobProgress {
    pub processed: usize,
    pub total: Option<usize>,
}

/// Полный статус работы бэкенда
#[derive(Debug, Clone, Serialize)]
pub struct JobStatus {
    pub phase: JobPhase,
    pub progress: Option<JobProgress>,
    pub started_at: Option<DateTime<Utc>>,
    pub message: Option<String>,
}

impl Default for JobStatus {
    fn default() -> Self {
        Self {
            phase: JobPhase::Idle,
            progress: None,
            started_at: None,
            message: None,
        }
    }
}

pub struct Collector {
    api_client: Arc<AdsApiClient>,
    repository: Arc<Repository>,
    image_analyzer: Option<Arc<ImageAnalyzer>>,
    listing_analyzer: Option<Arc<ListingAnalyzer>>,
    storage_client: Option<Arc<StorageClient>>,
    config: Arc<Config>,
    status: Arc<RwLock<JobStatus>>,
    status_tx: broadcast::Sender<JobStatus>,
    vision_api_key: Option<String>,
}

impl Collector {
    pub fn new(
        api_client: Arc<AdsApiClient>,
        repository: Arc<Repository>,
        config: Arc<Config>,
    ) -> Self {
        let (status_tx, _) = broadcast::channel(16);
        Self {
            api_client,
            repository,
            image_analyzer: None,
            listing_analyzer: None,
            storage_client: None,
            config,
            status: Arc::new(RwLock::new(JobStatus::default())),
            status_tx,
            vision_api_key: None,
        }
    }

    /// Subscribe to status updates via broadcast channel
    pub fn subscribe(&self) -> broadcast::Receiver<JobStatus> {
        self.status_tx.subscribe()
    }

    pub fn with_image_analyzer(mut self, analyzer: Arc<ImageAnalyzer>) -> Self {
        self.image_analyzer = Some(analyzer);
        self
    }

    pub fn with_listing_analyzer(mut self, analyzer: Arc<ListingAnalyzer>) -> Self {
        self.listing_analyzer = Some(analyzer);
        self
    }

    pub fn with_storage_client(mut self, storage: Arc<StorageClient>) -> Self {
        self.storage_client = Some(storage);
        self
    }

    pub fn with_vision_api_key(mut self, api_key: String) -> Self {
        self.vision_api_key = Some(api_key);
        self
    }

    pub async fn is_running(&self) -> bool {
        self.status.read().await.phase != JobPhase::Idle
    }

    pub async fn get_status(&self) -> JobStatus {
        self.status.read().await.clone()
    }

    async fn set_status(&self, phase: JobPhase, message: Option<&str>) {
        let status_clone = {
            let mut status = self.status.write().await;
            if phase == JobPhase::Idle {
                *status = JobStatus::default();
            } else {
                status.phase = phase;
                status.message = message.map(|s| s.to_string());
                if status.started_at.is_none() {
                    status.started_at = Some(Utc::now());
                }
            }
            status.clone()
        };
        // Broadcast to WebSocket subscribers (ignore errors if no subscribers)
        let _ = self.status_tx.send(status_clone);
    }

    async fn set_progress(&self, processed: usize, total: Option<usize>) {
        let status_clone = {
            let mut status = self.status.write().await;
            status.progress = Some(JobProgress { processed, total });
            status.clone()
        };
        // Broadcast to WebSocket subscribers
        let _ = self.status_tx.send(status_clone);
    }

    pub async fn run_collection(&self) -> Result<CollectionResult, String> {
        {
            let status = self.status.read().await;
            if status.phase != JobPhase::Idle {
                return Err("Job already in progress".to_string());
            }
        }

        self.set_status(JobPhase::CollectingListings, Some("Запуск сбора объявлений")).await;

        let result = self.do_collection().await;

        self.set_status(JobPhase::Idle, None).await;

        result
    }

    async fn do_collection(&self) -> Result<CollectionResult, String> {
        info!("Starting collection job");

        let query_config = &self.config.api.query;
        let request_delay =
            std::time::Duration::from_secs(self.config.api.request_delay_seconds as u64);
        let mut total_fetched = 0;
        let mut total_saved = 0;
        let mut errors = Vec::new();

        // Get last collection time for date-based loading
        let last_collection_at = self
            .repository
            .get_collection_state()
            .await
            .ok()
            .and_then(|s| s.last_collection_at);

        // Moscow time is UTC+3, API time is 30 min behind
        let now_moscow = Utc::now() + Duration::hours(3) - Duration::minutes(30);

        // Determine date range
        let (date1, is_initial) = match last_collection_at {
            Some(last) => {
                // Incremental: from last collection (with 1 hour overlap for safety)
                let from = last + Duration::hours(3) - Duration::minutes(30) - Duration::hours(1);
                (from, false)
            }
            None => {
                // Initial load: last N days from config
                let days = self.config.api.initial_load_days as i64;
                let from = now_moscow - Duration::days(days);
                (from, true)
            }
        };

        let date1_str = date1.format("%Y-%m-%d %H:%M:%S").to_string();
        let date2_str = now_moscow.format("%Y-%m-%d %H:%M:%S").to_string();

        info!(
            "Collecting listings for city: {}, category: {}, date range: {} to {} (initial: {})",
            query_config.city, query_config.category_id, date1_str, date2_str, is_initial
        );

        // For date-based collection, we paginate by shrinking the time window
        let mut current_date2 = now_moscow;

        loop {
            let batch_date2_str = current_date2.format("%Y-%m-%d %H:%M:%S").to_string();

            match self.collect_by_date(&date1_str, &batch_date2_str).await {
                Ok((fetched, saved, oldest_time)) => {
                    total_fetched += fetched;
                    total_saved += saved;
                    info!(
                        "Batch: fetched {}, saved {} (total: {}/{})",
                        fetched, saved, total_fetched, total_saved
                    );

                    if fetched < 1000 {
                        // Got all data in this range
                        info!("Fetched all available listings in date range");
                        break;
                    }

                    // If we got 1000, there might be more - shrink the window
                    if let Some(oldest) = oldest_time {
                        // Parse the oldest time and use it as new date2
                        if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(&oldest, "%Y-%m-%d %H:%M:%S") {
                            current_date2 = chrono::TimeZone::from_utc_datetime(&Utc, &parsed);
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }

                    tokio::time::sleep(request_delay).await;
                }
                Err(e) => {
                    let error_msg = format!("Error collecting: {}", e);
                    error!("{}", error_msg);
                    errors.push(error_msg);
                    break;
                }
            }
        }

        // Update collection state with current time
        if let Err(e) = self.repository.update_collection_time().await {
            warn!("Failed to update collection state: {}", e);
        }

        info!(
            "Collection completed. Total fetched: {}, saved: {}",
            total_fetched, total_saved
        );

        // Download images for all pending images
        let mut total_images_downloaded = 0;
        if let Some(storage) = &self.storage_client {
            // Get all pending images
            let images = self.repository
                .get_images_for_download(10000)
                .await
                .unwrap_or_default();

            let total_pending = images.len();
            if total_pending > 0 {
                self.set_status(JobPhase::DownloadingImages, Some("Загрузка изображений")).await;
                self.set_progress(0, Some(total_pending)).await;

                let mut processed = 0;
                for image in images {
                    let result = storage
                        .download_and_store_single(image.listing_id, image.image_index, &image.source_url)
                        .await;

                    let status = if result.success {
                        total_images_downloaded += 1;
                        ImageStatus::Downloaded
                    } else {
                        match result.http_status {
                            Some(404) => ImageStatus::NotFound,
                            Some(410) => ImageStatus::Gone,
                            _ => {
                                if image.download_attempts >= 2 {
                                    ImageStatus::Failed
                                } else {
                                    ImageStatus::Pending
                                }
                            }
                        }
                    };

                    if let Err(e) = self.repository
                        .update_image_download(
                            image.id,
                            status,
                            result.s3_key.as_deref(),
                            result.http_status.map(|s| s as i32),
                            result.error.as_deref(),
                        )
                        .await
                    {
                        warn!("Failed to update image {}: {}", image.id, e);
                    }

                    processed += 1;
                    self.set_progress(processed, Some(total_pending)).await;
                }
                info!("All images downloaded: {}/{}", total_images_downloaded, total_pending);
            }
        }

        // Run per-image AI analysis (if auto_analyze enabled)
        let mut total_images_analyzed = 0;
        if let (Some(analyzer), Some(storage)) = (&self.image_analyzer, &self.storage_client) {
            if self.config.vision.auto_analyze {
                // Get all pending images
                let images = self.repository
                    .get_images_for_ai_analysis(10000)
                    .await
                    .unwrap_or_default();

                let total_pending = images.len();
                if total_pending > 0 {
                    self.set_status(JobPhase::AnalyzingImages, Some("AI анализ изображений")).await;
                    self.set_progress(0, Some(total_pending)).await;

                    for image in images {
                        let s3_key = match &image.s3_key {
                            Some(k) => k,
                            None => continue,
                        };

                        let image_bytes = match storage.get_image_by_key(s3_key).await {
                            Ok(b) => b,
                            Err(e) => {
                                warn!("Failed to get image {} from S3: {}", image.id, e);
                                let _ = self.repository.mark_image_s3_missing(image.id).await;
                                continue;
                            }
                        };

                        let image_base64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

                        match analyzer.analyze_base64(&image_base64).await {
                            Ok(analysis) => {
                                if let Err(e) = self.repository
                                    .save_image_analysis(image.id, &analysis, &self.config.vision.image_model)
                                    .await
                                {
                                    warn!("Failed to save analysis for image {}: {}", image.id, e);
                                } else {
                                    total_images_analyzed += 1;
                                    debug!("Analyzed image {}: type={}, condition={:?}", image.id, analysis.image_type, analysis.condition);
                                }
                            }
                            Err(e) => {
                                warn!("Failed to analyze image {}: {}", image.id, e);
                            }
                        }

                        self.set_progress(total_images_analyzed, Some(total_pending)).await;
                    }
                    info!("Image analysis completed: {}/{}", total_images_analyzed, total_pending);
                }
            }
        }

        // Run listing-level analysis (if auto_analyze enabled)
        let mut total_listings_analyzed = 0;
        if let Some(analyzer) = &self.listing_analyzer {
            if self.config.vision.auto_analyze {
                // Get all pending listing IDs
                let listing_ids = self.repository
                    .get_listings_for_second_layer_analysis(10000)
                    .await
                    .unwrap_or_default();

                let total_pending = listing_ids.len();
                if total_pending > 0 {
                    self.set_status(JobPhase::AnalyzingListings, Some("AI анализ объявлений")).await;
                    self.set_progress(0, Some(total_pending)).await;

                    for listing_id in listing_ids {
                        // Get listing details
                        let listing = match self.repository.get_listing_for_analysis(listing_id).await {
                            Ok(Some(l)) => l,
                            _ => continue,
                        };

                        // Get image analyses for this listing
                        let image_analyses = self.repository
                            .get_listing_image_analyses(listing_id)
                            .await
                            .unwrap_or_default();

                        if image_analyses.is_empty() {
                            continue;
                        }

                        match analyzer.analyze(&listing, &image_analyses).await {
                            Ok(analysis) => {
                                if let Err(e) = self.repository
                                    .save_listing_analysis(listing_id, &analysis, analyzer.model())
                                    .await
                                {
                                    warn!("Failed to save listing analysis {}: {}", listing_id, e);
                                } else {
                                    total_listings_analyzed += 1;
                                    debug!("Analyzed listing {}: condition={}, shell={}", listing_id, analysis.overall_condition, analysis.is_shell);
                                }
                            }
                            Err(e) => {
                                warn!("Failed to analyze listing {}: {}", listing_id, e);
                            }
                        }

                        self.set_progress(total_listings_analyzed, Some(total_pending)).await;
                    }
                    info!("Listing analysis completed: {}/{}", total_listings_analyzed, total_pending);
                }
            }
        }

        Ok(CollectionResult {
            total_fetched,
            total_saved,
            total_images_downloaded,
            total_images_analyzed,
            total_listings_analyzed,
            errors,
        })
    }

    /// Collect listings using date range
    /// Returns (fetched_count, saved_count, oldest_time_in_batch)
    async fn collect_by_date(
        &self,
        date1: &str,
        date2: &str,
    ) -> Result<(usize, usize, Option<String>), String> {
        let query_config = &self.config.api.query;

        let query = ListingsQuery {
            user: String::new(),
            token: String::new(),
            category_id: Some(query_config.category_id.to_string()),
            city: Some(query_config.city.clone()),
            nedvigimost_type: Some(query_config.nedvigimost_type.to_string()),
            limit: Some(1000),
            withphone: Some(0),
            withcoords: query_config.withcoords,
            is_actual: query_config.is_actual.clone(),
            price1: query_config.price1,
            price2: query_config.price2,
            area_from: query_config.area_from,
            area_to: query_config.area_to,
            person_type: query_config.person_type,
            source: query_config.source.clone(),
            date1: Some(date1.to_string()),
            date2: Some(date2.to_string()),
            sort: Some("desc".to_string()), // Newest first, so we can paginate by oldest
            ..Default::default()
        };

        let listings = self
            .api_client
            .get_listings(query)
            .await
            .map_err(|e| format!("API error: {}", e))?;

        let fetched = listings.len();

        if fetched == 0 {
            return Ok((0, 0, None));
        }

        // Get oldest time for pagination (last item since sorted desc)
        let oldest_time = listings.last().and_then(|l| l.time.clone());

        let mut saved = 0;
        for listing in &listings {
            let (db_listing, photo_urls) =
                DbListing::from_api_listing(listing, &query_config.city);

            if let Err(e) = self
                .repository
                .upsert_listing_with_images(&db_listing, &photo_urls)
                .await
            {
                warn!("Failed to save listing {}: {}", listing.id, e);
            } else {
                saved += 1;
            }
        }

        Ok((fetched, saved, oldest_time))
    }

    /// Download images for pending entries
    async fn download_images_batch(&self, limit: i64) -> Result<usize, String> {
        let storage = match &self.storage_client {
            Some(s) => s,
            None => return Ok(0),
        };

        let images = self
            .repository
            .get_images_for_download(limit)
            .await
            .map_err(|e| format!("Failed to get images for download: {}", e))?;

        if images.is_empty() {
            return Ok(0);
        }

        debug!("Downloading {} images", images.len());
        let mut downloaded = 0;

        for image in images {
            let result = storage
                .download_and_store_single(image.listing_id, image.image_index, &image.source_url)
                .await;

            let status = if result.success {
                downloaded += 1;
                ImageStatus::Downloaded
            } else {
                match result.http_status {
                    Some(404) => ImageStatus::NotFound,
                    Some(410) => ImageStatus::Gone,
                    Some(status) if status >= 500 => {
                        if image.download_attempts >= 2 {
                            ImageStatus::Failed
                        } else {
                            ImageStatus::Pending
                        }
                    }
                    _ => {
                        if image.download_attempts >= 2 {
                            ImageStatus::Failed
                        } else {
                            ImageStatus::Pending
                        }
                    }
                }
            };

            if let Err(e) = self
                .repository
                .update_image_download(
                    image.id,
                    status,
                    result.s3_key.as_deref(),
                    result.http_status.map(|s| s as i32),
                    result.error.as_deref(),
                )
                .await
            {
                warn!("Failed to update image {}: {}", image.id, e);
            }
        }

        Ok(downloaded)
    }

    /// Run per-image AI analysis continuously until all images are done
    pub async fn run_image_analysis_all(&self, concurrency: usize) -> Result<usize, String> {
        let analyzer = match &self.image_analyzer {
            Some(a) => a.clone(),
            None => return Err("Image analyzer not configured".to_string()),
        };

        let storage = match &self.storage_client {
            Some(s) => s.clone(),
            None => return Err("Storage client not configured".to_string()),
        };

        info!("Starting continuous image analysis with concurrency {}", concurrency);
        let mut total_analyzed = 0;
        let batch_size = 100i64;

        loop {
            let images = self
                .repository
                .get_images_for_ai_analysis(batch_size)
                .await
                .map_err(|e| format!("Failed to get images for analysis: {}", e))?;

            if images.is_empty() {
                info!("All images analyzed! Total: {}", total_analyzed);
                break;
            }

            let batch_count = images.len();
            debug!("Processing batch of {} images", batch_count);

            let analyzed = Arc::new(AtomicUsize::new(0));
            let repository = self.repository.clone();
            let model = analyzer.model().to_string();

            stream::iter(images)
                .map(|image| {
                    let analyzer = analyzer.clone();
                    let storage = storage.clone();
                    let repository = repository.clone();
                    let analyzed = analyzed.clone();
                    let model = model.clone();

                    async move {
                        let s3_key = match &image.s3_key {
                            Some(k) => k.clone(),
                            None => return,
                        };

                        let image_bytes = match storage.get_image_by_key(&s3_key).await {
                            Ok(b) => b,
                            Err(e) => {
                                warn!("Image {} not found in S3, marking as failed: {}", image.id, e);
                                // Mark as failed so we don't keep retrying
                                let _ = repository.mark_image_s3_missing(image.id).await;
                                return;
                            }
                        };

                        let image_base64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

                        match analyzer.analyze_base64(&image_base64).await {
                            Ok(analysis) => {
                                if let Err(e) = repository
                                    .save_image_analysis(image.id, &analysis, &model)
                                    .await
                                {
                                    warn!("Failed to save analysis for image {}: {}", image.id, e);
                                } else {
                                    analyzed.fetch_add(1, Ordering::Relaxed);
                                    debug!(
                                        "Analyzed image {}: type={}, condition={:?}",
                                        image.id, analysis.image_type, analysis.condition
                                    );
                                }
                            }
                            Err(e) => {
                                warn!("Failed to analyze image {}: {}", image.id, e);
                            }
                        }
                    }
                })
                .buffer_unordered(concurrency)
                .collect::<Vec<()>>()
                .await;

            let batch_analyzed = analyzed.load(Ordering::Relaxed);
            total_analyzed += batch_analyzed;
            info!("Batch complete: {}/{} analyzed (total: {})", batch_analyzed, batch_count, total_analyzed);
        }

        Ok(total_analyzed)
    }

    /// Run per-image AI analysis batch (single batch, for backward compatibility)
    async fn analyze_images_batch(&self, limit: i64) -> Result<usize, String> {
        let analyzer = match &self.image_analyzer {
            Some(a) => a,
            None => return Ok(0),
        };

        let storage = match &self.storage_client {
            Some(s) => s,
            None => return Ok(0),
        };

        let images = self
            .repository
            .get_images_for_ai_analysis(limit)
            .await
            .map_err(|e| format!("Failed to get images for analysis: {}", e))?;

        if images.is_empty() {
            return Ok(0);
        }

        debug!("Analyzing {} images", images.len());
        let mut analyzed = 0;

        for image in images {
            let s3_key = match &image.s3_key {
                Some(k) => k,
                None => continue,
            };

            // Get image from S3 and convert to base64
            let image_bytes = match storage.get_image_by_key(s3_key).await {
                Ok(b) => b,
                Err(e) => {
                    warn!("Failed to get image {} from S3: {}", image.id, e);
                    continue;
                }
            };

            let image_base64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

            match analyzer.analyze_base64(&image_base64).await {
                Ok(analysis) => {
                    if let Err(e) = self
                        .repository
                        .save_image_analysis(image.id, &analysis, analyzer.model())
                        .await
                    {
                        warn!("Failed to save analysis for image {}: {}", image.id, e);
                    } else {
                        analyzed += 1;
                        debug!(
                            "Analyzed image {}: type={}, condition={:?}",
                            image.id, analysis.image_type, analysis.condition
                        );
                    }
                }
                Err(e) => {
                    warn!("Failed to analyze image {}: {}", image.id, e);
                }
            }
        }

        Ok(analyzed)
    }

    /// Run listing-level AI analysis batch
    async fn analyze_listings_batch(&self, limit: i64) -> Result<usize, String> {
        let analyzer = match &self.listing_analyzer {
            Some(a) => a,
            None => return Ok(0),
        };

        let listing_ids = self
            .repository
            .get_listings_for_second_layer_analysis(limit)
            .await
            .map_err(|e| format!("Failed to get listings for analysis: {}", e))?;

        if listing_ids.is_empty() {
            return Ok(0);
        }

        debug!("Analyzing {} listings", listing_ids.len());
        let mut analyzed = 0;

        for listing_id in listing_ids {
            let listing = match self
                .repository
                .get_listing_for_analysis(listing_id)
                .await
                .map_err(|e| format!("Failed to get listing {}: {}", listing_id, e))?
            {
                Some(l) => l,
                None => continue,
            };

            let image_analyses = self
                .repository
                .get_listing_image_analyses(listing_id)
                .await
                .map_err(|e| format!("Failed to get image analyses for {}: {}", listing_id, e))?;

            if image_analyses.is_empty() {
                continue;
            }

            match analyzer.analyze(&listing, &image_analyses).await {
                Ok(analysis) => {
                    if let Err(e) = self
                        .repository
                        .save_listing_analysis(listing_id, &analysis, analyzer.model())
                        .await
                    {
                        warn!(
                            "Failed to save listing analysis for {}: {}",
                            listing_id, e
                        );
                    } else {
                        analyzed += 1;
                        debug!(
                            "Analyzed listing {}: condition={}, shell={}",
                            listing_id, analysis.overall_condition, analysis.is_shell
                        );
                    }
                }
                Err(e) => {
                    warn!("Failed to analyze listing {}: {}", listing_id, e);
                }
            }
        }

        Ok(analyzed)
    }

    /// Sync actuality status of listings
    pub async fn sync_actuality(&self, hours: u32) -> Result<SyncResult, String> {
        info!("Starting actuality sync for the last {} hours", hours);

        let check_feed = self
            .api_client
            .get_check_feed_since_hours(hours)
            .await
            .map_err(|e| format!("Failed to get check feed: {}", e))?;

        info!("Received {} items from check feed", check_feed.len());

        let mut marked_inactive = 0;
        let mut still_active = 0;

        for item in check_feed {
            if item.is_actual == 0 {
                match self.repository.mark_listing_inactive(item.id).await {
                    Ok(updated) => {
                        if updated {
                            marked_inactive += 1;
                            debug!("Marked listing {} as inactive", item.id);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to mark listing {} as inactive: {}", item.id, e);
                    }
                }
            } else {
                still_active += 1;
            }
        }

        info!(
            "Actuality sync completed: {} marked inactive, {} still active",
            marked_inactive, still_active
        );

        Ok(SyncResult {
            total_checked: marked_inactive + still_active,
            marked_inactive,
            still_active,
        })
    }

    /// Check actuality for specific listing IDs
    pub async fn check_actuality_by_ids(
        &self,
        ids: &[i64],
    ) -> Result<Vec<CheckActualityResult>, String> {
        let results = self
            .api_client
            .check_actuality_by_ids(ids)
            .await
            .map_err(|e| format!("Failed to check actuality: {}", e))?;

        Ok(results
            .into_iter()
            .map(|item| CheckActualityResult {
                id: item.id,
                is_actual: item.is_actual,
                checked_at: item.time,
            })
            .collect())
    }

    /// Run listing-level AI analysis continuously until all listings are done
    pub async fn run_listing_analysis_all(&self, concurrency: usize) -> Result<usize, String> {
        let analyzer = match &self.listing_analyzer {
            Some(a) => a.clone(),
            None => return Err("Listing analyzer not configured".to_string()),
        };

        info!("Starting continuous listing analysis with concurrency {}", concurrency);
        let mut total_analyzed = 0;
        let batch_size = 100i64;

        loop {
            let listing_ids = self
                .repository
                .get_listings_for_second_layer_analysis(batch_size)
                .await
                .map_err(|e| format!("Failed to get listings for analysis: {}", e))?;

            if listing_ids.is_empty() {
                break;
            }

            info!("Processing batch of {} listings", listing_ids.len());

            // Process concurrently using semaphore
            let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
            let mut handles = Vec::new();

            for listing_id in listing_ids {
                let permit = semaphore.clone().acquire_owned().await.unwrap();
                let repo = self.repository.clone();
                let analyzer = analyzer.clone();

                let handle = tokio::spawn(async move {
                    let _permit = permit;

                    let listing = match repo.get_listing_for_analysis(listing_id).await {
                        Ok(Some(l)) => l,
                        Ok(None) => return None,
                        Err(e) => {
                            warn!("Failed to get listing {}: {}", listing_id, e);
                            return None;
                        }
                    };

                    let image_analyses = match repo.get_listing_image_analyses(listing_id).await {
                        Ok(a) => a,
                        Err(e) => {
                            warn!("Failed to get image analyses for {}: {}", listing_id, e);
                            return None;
                        }
                    };

                    // Handle case with no analyzed images - mark as "no photos"
                    if image_analyses.is_empty() {
                        let no_photos_analysis = crate::db::ListingAnalysisResult {
                            overall_condition: 0,
                            is_shell: true,
                            move_in_ready: false,
                            red_flags: vec!["Нет фото".to_string()],
                            summary: "Объявление без фотографий. Невозможно оценить состояние квартиры.".to_string(),
                        };
                        if let Err(e) = repo
                            .save_listing_analysis(listing_id, &no_photos_analysis, "none")
                            .await
                        {
                            warn!("Failed to save no-photos analysis for {}: {}", listing_id, e);
                            return None;
                        }
                        debug!("Marked listing {} as no-photos", listing_id);
                        return Some(());
                    }

                    match analyzer.analyze(&listing, &image_analyses).await {
                        Ok(analysis) => {
                            if let Err(e) = repo
                                .save_listing_analysis(listing_id, &analysis, analyzer.model())
                                .await
                            {
                                warn!("Failed to save listing analysis for {}: {}", listing_id, e);
                                None
                            } else {
                                debug!("Analyzed listing {}: condition={}", listing_id, analysis.overall_condition);
                                Some(())
                            }
                        }
                        Err(e) => {
                            warn!("Failed to analyze listing {}: {}", listing_id, e);
                            None
                        }
                    }
                });

                handles.push(handle);
            }

            let results = futures::future::join_all(handles).await;
            let batch_analyzed = results.into_iter().filter(|r| r.as_ref().ok().and_then(|o| o.as_ref()).is_some()).count();
            total_analyzed += batch_analyzed;
            info!("Listing analysis batch completed: {} analyzed (total: {})", batch_analyzed, total_analyzed);
        }

        info!("Continuous listing analysis completed: {} total", total_analyzed);
        Ok(total_analyzed)
    }

    /// Analyze all images for a single listing
    pub async fn analyze_listing_images(&self, listing_id: i64) -> Result<usize, String> {
        let analyzer = match &self.image_analyzer {
            Some(a) => a,
            None => return Err("Image analyzer not configured".to_string()),
        };

        let storage = match &self.storage_client {
            Some(s) => s,
            None => return Err("Storage client not configured".to_string()),
        };

        let images = self
            .repository
            .get_images_for_listing(listing_id)
            .await
            .map_err(|e| format!("Failed to get images: {}", e))?;

        let downloaded_images: Vec<_> = images
            .into_iter()
            .filter(|img| img.s3_key.is_some() && img.status == ImageStatus::Downloaded)
            .collect();

        if downloaded_images.is_empty() {
            return Ok(0);
        }

        info!("Analyzing {} images for listing {}", downloaded_images.len(), listing_id);
        let mut analyzed = 0;

        for image in downloaded_images {
            let s3_key = image.s3_key.as_ref().unwrap();

            let image_bytes = match storage.get_image_by_key(s3_key).await {
                Ok(b) => b,
                Err(e) => {
                    warn!("Failed to get image {} from S3: {}", image.id, e);
                    continue;
                }
            };

            let image_base64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

            match analyzer.analyze_base64(&image_base64).await {
                Ok(analysis) => {
                    if let Err(e) = self
                        .repository
                        .save_image_analysis(image.id, &analysis, analyzer.model())
                        .await
                    {
                        warn!("Failed to save analysis for image {}: {}", image.id, e);
                    } else {
                        analyzed += 1;
                        debug!(
                            "Analyzed image {}: type={}, condition={:?}",
                            image.id, analysis.image_type, analysis.condition
                        );
                    }
                }
                Err(e) => {
                    warn!("Failed to analyze image {}: {}", image.id, e);
                }
            }
        }

        Ok(analyzed)
    }

    /// Analyze a single listing (second layer)
    pub async fn analyze_single_listing(&self, listing_id: i64) -> Result<bool, String> {
        self.analyze_single_listing_with_model(listing_id, None).await
    }

    /// Analyze a single listing with optional custom model
    pub async fn analyze_single_listing_with_model(
        &self,
        listing_id: i64,
        custom_model: Option<String>,
    ) -> Result<bool, String> {
        // Create temporary analyzer if custom model specified, otherwise use default
        let temp_analyzer: Option<ListingAnalyzer>;
        let analyzer: &ListingAnalyzer = match &custom_model {
            Some(model) => {
                let api_key = self
                    .vision_api_key
                    .as_ref()
                    .ok_or_else(|| "Model API key not configured".to_string())?;
                temp_analyzer = Some(ListingAnalyzer::new(
                    api_key.clone(),
                    model.clone(),
                    self.config.vision.api_base_url.clone(),
                ));
                temp_analyzer.as_ref().unwrap()
            }
            None => match &self.listing_analyzer {
                Some(a) => a.as_ref(),
                None => return Err("Listing analyzer not configured".to_string()),
            },
        };

        let listing = match self
            .repository
            .get_listing_for_analysis(listing_id)
            .await
            .map_err(|e| format!("Failed to get listing {}: {}", listing_id, e))?
        {
            Some(l) => l,
            None => return Err(format!("Listing {} not found", listing_id)),
        };

        // Check if listing has any images at all
        let all_images = self
            .repository
            .get_images_for_listing(listing_id)
            .await
            .map_err(|e| format!("Failed to get images: {}", e))?;

        // If no images at all, save "no photos" analysis
        if all_images.is_empty() {
            let no_photos_analysis = crate::db::ListingAnalysisResult {
                overall_condition: 0,
                is_shell: true,
                move_in_ready: false,
                red_flags: vec!["Нет фото".to_string()],
                summary: "Объявление без фотографий, невозможно оценить состояние квартиры."
                    .to_string(),
            };

            self.repository
                .save_listing_analysis(listing_id, &no_photos_analysis, "none")
                .await
                .map_err(|e| format!("Failed to save analysis: {}", e))?;

            info!(
                "Listing {} has no photos, marked as no data",
                listing_id
            );
            return Ok(true);
        }

        let image_analyses = self
            .repository
            .get_listing_image_analyses(listing_id)
            .await
            .map_err(|e| format!("Failed to get image analyses: {}", e))?;

        // If has images but none analyzed yet, return error (need to analyze images first)
        if image_analyses.is_empty() {
            return Err("No analyzed images for this listing. Run image analysis first.".to_string());
        }

        info!(
            "Analyzing listing {} with {} image analyses using model: {}",
            listing_id,
            image_analyses.len(),
            analyzer.model()
        );

        match analyzer.analyze(&listing, &image_analyses).await {
            Ok(analysis) => {
                self.repository
                    .save_listing_analysis(listing_id, &analysis, analyzer.model())
                    .await
                    .map_err(|e| format!("Failed to save analysis: {}", e))?;

                info!(
                    "Analyzed listing {}: condition={}, shell={}, model={}",
                    listing_id, analysis.overall_condition, analysis.is_shell, analyzer.model()
                );
                Ok(true)
            }
            Err(e) => Err(format!("Analysis failed: {}", e)),
        }
    }
}

#[derive(Debug)]
pub struct CollectionResult {
    pub total_fetched: usize,
    pub total_saved: usize,
    pub total_images_downloaded: usize,
    pub total_images_analyzed: usize,
    pub total_listings_analyzed: usize,
    pub errors: Vec<String>,
}

impl CollectionResult {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "total_fetched": self.total_fetched,
            "total_saved": self.total_saved,
            "total_images_downloaded": self.total_images_downloaded,
            "total_images_analyzed": self.total_images_analyzed,
            "total_listings_analyzed": self.total_listings_analyzed,
            "errors": self.errors,
            "success": self.errors.is_empty()
        })
    }
}

#[derive(Debug)]
pub struct SyncResult {
    pub total_checked: usize,
    pub marked_inactive: usize,
    pub still_active: usize,
}

impl SyncResult {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "total_checked": self.total_checked,
            "marked_inactive": self.marked_inactive,
            "still_active": self.still_active
        })
    }
}

#[derive(Debug, serde::Serialize)]
pub struct CheckActualityResult {
    pub id: i64,
    pub is_actual: i32,
    pub checked_at: Option<String>,
}
