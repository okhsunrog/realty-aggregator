use crate::{
    ads_api::{AdsApiClient, models::ListingsQuery},
    config::Config,
    db::{DbListing, ImageAnalysisResult, ImageStatus, ListingAnalysisResult, Repository},
    storage::StorageClient,
    vision::{ImageAnalyzer, ListingAnalyzer},
};
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use futures::{FutureExt, StreamExt, stream};
use serde::Serialize;
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{RwLock, broadcast};

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
#[derive(Debug, Clone, Serialize)]
pub struct JobProgress {
    pub processed: usize,
    pub total: Option<usize>,
}
#[derive(Debug, Clone, Serialize)]
pub struct JobStatus {
    pub phase: JobPhase,
    pub progress: Option<JobProgress>,
    pub started_at: Option<DateTime<Utc>>,
    pub message: Option<String>,
    pub last_error: Option<String>,
}
impl Default for JobStatus {
    fn default() -> Self {
        Self {
            phase: JobPhase::Idle,
            progress: None,
            started_at: None,
            message: None,
            last_error: None,
        }
    }
}

pub enum Job {
    Collect,
    CheckSources { ids: Vec<i64> },
    Images { concurrency: usize },
    Listings { concurrency: usize },
    ListingImages(i64),
    Listing { id: i64, model: Option<String> },
    Sync { hours: u32 },
}
impl Job {
    fn phase(&self) -> JobPhase {
        match self {
            Self::Collect => JobPhase::CollectingListings,
            Self::Images { .. } | Self::ListingImages(_) => JobPhase::AnalyzingImages,
            Self::Listings { .. } | Self::Listing { .. } => JobPhase::AnalyzingListings,
            Self::Sync { .. } | Self::CheckSources { .. } => JobPhase::SyncingActuality,
        }
    }
}
struct Permit(Arc<AtomicBool>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub struct Collector {
    source_checker: crate::actuality::SourceChecker,
    api_client: Arc<AdsApiClient>,
    repository: Arc<Repository>,
    config: Arc<Config>,
    image_analyzer: Option<Arc<ImageAnalyzer>>,
    listing_analyzer: Option<Arc<ListingAnalyzer>>,
    storage_client: Option<Arc<StorageClient>>,
    vision_api_key: Option<String>,
    running: Arc<AtomicBool>,
    status: RwLock<JobStatus>,
    status_tx: broadcast::Sender<JobStatus>,
}
impl Collector {
    pub fn new(
        api_client: Arc<AdsApiClient>,
        repository: Arc<Repository>,
        config: Arc<Config>,
    ) -> Self {
        let (status_tx, _) = broadcast::channel(64);
        Self {
            source_checker: crate::actuality::SourceChecker::new(
                config.actuality.request_delay_seconds,
            )
            .expect("Source HTTP client"),
            api_client,
            repository,
            config,
            image_analyzer: None,
            listing_analyzer: None,
            storage_client: None,
            vision_api_key: None,
            running: Arc::new(AtomicBool::new(false)),
            status: RwLock::new(JobStatus::default()),
            status_tx,
        }
    }
    pub fn with_image_analyzer(mut self, v: Arc<ImageAnalyzer>) -> Self {
        self.image_analyzer = Some(v);
        self
    }
    pub fn with_listing_analyzer(mut self, v: Arc<ListingAnalyzer>) -> Self {
        self.listing_analyzer = Some(v);
        self
    }
    pub fn with_storage_client(mut self, v: Arc<StorageClient>) -> Self {
        self.storage_client = Some(v);
        self
    }
    pub fn with_vision_api_key(mut self, v: String) -> Self {
        self.vision_api_key = Some(v);
        self
    }
    pub fn subscribe(&self) -> broadcast::Receiver<JobStatus> {
        self.status_tx.subscribe()
    }
    pub async fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
    pub async fn get_status(&self) -> JobStatus {
        self.status.read().await.clone()
    }
    fn reserve(&self) -> Result<Permit, String> {
        self.running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "Job already in progress".to_owned())?;
        Ok(Permit(self.running.clone()))
    }
    async fn publish(&self, status: JobStatus) {
        *self.status.write().await = status.clone();
        let _ = self.status_tx.send(status);
    }
    async fn phase(&self, phase: JobPhase, message: &str) {
        self.publish(JobStatus {
            phase,
            message: Some(message.to_owned()),
            started_at: Some(Utc::now()),
            ..Default::default()
        })
        .await;
    }
    async fn progress(&self, processed: usize, total: usize) {
        let mut status = self.get_status().await;
        status.progress = Some(JobProgress {
            processed,
            total: Some(total),
        });
        self.publish(status).await;
    }
    async fn perform<T>(
        &self,
        _permit: Permit,
        phase: JobPhase,
        work: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        self.phase(phase, "Выполняется задача").await;
        let result = std::panic::AssertUnwindSafe(work)
            .catch_unwind()
            .await
            .unwrap_or_else(|_| Err("Задача была прервана внутренней ошибкой".to_owned()));
        self.publish(JobStatus {
            last_error: result.as_ref().err().cloned(),
            ..Default::default()
        })
        .await;
        result
    }
    pub fn start(self: Arc<Self>, job: Job) -> Result<(), String> {
        match &job {
            Job::CheckSources { ids } if ids.len() > 100 => {
                return Err("At most 100 source checks per job".to_owned());
            }
            Job::Images { concurrency } | Job::Listings { concurrency }
                if !(1..=16).contains(concurrency) =>
            {
                return Err("Concurrency must be between 1 and 16".to_owned());
            }
            Job::Listing {
                model: Some(model), ..
            } if !self.config.vision.allowed_models.contains(model) => {
                return Err("Model is not allowed by config.toml".to_owned());
            }
            Job::Sync { hours } if *hours == 0 || *hours > 24 * 365 => {
                return Err("Invalid sync period".to_owned());
            }
            _ => {}
        }
        let permit = self.reserve()?;
        let initial = JobStatus {
            phase: job.phase(),
            started_at: Some(Utc::now()),
            message: Some("Задача запущена".to_owned()),
            ..Default::default()
        };
        if let Ok(mut status) = self.status.try_write() {
            *status = initial.clone();
        }
        let _ = self.status_tx.send(initial);
        tokio::spawn(async move {
            let phase = job.phase();
            let work = async {
                match job {
                    Job::CheckSources { ids } => self.do_source_checks(&ids).await.map(|_| ()),
                    Job::Collect => self.do_collection().await.map(|_| ()),
                    Job::Images { concurrency } => {
                        self.analyze_images(concurrency, None).await.map(|_| ())
                    }
                    Job::Listings { concurrency } => {
                        self.analyze_listings(concurrency).await.map(|_| ())
                    }
                    Job::ListingImages(id) => {
                        self.download_listing_images(id).await?;
                        self.repository
                            .reset_analysis(id, true)
                            .await
                            .map_err(|e| e.to_string())?;
                        self.analyze_images(1, Some(id)).await.map(|_| ())
                    }
                    Job::Listing { id, model } => {
                        self.repository
                            .reset_analysis(id, false)
                            .await
                            .map_err(|e| e.to_string())?;
                        self.analyze_listing(id, model.as_deref()).await
                    }
                    Job::Sync { hours } => self.do_sync(hours).await.map(|_| ()),
                }
            };
            if let Err(e) = self.perform(permit, phase, work).await {
                tracing::error!("Job failed: {e}");
            }
        });
        Ok(())
    }
    pub async fn run_collection(&self) -> Result<CollectionResult, String> {
        self.perform(
            self.reserve()?,
            JobPhase::CollectingListings,
            self.do_collection(),
        )
        .await
    }
    /// Scheduled work waits for the current job instead of losing an entire interval.
    pub async fn run_scheduled(&self, job: Job) -> Result<(), String> {
        if !matches!(
            &job,
            Job::Collect | Job::Sync { .. } | Job::CheckSources { .. }
        ) {
            return Err("Unsupported scheduled job".to_owned());
        }
        let permit = loop {
            if let Ok(permit) = self.reserve() {
                break permit;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        };
        let phase = job.phase();
        self.perform(permit, phase, async {
            match job {
                Job::Collect => self.do_collection().await.map(|_| ()),
                Job::Sync { hours } => self.do_sync(hours).await.map(|_| ()),
                Job::CheckSources { ids } => self.do_source_checks(&ids).await.map(|_| ()),
                _ => unreachable!("scheduled job checked before reserving"),
            }
        })
        .await
    }
    pub async fn sync_actuality(&self, hours: u32) -> Result<SyncResult, String> {
        self.perform(
            self.reserve()?,
            JobPhase::SyncingActuality,
            self.do_sync(hours),
        )
        .await
    }
    pub async fn run_source_checks(&self, ids: &[i64]) -> Result<usize, String> {
        self.perform(
            self.reserve()?,
            JobPhase::SyncingActuality,
            self.do_source_checks(ids),
        )
        .await
    }
    async fn do_source_checks(&self, requested: &[i64]) -> Result<usize, String> {
        let mut ids = requested.to_vec();
        if ids.is_empty() {
            for policy in &self.config.actuality.sources {
                if self
                    .repository
                    .source_ready(&policy.source)
                    .await
                    .map_err(|e| e.to_string())?
                {
                    ids.extend(
                        self.repository
                            .get_source_check_queue(
                                &policy.source,
                                policy.batch_size as i64,
                                self.config.actuality.recheck_hours,
                            )
                            .await
                            .map_err(|e| e.to_string())?,
                    );
                }
            }
        }
        let mut checked = 0;
        for (index, id) in ids.iter().enumerate() {
            let listing = self
                .repository
                .get_listing_by_id(*id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or("Listing not found")?;
            // Re-check after each request: one block pauses the rest of this source's batch.
            if !self
                .repository
                .source_ready(&listing.source)
                .await
                .map_err(|e| e.to_string())?
            {
                self.progress(index + 1, ids.len()).await;
                continue;
            }
            let check = self.source_checker.check(&listing.url).await;
            self.repository
                .save_source_check(*id, &check)
                .await
                .map_err(|e| e.to_string())?;
            self.repository
                .record_source_attempt(&listing.source, &check)
                .await
                .map_err(|e| e.to_string())?;
            tracing::info!(listing_id = id, status = check.status.as_str(), http = ?check.http_status, reason = %check.reason, "Source check completed");
            checked += 1;
            self.progress(index + 1, ids.len()).await;
        }
        if checked == 0 && !requested.is_empty() {
            return Err("Источник временно приостановлен после ограничения доступа. Срок следующей попытки указан в статистике.".into());
        }
        Ok(checked)
    }
    pub async fn run_listing_pipeline(&self, id: i64) -> Result<(), String> {
        self.perform(self.reserve()?, JobPhase::DownloadingImages, async {
            self.download_listing_images(id).await?;
            self.analyze_images(2, Some(id)).await?;
            self.analyze_listing(id, None).await
        })
        .await
    }
    async fn download_listing_images(&self, id: i64) -> Result<(), String> {
        let storage = self
            .storage_client
            .as_ref()
            .ok_or("Storage not configured")?;
        let images = self
            .repository
            .get_images_for_listing(id)
            .await
            .map_err(|e| e.to_string())?;
        let selected = images
            .iter()
            .filter(|image| image.image_index < self.photo_limit())
            .count();
        if selected == 0 {
            return Err("У объявления нет фотографий".to_owned());
        }
        let mut failures = 0;
        for image in images
            .into_iter()
            .filter(|image| image.image_index < self.photo_limit())
        {
            if image.status == ImageStatus::Downloaded {
                continue;
            }
            if image.download_attempts >= 3 || image.status == ImageStatus::Gone {
                failures += 1;
                continue;
            }
            let download = storage
                .download_and_store_single(id, image.image_index, &image.source_url)
                .await;
            if !download.success {
                failures += 1;
            }
            let status = if download.success {
                ImageStatus::Downloaded
            } else {
                match download.http_status {
                    Some(404) => ImageStatus::NotFound,
                    Some(410) => ImageStatus::Gone,
                    _ if image.download_attempts >= 2 => ImageStatus::Failed,
                    _ => ImageStatus::Pending,
                }
            };
            self.repository
                .update_image_download(
                    image.id,
                    status,
                    download.s3_key.as_deref(),
                    download.http_status.map(i32::from),
                    download.error.as_deref(),
                )
                .await
                .map_err(|e| e.to_string())?;
        }
        if failures > 0 {
            return Err(format!(
                "Не удалось загрузить {failures} фотографий. Подробности доступны в данных изображений."
            ));
        }
        Ok(())
    }

    async fn do_collection(&self) -> Result<CollectionResult, String> {
        let started = Utc::now();
        let state = self
            .repository
            .get_collection_state()
            .await
            .map_err(|e| e.to_string())?;
        let fingerprint =
            serde_json::to_string(&self.config.api.query).map_err(|e| e.to_string())?;
        let since = if state.query_fingerprint.as_deref() == Some(&fingerprint) {
            state
                .last_collection_at
                .unwrap_or(started - Duration::days(self.config.api.initial_load_days as i64))
        } else {
            started - Duration::days(self.config.api.initial_load_days as i64)
        } - Duration::hours(1);
        let mut upper = started;
        let mut result = CollectionResult::default();
        loop {
            let c = &self.config.api.query;
            let query = ListingsQuery {
                category_id: Some(c.category_id.to_string()),
                city: Some(c.city.clone()),
                nedvigimost_type: Some(c.nedvigimost_type.to_string()),
                limit: Some(1000),
                withphone: Some(0),
                withcoords: c.withcoords,
                is_actual: c.is_actual.clone(),
                price1: c.price1,
                price2: c.price2,
                area_from: c.area_from,
                area_to: c.area_to,
                person_type: c.person_type,
                source: c.source.clone(),
                date1: Some(api_time(since)),
                date2: Some(api_time(upper)),
                sort: Some("desc".to_owned()),
                ..Default::default()
            };
            let listings = self
                .api_client
                .get_listings(query)
                .await
                .map_err(|e| e.to_string())?;
            result.total_fetched += listings.len();
            for listing in &listings {
                let (db, urls) = DbListing::from_api_listing(listing, &c.city);
                self.repository
                    .upsert_listing_with_images(&db, &urls)
                    .await
                    .map_err(|e| format!("Save {}: {e}", db.id))?;
                result.total_saved += 1;
            }
            if listings.len() < 1000 {
                break;
            }
            let oldest = listings
                .last()
                .and_then(|l| l.time.as_deref())
                .ok_or("Missing pagination timestamp")?;
            let next = chrono::NaiveDateTime::parse_from_str(oldest, "%Y-%m-%d %H:%M:%S")
                .map_err(|e| e.to_string())?
                .and_utc()
                - Duration::minutes(150)
                + Duration::seconds(1);
            if next >= upper || next < since {
                return Err(
                    "Collection pagination did not advance; checkpoint unchanged".to_owned(),
                );
            }
            upper = next;
        }
        // The upper bound captured before fetching is durable only after every row is saved.
        self.repository
            .update_collection_checkpoint(started, &fingerprint)
            .await
            .map_err(|e| e.to_string())?;
        if let Some(storage) = &self.storage_client {
            self.phase(JobPhase::DownloadingImages, "Загрузка фотографий")
                .await;
            let mut remaining = self.config.api.max_downloads_per_run as usize;
            while remaining > 0 {
                let images = self
                    .repository
                    .get_images_for_download_batch(remaining.min(100) as i64, self.photo_limit())
                    .await
                    .map_err(|e| e.to_string())?;
                if images.is_empty() {
                    break;
                }
                let total = images.len();
                remaining -= total;
                for (index, image) in images.into_iter().enumerate() {
                    let download = storage
                        .download_and_store_single(
                            image.listing_id,
                            image.image_index,
                            &image.source_url,
                        )
                        .await;
                    let status = if download.success {
                        result.total_images_downloaded += 1;
                        ImageStatus::Downloaded
                    } else {
                        match download.http_status {
                            Some(404) => ImageStatus::NotFound,
                            Some(410) => ImageStatus::Gone,
                            _ if image.download_attempts >= 2 => ImageStatus::Failed,
                            _ => ImageStatus::Pending,
                        }
                    };
                    self.repository
                        .update_image_download(
                            image.id,
                            status,
                            download.s3_key.as_deref(),
                            download.http_status.map(i32::from),
                            download.error.as_deref(),
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                    self.progress(index + 1, total).await;
                }
            }
        }
        if self.config.vision.auto_analyze {
            if self.image_analyzer.is_some() && self.storage_client.is_some() {
                result.total_images_analyzed = self.analyze_images(3, None).await?;
            }
            if self.listing_analyzer.is_some() {
                result.total_listings_analyzed = self.analyze_listings(3).await?;
            }
        }
        Ok(result)
    }
    fn photo_limit(&self) -> i32 {
        self.config
            .vision
            .filter
            .max_photos
            .unwrap_or(i32::MAX as usize)
            .min(i32::MAX as usize) as i32
    }
    async fn passes_filter(&self, id: i64) -> Result<bool, String> {
        let l = self
            .repository
            .get_listing_for_analysis(id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Listing not found")?;
        let count = self
            .repository
            .get_image_count_for_listing(id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(self.config.vision.filter.passes_filter(
            count as usize,
            l.price,
            l.area,
            Some(&l.source),
            l.has_coords(),
        ))
    }
    async fn analyze_images(
        &self,
        concurrency: usize,
        listing: Option<i64>,
    ) -> Result<usize, String> {
        let analyzer = self
            .image_analyzer
            .as_ref()
            .ok_or("Image analyzer not configured")?;
        let storage = self
            .storage_client
            .as_ref()
            .ok_or("Storage not configured")?;
        self.phase(JobPhase::AnalyzingImages, "Анализ фотографий")
            .await;
        let mut done = 0;
        let mut errors = Vec::new();
        loop {
            let images = self
                .repository
                .get_images_for_ai_analysis(
                    100,
                    self.config.vision.max_retries as i32,
                    self.photo_limit(),
                    listing,
                )
                .await
                .map_err(|e| e.to_string())?;
            if images.is_empty() {
                break;
            }
            let total = images.len();
            let mut work = stream::iter(images)
                .map(|image| async move {
                    self.repository
                        .begin_image_attempt(image.id)
                        .await
                        .map_err(|e| e.to_string())?;
                    let result = async {
                        if !self.passes_filter(image.listing_id).await? {
                            return Ok(ImageAnalysisResult {
                                image_type: "other".to_owned(),
                                room: None,
                                condition: None,
                                confidence: 0,
                                issues: vec![],
                                description: "Пропущено фильтром анализа".to_owned(),
                            });
                        }
                        let key = image.s3_key.as_deref().ok_or("Image has no storage key")?;
                        let bytes = storage
                            .get_image_by_key(key)
                            .await
                            .map_err(|e| e.to_string())?;
                        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
                        analyzer
                            .analyze_base64(&encoded)
                            .await
                            .map_err(|e| e.to_string())
                    }
                    .await;
                    match result {
                        Ok(analysis) => self
                            .repository
                            .save_image_analysis(image.id, &analysis, analyzer.model())
                            .await
                            .map_err(|e| e.to_string()),
                        Err(e) => {
                            self.repository
                                .fail_image_analysis(image.id, &e)
                                .await
                                .map_err(|e| e.to_string())?;
                            Err(e)
                        }
                    }
                })
                .buffer_unordered(concurrency);
            let mut processed = 0;
            while let Some(result) = work.next().await {
                processed += 1;
                match result {
                    Ok(()) => done += 1,
                    Err(e) => errors.push(e),
                };
                self.progress(processed, total).await;
            }
        }
        if errors.is_empty() {
            Ok(done)
        } else {
            Err(format!(
                "{done} photos analyzed, {} failed; retries delayed: {}",
                errors.len(),
                errors[0]
            ))
        }
    }
    async fn analyze_listing(&self, id: i64, model: Option<&str>) -> Result<(), String> {
        let listing = self
            .repository
            .get_listing_for_analysis(id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Listing not found")?;
        let all = self
            .repository
            .get_images_for_listing(id)
            .await
            .map_err(|e| e.to_string())?;
        if all.iter().any(|i| {
            i.image_index < self.photo_limit()
                && (i.status == ImageStatus::Pending
                    || (i.status == ImageStatus::NotFound && i.download_attempts < 2)
                    || (i.status == ImageStatus::Downloaded
                        && i.ai_analyzed_at.is_none()
                        && i.ai_attempts < self.config.vision.max_retries as i32))
        }) {
            return Err("Фотографии ещё не обработаны".to_owned());
        }
        self.repository
            .begin_listing_attempt(id)
            .await
            .map_err(|e| e.to_string())?;
        let result: Result<(ListingAnalysisResult, String), String> = async {
            if !self.passes_filter(id).await? {
                return Ok((
                    ListingAnalysisResult::unknown(
                        "Недостаточно данных: объявление не прошло фильтр анализа",
                    ),
                    "none".to_owned(),
                ));
            }
            let custom;
            let analyzer = if let Some(model) = model {
                custom = ListingAnalyzer::new(
                    self.vision_api_key
                        .clone()
                        .ok_or("Model key not configured")?,
                    model.to_owned(),
                    self.config.vision.api_base_url.clone(),
                );
                &custom
            } else {
                self.listing_analyzer
                    .as_ref()
                    .ok_or("Listing analyzer not configured")?
                    .as_ref()
            };
            let mut images = self
                .repository
                .get_listing_image_analyses(id)
                .await
                .map_err(|e| e.to_string())?;
            images.retain(|i| i.image_index < self.photo_limit());
            Ok((
                analyzer
                    .analyze(&listing, &images)
                    .await
                    .map_err(|e| e.to_string())?,
                analyzer.model().to_owned(),
            ))
        }
        .await;
        match result {
            Ok((analysis, model)) => self
                .repository
                .save_listing_analysis(id, &analysis, &model)
                .await
                .map_err(|e| e.to_string()),
            Err(e) => {
                self.repository
                    .fail_listing_analysis(id, &e)
                    .await
                    .map_err(|e| e.to_string())?;
                Err(e)
            }
        }
    }
    async fn analyze_listings(&self, concurrency: usize) -> Result<usize, String> {
        self.phase(JobPhase::AnalyzingListings, "Анализ объявлений")
            .await;
        let mut done = 0;
        let mut errors = Vec::new();
        loop {
            let ids = self
                .repository
                .get_listings_for_second_layer_analysis(
                    100,
                    self.config.vision.max_retries as i32,
                    self.photo_limit(),
                )
                .await
                .map_err(|e| e.to_string())?;
            if ids.is_empty() {
                break;
            }
            let total = ids.len();
            let mut work = stream::iter(ids)
                .map(|id| self.analyze_listing(id, None))
                .buffer_unordered(concurrency);
            let mut processed = 0;
            while let Some(result) = work.next().await {
                processed += 1;
                match result {
                    Ok(()) => done += 1,
                    Err(e) => errors.push(e),
                };
                self.progress(processed, total).await;
            }
        }
        if errors.is_empty() {
            Ok(done)
        } else {
            Err(format!(
                "{done} listings analyzed, {} failed; retries delayed: {}",
                errors.len(),
                errors[0]
            ))
        }
    }
    async fn do_sync(&self, hours: u32) -> Result<SyncResult, String> {
        let through = Utc::now();
        let state = self
            .repository
            .get_collection_state()
            .await
            .map_err(|e| e.to_string())?;
        let since = state
            .last_sync_at
            .unwrap_or(through - Duration::hours(hours as i64))
            - Duration::hours(1);
        // Capture both groups before marking chunks complete. New records always receive their history.
        let pending = self
            .repository
            .feed_sync_ids(true)
            .await
            .map_err(|e| e.to_string())?;
        let existing = self
            .repository
            .feed_sync_ids(false)
            .await
            .map_err(|e| e.to_string())?;
        let mut result = SyncResult::default();
        let total = pending.len() + existing.len();
        let mut processed = 0;
        for (ids, from) in [(&pending, chrono::DateTime::UNIX_EPOCH), (&existing, since)] {
            for chunk in ids.chunks(250) {
                let checks = self
                    .api_client
                    .get_check_feed_between(from, through, chunk)
                    .await
                    .map_err(|e| e.to_string())?;
                let applied = self.reconcile_feed_checks(checks).await?;
                result.total_checked += applied.total_checked;
                result.marked_inactive += applied.marked_inactive;
                result.still_active += applied.still_active;
                self.repository
                    .mark_feed_history_synced(chunk, through)
                    .await
                    .map_err(|e| e.to_string())?;
                processed += chunk.len();
                self.progress(processed, total).await;
            }
        }
        self.repository
            .update_sync_checkpoint(through)
            .await
            .map_err(|e| e.to_string())?;
        Ok(result)
    }

    async fn reconcile_feed_checks(
        &self,
        checks: Vec<crate::ads_api::models::CheckFeedItem>,
    ) -> Result<SyncResult, String> {
        let mut newest = std::collections::BTreeMap::new();
        for check in checks {
            let time = check
                .time
                .as_deref()
                .and_then(parse_moscow)
                .ok_or("Invalid actuality timestamp; checkpoint unchanged")?;
            if time > Utc::now() + Duration::minutes(5) {
                return Err("Actuality timestamp is in the future".into());
            }
            // Although documented as 0/1, the live feed can contain 11 (a scrape, not a check).
            if !matches!(check.is_actual, 0 | 1) {
                continue;
            }
            let entry = newest.entry(check.id).or_insert((time, check.is_actual));
            if time > entry.0 {
                *entry = (time, check.is_actual);
            }
        }
        let mut result = SyncResult::default();
        for (id, (time, status)) in newest {
            let Some(local) = self
                .repository
                .get_listing_by_id(id)
                .await
                .map_err(|e| e.to_string())?
            else {
                continue;
            };
            if local.source_time.is_some_and(|v| v > time)
                || local.actuality_checked_at.is_some_and(|v| v >= time)
            {
                continue;
            }
            // A historical removal may precede a republication absent from our local snapshot.
            // Refresh the latest provider version before accepting either historical outcome.
            let Some((source, source_id)) =
                crate::actuality::source_identity(&local.source, &local.url)
            else {
                continue;
            };
            let Some(latest) = self
                .api_client
                .get_latest_source_listing(source, &source_id)
                .await
                .map_err(|e| e.to_string())?
            else {
                continue;
            };
            if latest.id != id {
                return Err(format!("Provider returned a different listing for {id}"));
            }
            let (snapshot, photos) = DbListing::from_api_listing(&latest, &local.city);
            if snapshot.source_time.is_none() {
                continue;
            }
            self.repository
                .upsert_listing_with_images(&snapshot, &photos)
                .await
                .map_err(|e| e.to_string())?;
            if self
                .repository
                .apply_actuality(id, status == 1, time)
                .await
                .map_err(|e| e.to_string())?
            {
                result.total_checked += 1;
                if status == 0 {
                    result.marked_inactive += 1;
                } else {
                    result.still_active += 1;
                }
            }
        }
        Ok(result)
    }
    pub async fn check_actuality_by_ids(
        &self,
        ids: &[i64],
    ) -> Result<Vec<CheckActualityResult>, String> {
        let _permit = self.reserve()?;
        let rows = self
            .api_client
            .check_actuality_by_ids(ids)
            .await
            .map_err(|e| e.to_string())?;
        self.reconcile_feed_checks(rows.clone()).await?;
        let out = rows
            .into_iter()
            .map(|row| CheckActualityResult {
                id: row.id,
                is_actual: row.is_actual,
                checked_at: row.time,
            })
            .collect();
        Ok(out)
    }

    pub async fn import_recorded_checks(
        &self,
        checks: Vec<crate::ads_api::models::CheckFeedItem>,
    ) -> Result<SyncResult, String> {
        self.perform(
            self.reserve()?,
            JobPhase::SyncingActuality,
            self.reconcile_feed_checks(checks),
        )
        .await
    }

    pub async fn request_reparse(&self, id: i64) -> Result<bool, String> {
        self.perform(self.reserve()?, JobPhase::SyncingActuality, async {
            if let Some(queued) = self
                .repository
                .recent_reparse_request(id)
                .await
                .map_err(|e| e.to_string())?
            {
                return Ok(queued);
            }
            let listing = self
                .repository
                .get_listing_by_id(id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or("Listing not found")?;
            if !matches!(
                listing.source.as_str(),
                "avito.ru" | "cian.ru" | "realty.yandex.ru"
            ) || crate::actuality::source_identity(&listing.source, &listing.url).is_none()
            {
                return Err("Повторный сбор этого источника не поддерживается".into());
            }
            let queued = self
                .api_client
                .request_reparse(&listing.url)
                .await
                .map_err(|e| e.to_string())?;
            self.repository
                .save_reparse_request(id, queued)
                .await
                .map_err(|e| e.to_string())?;
            Ok(queued)
        })
        .await
    }
}
fn api_time(time: DateTime<Utc>) -> String {
    (time + Duration::minutes(150))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}
fn parse_moscow(time: &str) -> Option<DateTime<Utc>> {
    chrono::NaiveDateTime::parse_from_str(time, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|v| v.and_utc() - Duration::hours(3))
}
#[derive(Debug, Default, Serialize)]
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
        serde_json::to_value(self).expect("collection result serializes")
    }
}
#[derive(Debug, Default, Serialize)]
pub struct SyncResult {
    pub total_checked: usize,
    pub marked_inactive: usize,
    pub still_active: usize,
}
impl SyncResult {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("sync result serializes")
    }
}
#[derive(Debug, Serialize)]
pub struct CheckActualityResult {
    pub id: i64,
    pub is_actual: i32,
    pub checked_at: Option<String>,
}
