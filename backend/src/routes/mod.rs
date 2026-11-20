use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{
        Path, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
};
use futures::{SinkExt, StreamExt};
use serde_json::json;

use crate::db::{NewCollectionConfig, Repository};
use crate::jobs::{Collector, Job};
use crate::storage::StorageClient;

#[derive(Clone)]
pub struct AppState {
    pub repository: Arc<Repository>,
    pub collector: Arc<Collector>,
    pub storage: Option<Arc<StorageClient>>,
    pub config: Arc<crate::config::Config>,
}

pub fn create_router(state: AppState) -> Router {
    let app = Router::new()
        .route("/health", get(health))
        .route("/api/collect", post(trigger_collection))
        .route("/api/collect/status", get(collection_status))
        .route("/api/status", get(get_job_status))
        .route("/api/ws/status", get(ws_status))
        .route("/api/configs", get(list_configs))
        .route("/api/configs", post(create_config))
        .route("/api/configs/{id}", delete(delete_config))
        .route("/api/configs/{id}/toggle", post(toggle_config))
        .route("/api/stats", get(get_stats))
        .route("/api/settings", get(get_public_settings))
        // Listings endpoints
        .route("/api/listings", get(get_listings))
        .route("/api/images/{id}/content", get(image_content))
        .route(
            "/api/listings/{id}/review",
            get(get_review).put(save_review),
        )
        .route("/api/listings/{id}/price-history", get(price_history))
        .route("/api/listings/{id}", get(get_listing))
        .route("/api/listings/{id}/images", get(get_listing_images))
        .route(
            "/api/listings/{id}/mark-inactive",
            post(mark_listing_inactive),
        )
        // Analysis endpoints
        .route("/api/analyze/images", post(trigger_image_analysis))
        .route("/api/analyze/listings", post(trigger_listing_analysis))
        // Single listing analysis endpoints
        .route(
            "/api/listings/{id}/analyze/images",
            post(analyze_listing_images),
        )
        .route("/api/listings/{id}/analyze", post(analyze_single_listing))
        // Actuality sync
        .route("/api/sync/actuality", post(trigger_actuality_sync))
        .route("/api/check-actuality", post(check_actuality_by_ids))
        .route("/api/check-sources", post(check_sources))
        .route("/api/listings/{id}/request-reparse", post(request_reparse))
        .route("/api/listings/{id}/check-source", post(check_single_source));
    let app = if let Some(directory) = state
        .config
        .server
        .frontend_dir
        .as_ref()
        .filter(|path| path.join("index.html").is_file())
    {
        app.fallback_service(
            tower_http::services::ServeDir::new(directory).not_found_service(
                tower_http::services::ServeFile::new(directory.join("index.html")),
            ),
        )
    } else {
        app.route("/", get(root))
    };
    app.with_state(state)
}

async fn root() -> &'static str {
    "Realty Aggregator API"
}

async fn health() -> &'static str {
    "OK"
}

async fn request_reparse(State(state): State<AppState>, Path(id): Path<i64>) -> impl IntoResponse {
    match state.collector.request_reparse(id).await {
        Ok(queued) => (
            StatusCode::OK,
            Json(json!({"queued": queued, "message": if queued {
            "Запрос на повторный сбор принят. Срок обработки не гарантирован; это не подтверждение актуальности."
        } else { "Провайдер уже собирал объявление за последние 31 день. Новый запрос в очередь не добавлен." }})),
        ),
        Err(error) => (
            if error == "Job already in progress" {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_GATEWAY
            },
            Json(json!({"error": error})),
        ),
    }
}

async fn trigger_collection(State(state): State<AppState>) -> impl IntoResponse {
    start_job(state, Job::Collect)
}

async fn collection_status(State(state): State<AppState>) -> impl IntoResponse {
    let is_running = state.collector.is_running().await;
    Json(json!({"is_running": is_running}))
}

async fn get_job_status(State(state): State<AppState>) -> impl IntoResponse {
    let status = state.collector.get_status().await;
    Json(json!(status))
}

async fn ws_status(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_status(socket, state))
}

