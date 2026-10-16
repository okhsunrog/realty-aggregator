use std::sync::Arc;

use sqlx::postgres::PgPoolOptions;
use std::net::SocketAddr;
use tokio_cron_scheduler::{Job, JobScheduler};
use tokio_shutdown::Shutdown;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

mod ads_api;
mod config;
mod db;
mod jobs;
mod routes;
mod storage;
mod vision;

use ads_api::AdsApiClient;
use config::Config;
use db::Repository;
use jobs::Collector;
use routes::{AppState, create_router};
use storage::StorageClient;
use vision::{ImageAnalyzer, ListingAnalyzer};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "backend=debug,tower_http=debug".into()),
        )
        .init();

    // Load configuration
    let config = Arc::new(Config::load_default().expect("Failed to load config.toml"));
    tracing::info!("Configuration loaded from config.toml");

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://realty:realty_dev_password@localhost:5432/realty".into());

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    tracing::info!("Connected to database");

    // Run migrations
    sqlx::migrate!("./migrations").run(&pool).await?;
    tracing::info!("Migrations applied");

    // Initialize API client
    let ads_api_user = std::env::var("ADS_API_USER").unwrap_or_default();
    let ads_api_token = std::env::var("ADS_API_TOKEN").unwrap_or_default();

    if ads_api_user.is_empty() || ads_api_token.is_empty() {
        tracing::warn!("ADS_API_USER or ADS_API_TOKEN not set. Collection will fail.");
    }

    let api_client = Arc::new(AdsApiClient::new(ads_api_user, ads_api_token));
    let repository = Arc::new(Repository::new(pool));

    // Initialize Storage Client (optional)
    let storage_client = match StorageClient::from_env(config.storage.image_path_template.clone()) {
        Ok(client) => {
            // Ensure bucket exists
            if let Err(e) = client.ensure_bucket_exists().await {
                tracing::warn!(
                    "Failed to ensure MinIO bucket exists: {}. Image storage disabled.",
                    e
                );
                None
            } else {
                tracing::info!("MinIO storage client initialized");
                Some(Arc::new(client))
            }
        }
        Err(e) => {
            tracing::warn!("MinIO not configured: {}. Image storage disabled.", e);
            None
        }
    };

    // Initialize Vision Analyzers (optional)
    let vision_api_key = std::env::var(&config.vision.api_key_env)
        .ok()
        .filter(|key| !key.trim().is_empty());
    let (image_analyzer, listing_analyzer) = match &vision_api_key {
        Some(key) => {
            tracing::info!(
                "Image analyzer initialized with model: {}",
                config.vision.image_model
            );
            tracing::info!(
                "Listing analyzer initialized with model: {}",
                config.vision.listing_model
            );
            (
                Some(Arc::new(ImageAnalyzer::new(
                    key.clone(),
                    config.vision.image_model.clone(),
                    config.vision.api_base_url.clone(),
                ))),
                Some(Arc::new(ListingAnalyzer::new(
                    key.clone(),
                    config.vision.listing_model.clone(),
                    config.vision.api_base_url.clone(),
                ))),
            )
        }
        None => {
            tracing::warn!(
                key_env = %config.vision.api_key_env,
                "API key not set. Vision analysis disabled."
            );
            (None, None)
        }
    };

    let mut collector = Collector::new(api_client.clone(), repository.clone(), config.clone());
    if let Some(storage) = storage_client {
        collector = collector.with_storage_client(storage);
    }
    if let Some(analyzer) = image_analyzer {
        collector = collector.with_image_analyzer(analyzer);
    }
    if let Some(analyzer) = listing_analyzer {
        collector = collector.with_listing_analyzer(analyzer);
    }
    if let Some(key) = vision_api_key {
        collector = collector.with_vision_api_key(key);
    }
    let collector = Arc::new(collector);

    // Setup scheduler
    let mut scheduler = JobScheduler::new().await?;

    // Collection job - run at configured interval
    let collect_interval = config.api.collect_interval_minutes;
    let collector_for_job = collector.clone();

    // Build cron expression for the collection interval
    // For intervals like 60 minutes, run every hour at minute 0
    let collection_cron = if collect_interval >= 60 {
        "0 0 * * * *".to_string() // Every hour
    } else {
        format!("0 */{} * * * *", collect_interval) // Every N minutes
    };

    let collection_job = Job::new_async(collection_cron.as_str(), move |_uuid, _l| {
        let collector = collector_for_job.clone();
        Box::pin(async move {
            tracing::info!("Starting scheduled collection");
            match collector.run_collection().await {
                Ok(result) => {
                    tracing::info!("Scheduled collection completed: {:?}", result);
                }
                Err(e) => {
                    tracing::error!("Scheduled collection failed: {}", e);
                }
            }
        })
    })?;

    scheduler.add(collection_job).await?;
    tracing::info!(
        "Collection job scheduled with cron: {} (every {} minutes)",
        collection_cron,
        collect_interval
    );

    // Actuality sync job - run daily if enabled
    if config.api.sync.enabled {
        let sync_interval_hours = config.api.sync.interval_hours;
        let collector_for_sync = collector.clone();

        // Run daily at 3:00 AM
        let sync_job = Job::new_async("0 0 3 * * *", move |_uuid, _l| {
            let collector = collector_for_sync.clone();
            let hours = sync_interval_hours;
            Box::pin(async move {
                tracing::info!("Starting scheduled actuality sync");
                match collector.sync_actuality(hours).await {
                    Ok(result) => {
                        tracing::info!(
                            "Actuality sync completed: {} checked, {} marked inactive",
                            result.total_checked,
                            result.marked_inactive
                        );
                    }
                    Err(e) => {
                        tracing::error!("Actuality sync failed: {}", e);
                    }
                }
            })
        })?;

        scheduler.add(sync_job).await?;
        tracing::info!(
            "Actuality sync job scheduled to run daily at 3:00 AM (checking last {} hours)",
            sync_interval_hours
        );
    }

    scheduler.start().await?;
    tracing::info!("Scheduler started");

    // Setup routes
    let state = AppState {
        repository,
        collector,
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = create_router(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    tracing::info!("Server listening on {}", addr);

    // Setup graceful shutdown
    let shutdown = Shutdown::new().expect("Failed to create shutdown handler");
    let shutdown_signal = shutdown.handle();

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal.await;
            tracing::info!("Shutdown signal received, starting graceful shutdown...");
        })
        .await?;

    // Shutdown scheduler
    tracing::info!("Shutting down scheduler...");
    scheduler.shutdown().await?;

    tracing::info!("Graceful shutdown complete");
    Ok(())
}
