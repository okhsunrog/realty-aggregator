use std::sync::Arc;

use sqlx::postgres::PgPoolOptions;
use std::net::SocketAddr;
use tokio_shutdown::Shutdown;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use backend::{
    ads_api::AdsApiClient,
    config::Config,
    db::Repository,
    jobs::{Collector, Job},
    routes::{AppState, create_router},
    storage::StorageClient,
    vision::{ImageAnalyzer, ListingAnalyzer},
};

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
    tracing::info!(interface = ?std::env::var("SOURCE_NETWORK_INTERFACE").ok(), "Source network configuration");

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

    let api_client = Arc::new(
        AdsApiClient::new(ads_api_user, ads_api_token)
            .with_request_delay(config.api.request_delay_seconds),
    );
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
    if let Some(storage) = storage_client.clone() {
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

    let mut scheduled_tasks = Vec::new();
    let scheduled_collector = collector.clone();
    let period = std::time::Duration::from_secs(config.api.collect_interval_minutes as u64 * 60);
    scheduled_tasks.push(tokio::spawn(async move {
        let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(error) = scheduled_collector.run_scheduled(Job::Collect).await {
                tracing::warn!(%error, "Scheduled collection failed");
            }
        }
    }));
    let (startup_sync_done, startup_sync_ready) = tokio::sync::oneshot::channel::<()>();
    if config.api.sync.enabled {
        let scheduled_collector = collector.clone();
        let hours = config.api.sync.interval_hours;
        let period = std::time::Duration::from_secs(hours as u64 * 3600);
        scheduled_tasks.push(tokio::spawn(async move {
            let mut startup_sync_done = Some(startup_sync_done);
            let mut interval = tokio::time::interval_at(tokio::time::Instant::now(), period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if let Err(error) = scheduled_collector.run_scheduled(Job::Sync { hours }).await {
                    tracing::warn!(%error, "Scheduled actuality sync failed");
                }
                if let Some(done) = startup_sync_done.take() {
                    let _ = done.send(());
                }
            }
        }));
    } else {
        drop(startup_sync_done);
    }

    if config.actuality.enabled {
        let source_collector = collector.clone();
        let period = std::time::Duration::from_secs(config.actuality.interval_minutes as u64 * 60);
        scheduled_tasks.push(tokio::spawn(async move {
            // Import provider removals before spending requests on original pages.
            let _ = startup_sync_ready.await;
            let mut interval = tokio::time::interval_at(tokio::time::Instant::now(), period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if let Err(error) = source_collector
                    .run_scheduled(Job::CheckSources { ids: Vec::new() })
                    .await
                {
                    tracing::warn!(%error, "Source checks failed");
                }
            }
        }));
    }

    // Setup routes
    let state = AppState {
        repository,
        collector,
        storage: storage_client,
        config: config.clone(),
    };

    let origins: Vec<axum::http::HeaderValue> = config
        .server
        .allowed_origins
        .iter()
        .map(|origin| origin.parse())
        .collect::<Result<_, _>>()?;
    let cors = CorsLayer::new()
        .allow_origin(origins)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = create_router(state)
        .layer(axum::middleware::from_fn_with_state(
            config.server.allowed_origins.clone(),
            check_origin,
        ))
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let addr: SocketAddr = std::env::var("SERVER_BIND")
        .unwrap_or_else(|_| config.server.bind.clone())
        .parse()?;
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

    // Stop scheduled tasks
    tracing::info!("Stopping scheduled tasks...");
    for task in scheduled_tasks {
        task.abort();
    }

    tracing::info!("Graceful shutdown complete");
    Ok(())
}

async fn check_origin(
    axum::extract::State(allowed): axum::extract::State<Vec<String>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let sensitive = !matches!(
        *request.method(),
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    ) || request.uri().path() == "/api/ws/status";
    if sensitive
        && request
            .headers()
            .get(axum::http::header::ORIGIN)
            .is_some_and(|origin| {
                origin.to_str().map_or(true, |origin| {
                    !allowed.iter().any(|allowed| allowed == origin)
                })
            })
    {
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}
