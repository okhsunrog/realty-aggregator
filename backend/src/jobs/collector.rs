use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::ads_api::AdsApiClient;
use crate::ads_api::models::ListingsQuery;
use crate::config::Config;
use crate::db::{DbListing, Repository, VisionStatus};
use crate::storage::StorageClient;
use crate::vision::VisionAnalyzer;

pub struct Collector {
    api_client: Arc<AdsApiClient>,
    repository: Arc<Repository>,
    vision_analyzer: Option<Arc<VisionAnalyzer>>,
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
            vision_analyzer: None,
            storage_client: None,
            config,
            is_running: Arc::new(RwLock::new(false)),
        }
    }

    pub fn with_vision_analyzer(mut self, analyzer: Arc<VisionAnalyzer>) -> Self {
        self.vision_analyzer = Some(analyzer);
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
        // Check if already running
        {
            let mut running = self.is_running.write().await;
            if *running {
                return Err("Collection already in progress".to_string());
            }
            *running = true;
        }

        let result = self.do_collection().await;

        // Reset running flag
        {
            let mut running = self.is_running.write().await;
            *running = false;
        }

        result
    }

    async fn do_collection(&self) -> Result<CollectionResult, String> {
        info!("Starting collection job");

        let query_config = &self.config.api.query;
        let mut total_fetched = 0;
        let mut total_saved = 0;
        let mut errors = Vec::new();

        // Get last collected ID for incremental loading
        let last_id = self
            .repository
            .get_collection_state()
            .await
            .ok()
            .and_then(|s| s.last_listing_id);

        info!(
            "Collecting listings for city: {}, category: {}, startid: {:?}",
            query_config.city, query_config.category_id, last_id
        );

        match self.collect_from_config(last_id).await {
            Ok((fetched, saved, max_id)) => {
                total_fetched = fetched;
                total_saved = saved;
                info!(
                    "Collected {} listings for {}, saved {}",
                    fetched, query_config.city, saved
                );

                // Update collection state with max ID
                if let Some(new_max_id) = max_id {
                    if let Err(e) = self.repository.update_collection_state(new_max_id).await {
                        warn!("Failed to update collection state: {}", e);
                    }
                }
            }
            Err(e) => {
                let error_msg = format!("Error collecting for {}: {}", query_config.city, e);
                error!("{}", error_msg);
                errors.push(error_msg);
            }
        }

        info!(
            "Collection completed. Total fetched: {}, saved: {}",
            total_fetched, total_saved
        );

        // Download images for new listings
        let total_images_stored = if self.storage_client.is_some() {
            match self.download_images_batch(50).await {
                Ok(count) => {
                    info!("Images stored for {} listings", count);
                    count
                }
                Err(e) => {
                    let error_msg = format!("Image download error: {}", e);
                    error!("{}", error_msg);
                    errors.push(error_msg);
                    0
                }
            }
        } else {
            0
        };

        // Run vision analysis on listings with stored images (if auto_analyze enabled)
        let total_analyzed = if self.vision_analyzer.is_some() && self.config.vision.auto_analyze {
            match self.run_vision_analysis_batch(50).await {
                Ok(count) => {
                    info!("Vision analysis completed: {} listings analyzed", count);
                    count
                }
                Err(e) => {
                    let error_msg = format!("Vision analysis error: {}", e);
                    error!("{}", error_msg);
                    errors.push(error_msg);
                    0
                }
            }
        } else {
            0
        };

        Ok(CollectionResult {
            total_fetched,
            total_saved,
            total_images_stored,
            total_analyzed,
            errors,
        })
    }

    /// Collect listings using config settings with optional startid for incremental loading
    /// Returns (fetched_count, saved_count, max_id)
    async fn collect_from_config(
        &self,
        start_id: Option<i64>,
    ) -> Result<(usize, usize, Option<i64>), String> {
        let query_config = &self.config.api.query;

        let query = ListingsQuery {
            user: String::new(), // will be set by client
            token: String::new(),
            category_id: Some(query_config.category_id.to_string()),
            city: Some(query_config.city.clone()),
            nedvigimost_type: Some(query_config.nedvigimost_type.to_string()),
            limit: Some(1000),
            withphone: Some(0), // Always include listings without phones (api-without-phones tariff)
            withcoords: query_config.withcoords,
            is_actual: query_config.is_actual.clone(),
            price1: query_config.price1,
            price2: query_config.price2,
            area_from: query_config.area_from,
            area_to: query_config.area_to,
            person_type: query_config.person_type,
            source: query_config.source.clone(),
            startid: start_id,
            sort: Some("asc".to_string()), // asc to get new items after startid
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

        // Find max ID for next incremental load
        let max_id = listings.iter().map(|l| l.id).max();

        let db_listings: Vec<DbListing> = listings
            .iter()
            .map(|l| DbListing::from_api_listing(l, &query_config.city))
            .collect();

        let saved = self
            .repository
            .upsert_listings(&db_listings)
            .await
            .map_err(|e| format!("DB error: {}", e))?;

        Ok((fetched, saved, max_id))
    }

    /// Download and store images for listings that don't have them yet
    async fn download_images_batch(&self, limit: i64) -> Result<usize, String> {
        let storage = match &self.storage_client {
            Some(s) => s,
            None => return Ok(0),
        };

        let listings = self
            .repository
            .get_listings_needing_images(limit)
            .await
            .map_err(|e| format!("Failed to get listings needing images: {}", e))?;

        if listings.is_empty() {
            return Ok(0);
        }

        info!("Downloading images for {} listings", listings.len());
        let mut stored_count = 0;

        for listing in listings {
            let photo_urls = listing.get_photo_urls();

            if photo_urls.is_empty() {
                // No photos, mark as stored with 0 count
                self.repository
                    .mark_images_stored(listing.id, 0)
                    .await
                    .map_err(|e| format!("Failed to mark images stored: {}", e))?;
                continue;
            }

            match storage
                .download_and_store_images(listing.id, &photo_urls)
                .await
            {
                Ok(stored_indices) => {
                    let count = stored_indices.len() as i32;
                    self.repository
                        .mark_images_stored(listing.id, count)
                        .await
                        .map_err(|e| format!("Failed to mark images stored: {}", e))?;
                    stored_count += 1;
                    debug!("Stored {} images for listing {}", count, listing.id);
                }
                Err(e) => {
                    warn!("Failed to store images for listing {}: {}", listing.id, e);
                    // Mark as stored with 0 to avoid retrying forever
                    let _ = self.repository.mark_images_stored(listing.id, 0).await;
                }
            }
        }

        Ok(stored_count)
    }

    /// Run vision analysis on a batch of pending listings
    /// Applies pre-analysis filtering from config
    pub async fn run_vision_analysis_batch(&self, limit: i64) -> Result<usize, String> {
        let analyzer = match &self.vision_analyzer {
            Some(a) => a,
            None => return Ok(0),
        };

        let filter_config = &self.config.vision.filter;
        let max_retries = self.config.vision.max_retries;

        let listings = self
            .repository
            .get_listings_for_analysis(limit)
            .await
            .map_err(|e| format!("Failed to get pending listings: {}", e))?;

        if listings.is_empty() {
            debug!("No listings to analyze");
            return Ok(0);
        }

        info!("Running vision analysis on {} listings", listings.len());

        let mut analyzed_count = 0;

        for listing in listings {
            // Check if listing passes pre-analysis filter
            let photos_count = listing.photos_count();
            let passes_filter = filter_config.passes_filter(
                photos_count,
                listing.price,
                listing.area,
                Some(&listing.source),
                listing.has_coords(),
            );

            if !passes_filter {
                debug!(
                    "Listing {} doesn't pass filter, marking as not_needed",
                    listing.id
                );
                self.repository
                    .set_vision_status_not_needed(listing.id, "Did not pass pre-analysis filter")
                    .await
                    .map_err(|e| format!("Failed to update status: {}", e))?;
                continue;
            }

            if photos_count == 0 {
                debug!(
                    "Listing {} has no photos, marking as not_needed",
                    listing.id
                );
                self.repository
                    .set_vision_status_not_needed(listing.id, "No photos available")
                    .await
                    .map_err(|e| format!("Failed to update status: {}", e))?;
                continue;
            }

            // Get photo URLs (either from stored images or original URLs)
            let photo_urls = listing.get_photo_urls();

            // Limit photos if configured
            let photos_to_analyze: Vec<_> = if let Some(max_photos) = filter_config.max_photos {
                photo_urls.into_iter().take(max_photos).collect()
            } else {
                photo_urls
            };

            match analyzer.analyze(&photos_to_analyze).await {
                Ok(analysis) => {
                    let analysis_json = serde_json::to_value(&analysis)
                        .map_err(|e| format!("Failed to serialize analysis: {}", e))?;

                    self.repository
                        .save_vision_analysis(listing.id, &analysis_json, analyzer.model())
                        .await
                        .map_err(|e| {
                            format!("Failed to save analysis for {}: {}", listing.id, e)
                        })?;

                    analyzed_count += 1;
                    debug!(
                        "Analyzed listing {}: condition={}, score={}",
                        listing.id, analysis.condition.state, analysis.overall_score.value
                    );
                }
                Err(e) => {
                    warn!("Failed to analyze listing {}: {}", listing.id, e);

                    // Check retry count
                    let attempts = listing.vision_attempts + 1;
                    if attempts >= max_retries as i32 {
                        self.repository
                            .update_vision_status(
                                listing.id,
                                VisionStatus::Failed,
                                Some(&e.to_string()),
                            )
                            .await
                            .map_err(|e| format!("Failed to update status: {}", e))?;
                    } else {
                        // Keep as pending with error recorded
                        self.repository
                            .update_vision_status(
                                listing.id,
                                VisionStatus::Pending,
                                Some(&e.to_string()),
                            )
                            .await
                            .map_err(|e| format!("Failed to update status: {}", e))?;
                    }
                }
            }
        }

        Ok(analyzed_count)
    }

    /// Run vision analysis on a single listing by ID
    pub async fn analyze_single_listing(
        &self,
        listing_id: i64,
    ) -> Result<serde_json::Value, String> {
        let analyzer = match &self.vision_analyzer {
            Some(a) => a,
            None => return Err("Vision analyzer not configured".to_string()),
        };

        let listing = self
            .repository
            .get_listing_by_id(listing_id)
            .await
            .map_err(|e| format!("Failed to get listing: {}", e))?
            .ok_or_else(|| format!("Listing {} not found", listing_id))?;

        let photo_urls = listing.get_photo_urls();

        if photo_urls.is_empty() {
            return Err(format!("Listing {} has no photos", listing_id));
        }

        let analysis = analyzer
            .analyze(&photo_urls)
            .await
            .map_err(|e| format!("Analysis failed: {}", e))?;

        let analysis_json = serde_json::to_value(&analysis)
            .map_err(|e| format!("Failed to serialize analysis: {}", e))?;

        self.repository
            .save_vision_analysis(listing_id, &analysis_json, analyzer.model())
            .await
            .map_err(|e| format!("Failed to save analysis: {}", e))?;

        Ok(analysis_json)
    }

    /// Sync actuality status of listings using the check feed API
    /// Marks listings as inactive if they are no longer actual
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
            // is_actual: 0 = inactive, 1 or 11 = active
            if item.is_actual == 0 {
                match self.repository.mark_listing_inactive(item.id).await {
                    Ok(updated) => {
                        if updated {
                            marked_inactive += 1;
                            debug!("Marked listing {} as inactive", item.id);
                        } else {
                            // Listing not found in our DB, skip
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
}

#[derive(Debug)]
pub struct CollectionResult {
    pub total_fetched: usize,
    pub total_saved: usize,
    pub total_images_stored: usize,
    pub total_analyzed: usize,
    pub errors: Vec<String>,
}

impl CollectionResult {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "total_fetched": self.total_fetched,
            "total_saved": self.total_saved,
            "total_images_stored": self.total_images_stored,
            "total_analyzed": self.total_analyzed,
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