async fn handle_ws_status(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    // Subscribe to status updates
    let mut rx = state.collector.subscribe();

    // Send current status immediately
    let current_status = state.collector.get_status().await;
    if let Ok(json) = serde_json::to_string(&current_status)
        && sender.send(Message::Text(json.into())).await.is_err()
    {
        return;
    }

    // Spawn task to handle incoming messages (for ping/pong and close)
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if matches!(msg, Message::Close(_)) {
                break;
            }
        }
    });

    // Send status updates from broadcast channel
    let mut send_task = tokio::spawn(async move {
        loop {
            let status = match rx.recv().await {
                Ok(status) => status,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    state.collector.get_status().await
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            if let Ok(json) = serde_json::to_string(&status)
                && sender.send(Message::Text(json.into())).await.is_err()
            {
                break;
            }
        }
    });

    // Wait for either task to complete
    tokio::select! {
        _ = &mut recv_task => {
            send_task.abort();
        }
        _ = &mut send_task => {
            recv_task.abort();
        }
    }
}

async fn list_configs(State(state): State<AppState>) -> impl IntoResponse {
    match state.repository.get_all_configs().await {
        Ok(configs) => (StatusCode::OK, Json(json!(configs))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ),
    }
}

async fn create_config(
    State(state): State<AppState>,
    Json(payload): Json<NewCollectionConfig>,
) -> impl IntoResponse {
    match state.repository.create_config(payload).await {
        Ok(config) => (StatusCode::CREATED, Json(json!(config))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ),
    }
}

async fn delete_config(State(state): State<AppState>, Path(id): Path<i32>) -> impl IntoResponse {
    match state.repository.delete_config(id).await {
        Ok(true) => (StatusCode::OK, Json(json!({"deleted": true}))),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Config not found"})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ),
    }
}

async fn toggle_config(State(state): State<AppState>, Path(id): Path<i32>) -> impl IntoResponse {
    let configs = match state.repository.get_all_configs().await {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            );
        }
    };

    let current = configs.iter().find(|c| c.id == id);
    let new_state = !current.map(|c| c.is_active.unwrap_or(true)).unwrap_or(true);

    match state.repository.toggle_config(id, new_state).await {
        Ok(Some(config)) => (StatusCode::OK, Json(json!(config))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Config not found"})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ),
    }
}

async fn get_stats(State(state): State<AppState>) -> impl IntoResponse {
    let cutoff =
        chrono::Utc::now() - chrono::Duration::hours(state.config.actuality.recheck_hours as i64);
    match tokio::try_join!(
        state.repository.get_listings_count(),
        state.repository.get_active_listings_count(),
        state.repository.get_images_stats(),
        state.repository.get_analysis_stats(),
        state.repository.get_all_configs(),
        state.repository.get_availability_stats(cutoff),
        state.repository.get_collection_state()
    ) {
        Ok((total, active, images, analysis, configs, availability, collection)) => (
            StatusCode::OK,
            Json(json!({
                "total_listings": total, "active_listings": active, "images": images, "analysis": analysis,
                "confirmed_listings": availability.iter().map(|s| s.confirmed).sum::<i64>(),
                "removed_listings": availability.iter().map(|s| s.removed).sum::<i64>(),
                "unconfirmed_listings": total - availability.iter().map(|s| s.confirmed + s.removed).sum::<i64>(),
                "availability": availability, "actuality_synced_at": collection.last_sync_at,
                "configs_count": configs.len(), "active_configs_count": configs.iter().filter(|c| c.is_active.unwrap_or(false)).count()
            })),
        ),
        Err(error) => {
            tracing::error!(%error, "Unable to read stats");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Unable to read stats"})),
            )
        }
    }
}

#[derive(serde::Deserialize)]
struct AnalyzeAllParams {
    #[serde(default = "default_concurrency")]
    concurrency: usize,
}

fn default_concurrency() -> usize {
    3
}

async fn trigger_image_analysis(
    State(state): State<AppState>,
    Json(params): Json<Option<AnalyzeAllParams>>,
) -> impl IntoResponse {
    start_job(
        state,
        Job::Images {
            concurrency: params.map(|p| p.concurrency).unwrap_or(3),
        },
    )
}

