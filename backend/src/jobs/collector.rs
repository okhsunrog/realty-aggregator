use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use base64::Engine;
use futures::{stream, StreamExt};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::ads_api::AdsApiClient;
use crate::ads_api::models::ListingsQuery;
use crate::config::Config;
use crate::db::{DbListing, ImageStatus, Repository};
use crate::storage::StorageClient;
use crate::vision::{ImageAnalyzer, ListingAnalyzer};

pub struct Collector {
    api_client: Arc<AdsApiClient>,
    repository: Arc<Repository>,
    image_analyzer: Option<Arc<ImageAnalyzer>>,
    listing_analyzer: Option<Arc<ListingAnalyzer>>,
    storage_client: Option<Arc<StorageClient>>,
    config: Arc<Config>,
    is_running: Arc<RwLock<bool>>,
}

impl Collector {
    pub fn new(
        api_client: Arc<AdsApiClient>,
        repository: Arc<Repository>,
        config: Arc<Config>,
    ) -> Self {
        Self {
            api_client,
            repository,
            image_analyzer: None,
            listing_analyzer: None,
            storage_client: None,
            config,
            is_running: Arc::new(RwLock::new(false)),
        }
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

    pub async fn is_running(&self) -> bool {
        *self.is_running.read().await
    }

    pub async fn run_collection(&self) -> Result<CollectionResult, String> {
        {
            let mut running = self.is_running.write().await;
            if *running {
                return Err("Collection already in progress".to_string());
            }
            *running = true;
        }

        let result = self.do_collection().await;

        {
            let mut running = self.is_running.write().await;
            *running = false;
        }

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

        // Get last collected ID for incremental loading
        let mut current_start_id = self
            .repository
            .get_collection_state()
            .await
            .ok()
            .and_then(|s| s.last_listing_id);

        info!(
            "Collecting listings for city: {}, category: {}, startid: {:?}",
            query_config.city, query_config.category_id, current_start_id
        );

        // Loop until we fetch less than the limit (meaning no more data)
        loop {
            match self.collect_from_config(current_start_id).await {
                Ok((fetched, saved, max_id)) => {
                    total_fetched += fetched;
                    total_saved += saved;
                    info!(
                        "Batch: fetched {}, saved {} (total: {}/{})",
                        fetched, saved, total_fetched, total_saved
                    );

                    if let Some(new_max_id) = max_id {
                        if let Err(e) = self.repository.update_collection_state(new_max_id).await {
                            warn!("Failed to update collection state: {}", e);
                        }
                        current_start_id = Some(new_max_id);
                    }

                    if fetched < 1000 {
                        info!("Fetched all available listings");
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

        info!(
            "Collection completed. Total fetched: {}, saved: {}",
            total_fetched, total_saved
        );

        // Download images for all pending images
        let mut total_images_downloaded = 0;
        if self.storage_client.is_some() {
            loop {
                match self.download_images_batch(100).await {
                    Ok(count) => {
                        if count == 0 {
                            break;
                        }
                        total_images_downloaded += count;
                        info!(
                            "Images batch: {} downloaded (total: {})",
                            count, total_images_downloaded
                        );
                    }
                    Err(e) => {
                        let error_msg = format!("Image download error: {}", e);
                        error!("{}", error_msg);
                        errors.push(error_msg);
                        break;
                    }
                }
            }
            info!("All images downloaded: {}", total_images_downloaded);
        }

        // Run per-image AI analysis (if auto_analyze enabled)
        let mut total_images_analyzed = 0;
        if self.image_analyzer.is_some()
            && self.storage_client.is_some()
            && self.config.vision.auto_analyze
        {
            loop {
                match self.analyze_images_batch(50).await {
                    Ok(count) => {
                        if count == 0 {
                            break;
                        }
                        total_images_analyzed += count;
                        info!(
                            "Image analysis batch: {} (total: {})",
                            count, total_images_analyzed
                        );
                    }
                    Err(e) => {
                        let error_msg = format!("Image analysis error: {}", e);
                        error!("{}", error_msg);
                        errors.push(error_msg);
                        break;
                    }
                }
            }
            info!("Image analysis completed: {}", total_images_analyzed);
        }

        // Run listing-level analysis (if auto_analyze enabled)
        let mut total_listings_analyzed = 0;
        if self.listing_analyzer.is_some() && self.config.vision.auto_analyze {
            loop {
                match self.analyze_listings_batch(20).await {
                    Ok(count) => {
                        if count == 0 {
                            break;
                        }
                        total_listings_analyzed += count;
                        info!(
                            "Listing analysis batch: {} (total: {})",
                            count, total_listings_analyzed
                        );
                    }
                    Err(e) => {
                        let error_msg = format!("Listing analysis error: {}", e);
                        error!("{}", error_msg);
                        errors.push(error_msg);
                        break;
                    }
                }
            }
            info!("Listing analysis completed: {}", total_listings_analyzed);
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

    /// Collect listings using config settings
    async fn collect_from_config(
        &self,
        start_id: Option<i64>,
    ) -> Result<(usize, usize, Option<i64>), String> {
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
            startid: start_id,
            sort: Some("asc".to_string()),
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

        let max_id = listings.iter().map(|l| l.id).max();

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

        Ok((fetched, saved, max_id))
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

    /// Manually trigger image analysis for a batch
    pub async fn run_image_analysis_batch(&self, limit: i64) -> Result<usize, String> {
        self.analyze_images_batch(limit).await
    }

    /// Manually trigger listing analysis for a batch
    pub async fn run_listing_analysis_batch(&self, limit: i64) -> Result<usize, String> {
        self.analyze_listings_batch(limit).await
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
        let analyzer = match &self.listing_analyzer {
            Some(a) => a,
            None => return Err("Listing analyzer not configured".to_string()),
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

        let image_analyses = self
            .repository
            .get_listing_image_analyses(listing_id)
            .await
            .map_err(|e| format!("Failed to get image analyses: {}", e))?;

        if image_analyses.is_empty() {
            return Err("No analyzed images for this listing".to_string());
        }

        info!("Analyzing listing {} with {} image analyses", listing_id, image_analyses.len());

        match analyzer.analyze(&listing, &image_analyses).await {
            Ok(analysis) => {
                self.repository
                    .save_listing_analysis(listing_id, &analysis, analyzer.model())
                    .await
                    .map_err(|e| format!("Failed to save analysis: {}", e))?;

                info!(
                    "Analyzed listing {}: condition={}, shell={}",
                    listing_id, analysis.overall_condition, analysis.is_shell
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
