use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State, WebSocketUpgrade, ws::{Message, WebSocket}},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
};
use futures::{SinkExt, StreamExt};
use serde_json::json;

use crate::db::{NewCollectionConfig, Repository};
use crate::jobs::Collector;

#[derive(Clone)]
pub struct AppState {
    pub repository: Arc<Repository>,
    pub collector: Arc<Collector>,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(root))
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
        // Listings endpoints
        .route("/api/listings", get(get_listings))
        .route("/api/listings/{id}", get(get_listing))
        .route("/api/listings/{id}/images", get(get_listing_images))
        .route("/api/listings/{id}/mark-inactive", post(mark_listing_inactive))
        // Analysis endpoints
        .route("/api/analyze/images", post(trigger_image_analysis))
        .route("/api/analyze/listings", post(trigger_listing_analysis))
        // Single listing analysis endpoints
        .route("/api/listings/{id}/analyze/images", post(analyze_listing_images))
        .route("/api/listings/{id}/analyze", post(analyze_single_listing))
        // Actuality sync
        .route("/api/sync/actuality", post(trigger_actuality_sync))
        .route("/api/check-actuality", post(check_actuality_by_ids))
        .with_state(state)
}

async fn root() -> &'static str {
    "Realty Aggregator API"
}

async fn health() -> &'static str {
    "OK"
}

async fn trigger_collection(State(state): State<AppState>) -> impl IntoResponse {
    if state.collector.is_running().await {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": "Collection already in progress"})),
        );
    }

    let collector = state.collector.clone();
    tokio::spawn(async move {
        match collector.run_collection().await {
            Ok(result) => {
                tracing::info!("Collection completed: {:?}", result);
            }
            Err(e) => {
                tracing::error!("Collection failed: {}", e);
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(json!({"message": "Collection started"})),
    )
}

async fn collection_status(State(state): State<AppState>) -> impl IntoResponse {
    let is_running = state.collector.is_running().await;
    Json(json!({"is_running": is_running}))
}

async fn get_job_status(State(state): State<AppState>) -> impl IntoResponse {
    let status = state.collector.get_status().await;
    Json(json!(status))
}

async fn ws_status(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_status(socket, state))
}

async fn handle_ws_status(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    // Subscribe to status updates
    let mut rx = state.collector.subscribe();

    // Send current status immediately
    let current_status = state.collector.get_status().await;
    if let Ok(json) = serde_json::to_string(&current_status) {
        if sender.send(Message::Text(json.into())).await.is_err() {
            return;
        }
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
        while let Ok(status) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&status) {
                if sender.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
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
    let total = state.repository.get_listings_count().await.unwrap_or(0);
    let active = state
        .repository
        .get_active_listings_count()
        .await
        .unwrap_or(0);

    let images_stats = state.repository.get_images_stats().await.ok();
    let analysis_stats = state.repository.get_analysis_stats().await.ok();

    let configs = state.repository.get_all_configs().await.unwrap_or_default();

    Json(json!({
        "total_listings": total,
        "active_listings": active,
        "images": images_stats,
        "analysis": analysis_stats,
        "configs_count": configs.len(),
        "active_configs_count": configs.iter().filter(|c| c.is_active.unwrap_or(false)).count()
    }))
}

#[derive(serde::Deserialize)]
struct AnalyzeAllParams {
    #[serde(default = "default_concurrency")]
    concurrency: usize,
}

fn default_concurrency() -> usize {
    10
}

async fn trigger_image_analysis(
    State(state): State<AppState>,
    Json(params): Json<Option<AnalyzeAllParams>>,
) -> impl IntoResponse {
    let concurrency = params.map(|p| p.concurrency).unwrap_or(10);

    let collector = state.collector.clone();
    tokio::spawn(async move {
        match collector.run_image_analysis_all(concurrency).await {
            Ok(count) => {
                tracing::info!("Image analysis completed: {} images analyzed", count);
            }
            Err(e) => {
                tracing::error!("Image analysis failed: {}", e);
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(json!({"message": "Image analysis started", "concurrency": concurrency})),
    )
}

async fn trigger_listing_analysis(
    State(state): State<AppState>,
    Json(params): Json<Option<AnalyzeAllParams>>,
) -> impl IntoResponse {
    let concurrency = params.map(|p| p.concurrency).unwrap_or(10);

    let collector = state.collector.clone();
    tokio::spawn(async move {
        match collector.run_listing_analysis_all(concurrency).await {
            Ok(count) => {
                tracing::info!(
                    "Listing analysis completed: {} listings analyzed",
                    count
                );
            }
            Err(e) => {
                tracing::error!("Listing analysis failed: {}", e);
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(json!({"message": "Listing analysis started", "concurrency": concurrency})),
    )
}

#[derive(serde::Deserialize)]
struct ListingsQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    min_price: Option<i64>,
    max_price: Option<i64>,
    rooms: Option<i32>,
    analyzed_only: Option<bool>,
    hide_shell: Option<bool>,
    active_only: Option<bool>,
    // New filters
    property_type: Option<String>,  // "Новостройка", "Вторичка", or empty for all
    is_shell: Option<bool>,         // true/false, or None for all
    min_condition: Option<i32>,
    max_condition: Option<i32>,
}

async fn get_listings(
    State(state): State<AppState>,
    Query(params): Query<ListingsQuery>,
) -> impl IntoResponse {
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    match state
        .repository
        .get_listings_paginated(
            offset,
            per_page,
            params.min_price,
            params.max_price,
            params.rooms,
            params.analyzed_only.unwrap_or(false),
            params.hide_shell.unwrap_or(false),
            params.active_only.unwrap_or(true),
            params.property_type,
            params.is_shell,
            params.min_condition,
            params.max_condition,
        )
        .await
    {
        Ok((listings, total)) => (
            StatusCode::OK,
            Json(json!({
                "listings": listings,
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

async fn get_listing(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.repository.get_listing_by_id(id).await {
        Ok(Some(listing)) => (StatusCode::OK, Json(json!(listing))),
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
    let collector = state.collector.clone();
    tokio::spawn(async move {
        match collector.analyze_listing_images(id).await {
            Ok(count) => {
                tracing::info!("Analyzed {} images for listing {}", count, id);
            }
            Err(e) => {
                tracing::error!("Failed to analyze images for listing {}: {}", id, e);
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(json!({"message": "Image analysis started", "listing_id": id})),
    )
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
    let collector = state.collector.clone();
    let model = params.and_then(|p| p.model);
    let model_for_response = model.clone();

    tokio::spawn(async move {
        match collector.analyze_single_listing_with_model(id, model).await {
            Ok(_) => {
                tracing::info!("Analyzed listing {}", id);
            }
            Err(e) => {
                tracing::error!("Failed to analyze listing {}: {}", id, e);
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(json!({
            "message": "Listing analysis started",
            "listing_id": id,
            "model": model_for_response
        })),
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
    let hours = params.hours.unwrap_or(24 * 7); // Default: last 7 days
    let collector = state.collector.clone();

    tokio::spawn(async move {
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
    });

    (
        StatusCode::ACCEPTED,
        Json(json!({"message": "Actuality sync started", "hours": hours})),
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
    match state.collector.check_actuality_by_ids(&params.ids).await {
        Ok(results) => (
            StatusCode::OK,
            Json(json!({
                "requested": params.ids.len(),
                "found": results.len(),
                "results": results
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e})),
        ),
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