async fn trigger_listing_analysis(
    State(state): State<AppState>,
    Json(params): Json<Option<AnalyzeAllParams>>,
) -> impl IntoResponse {
    start_job(
        state,
        Job::Listings {
            concurrency: params.map(|p| p.concurrency).unwrap_or(3),
        },
    )
}

#[derive(serde::Deserialize)]
struct ListingsQuery {
    min_area: Option<f64>,
    move_in_ready: Option<bool>,
    favorites_only: Option<bool>,
    hide_rejected: Option<bool>,
    verified_only: Option<bool>,
    page: Option<i64>,
    per_page: Option<i64>,
    min_price: Option<i64>,
    max_price: Option<i64>,
    rooms: Option<i32>,
    analyzed_only: Option<bool>,
    hide_shell: Option<bool>,
    active_only: Option<bool>,
    // New filters
    property_type: Option<String>, // "Новостройка", "Вторичка", or empty for all
    is_shell: Option<bool>,        // true/false, or None for all
    min_condition: Option<i32>,
    max_condition: Option<i32>,
}

async fn get_listings(
    State(state): State<AppState>,
    Query(params): Query<ListingsQuery>,
) -> impl IntoResponse {
    let page = params.page.unwrap_or(1).clamp(1, 1_000_000);
    let per_page = params.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    match state
        .repository
        .get_listings_paginated(
            offset,
            per_page,
            crate::db::repository::ListingsFilter {
                min_price: params.min_price,
                max_price: params.max_price,
                rooms: params.rooms,
                analyzed_only: params.analyzed_only.unwrap_or(false),
                hide_shell: params.hide_shell.unwrap_or(false) && params.is_shell != Some(true),
                active_only: params.active_only.unwrap_or(true),
                property_type: params.property_type,
                is_shell: params.is_shell,
                min_condition: params.min_condition,
                max_condition: params.max_condition,
                min_area: params.min_area,
                move_in_ready: params.move_in_ready,
                favorites_only: params.favorites_only.unwrap_or(false),
                hide_rejected: params.hide_rejected.unwrap_or(false),
                verified_after: params.verified_only.unwrap_or(false).then(|| {
                    chrono::Utc::now()
                        - chrono::Duration::hours(state.config.actuality.recheck_hours as i64)
                }),
            },
        )
        .await
    {
        Ok((listings, total)) => (
            StatusCode::OK,
            Json(json!({
                "listings": listings.iter().map(|l| listing_response(l, &state.config)).collect::<Vec<_>>(),
                "total": total,
                "page": page,
                "per_page": per_page
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ),
    }
}

async fn get_listing(State(state): State<AppState>, Path(id): Path<i64>) -> impl IntoResponse {
    match state.repository.get_listing_by_id(id).await {
        Ok(Some(listing)) => (
            StatusCode::OK,
            Json(listing_response(&listing, &state.config)),
        ),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Listing not found"})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ),
    }
}

fn listing_response(
    listing: &crate::db::DbListing,
    config: &crate::config::Config,
) -> serde_json::Value {
    let now = chrono::Utc::now();
    let expires = listing
        .actuality_checked_at
        .map(|time| time + chrono::Duration::hours(config.actuality.recheck_hours as i64));
    let confirmed = listing.is_active == Some(true)
        && matches!(
            listing.actuality_origin.as_str(),
            "source_page" | "check_feed" | "manual"
        )
        && expires.is_some_and(|v| v >= now)
        && listing.source_time.is_none_or(|source| {
            listing
                .actuality_checked_at
                .is_some_and(|check| check >= source)
        });
    let status = if listing.is_active == Some(false) {
        "removed"
    } else if confirmed {
        "confirmed"
    } else if listing
        .source_time
        .is_some_and(|v| v >= now - chrono::Duration::days(7))
    {
        "recently_seen"
    } else {
        "unconfirmed"
    };
    let mut value = json!(listing);
    value["availability_state"] = json!(status);
    value["confirmation_expires_at"] = json!(expires);
    value
}

async fn get_listing_images(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.repository.get_images_for_listing(id).await {
        Ok(images) => (StatusCode::OK, Json(json!(images))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ),
    }
}

async fn analyze_listing_images(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    start_job(state, Job::ListingImages(id))
}

#[derive(serde::Deserialize)]
struct AnalyzeListingParams {
    model: Option<String>,
}

async fn analyze_single_listing(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(params): Json<Option<AnalyzeListingParams>>,
) -> impl IntoResponse {
    start_job(
        state,
        Job::Listing {
            id,
            model: params.and_then(|p| p.model),
        },
    )
}

#[derive(serde::Deserialize)]
struct SyncQuery {
    hours: Option<u32>,
}

async fn trigger_actuality_sync(
    State(state): State<AppState>,
    Query(params): Query<SyncQuery>,
) -> impl IntoResponse {
    start_job(
        state,
        Job::Sync {
            hours: params.hours.unwrap_or(168),
        },
    )
}

#[derive(serde::Deserialize)]
struct CheckActualityParams {
    ids: Vec<i64>,
}

async fn check_actuality_by_ids(
    State(state): State<AppState>,
    Json(params): Json<CheckActualityParams>,
) -> impl IntoResponse {
    if params.ids.is_empty() || params.ids.len() > 250 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Expected 1..250 IDs"})),
        );
    }
    match state.collector.check_actuality_by_ids(&params.ids).await {
        Ok(results) => (
            StatusCode::OK,
            Json(json!({
                "requested": params.ids.len(),
                "found": results.len(),
                "results": results
            })),
        ),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))),
    }
}

async fn mark_listing_inactive(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.repository.mark_listing_inactive(id).await {
        Ok(true) => (
            StatusCode::OK,
            Json(json!({"success": true, "id": id, "is_active": false})),
        ),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Listing not found"})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ),
    }
}

fn start_job(state: AppState, job: Job) -> (StatusCode, Json<serde_json::Value>) {
    match state.collector.start(job) {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(json!({"message": "Задача запущена"})),
        ),
        Err(error) => (
            if error == "Job already in progress" {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            },
            Json(json!({"error": error})),
        ),
    }
}
async fn image_content(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> axum::response::Response {
    let Some(storage) = &state.storage else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    match state.repository.get_image_by_id(id).await {
        Ok(Some(key)) => match storage.get_image_by_key(&key).await {
            Ok(bytes) => (
                [
                    (axum::http::header::CONTENT_TYPE, "image/jpeg"),
                    (axum::http::header::CACHE_CONTROL, "private, no-cache"),
                ],
                bytes,
            )
                .into_response(),
            Err(_) => StatusCode::BAD_GATEWAY.into_response(),
        },
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
async fn get_review(State(state): State<AppState>, Path(id): Path<i64>) -> impl IntoResponse {
    match state.repository.get_review(id).await {
        Ok(value) => (StatusCode::OK, Json(json!(value))),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"Database error"})),
        ),
    }
}
async fn save_review(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(review): Json<crate::db::models::ListingReview>,
) -> impl IntoResponse {
    if ![
        "new",
        "contact",
        "visit",
        "documents",
        "approved",
        "rejected",
    ]
    .contains(&review.stage.as_str())
        || review.notes.len() > 20000
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"Invalid stage or notes too long"})),
        );
    }
    match state.repository.save_review(id, &review).await {
        Ok(()) => (StatusCode::OK, Json(json!(review))),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"Unable to save review"})),
        ),
    }
}
async fn price_history(State(state): State<AppState>, Path(id): Path<i64>) -> impl IntoResponse {
    match state.repository.get_price_history(id).await {
        Ok(value) => (StatusCode::OK, Json(json!(value))),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"Database error"})),
        ),
    }
}

async fn check_sources(
    State(state): State<AppState>,
    Json(params): Json<CheckActualityParams>,
) -> impl IntoResponse {
    start_job(state, Job::CheckSources { ids: params.ids })
}

async fn check_single_source(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    start_job(state, Job::CheckSources { ids: vec![id] })
}

async fn get_public_settings(State(state): State<AppState>) -> impl IntoResponse {
    Json(
        json!({"models": state.config.vision.allowed_models, "default_model": state.config.vision.listing_model,
        "source_check_ttl_hours": state.config.actuality.recheck_hours}),
    )
}
