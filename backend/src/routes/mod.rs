use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
};
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
        .route("/api/configs", get(list_configs))
        .route("/api/configs", post(create_config))
        .route("/api/configs/{id}", delete(delete_config))
        .route("/api/configs/{id}/toggle", post(toggle_config))
        .route("/api/stats", get(get_stats))
        // Vision analysis endpoints
        .route("/api/analyze", post(trigger_analysis_batch))
        .route("/api/analyze/{id}", post(analyze_single))
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
    // First get current state
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
    let unanalyzed = state.repository.get_unanalyzed_count().await.unwrap_or(0);
    let configs = state.repository.get_all_configs().await.unwrap_or_default();

    Json(json!({
        "total_listings": total,
        "active_listings": active,
        "unanalyzed_listings": unanalyzed,
        "configs_count": configs.len(),
        "active_configs_count": configs.iter().filter(|c| c.is_active.unwrap_or(false)).count()
    }))
}

#[derive(serde::Deserialize)]
struct AnalyzeBatchParams {
    #[serde(default = "default_batch_limit")]
    limit: i64,
}

fn default_batch_limit() -> i64 {
    50
}

async fn trigger_analysis_batch(
    State(state): State<AppState>,
    Json(params): Json<Option<AnalyzeBatchParams>>,
) -> impl IntoResponse {
    let limit = params.map(|p| p.limit).unwrap_or(50);

    let collector = state.collector.clone();
    tokio::spawn(async move {
        match collector.run_vision_analysis_batch(limit).await {
            Ok(count) => {
                tracing::info!(
                    "Vision analysis batch completed: {} listings analyzed",
                    count
                );
            }
            Err(e) => {
                tracing::error!("Vision analysis batch failed: {}", e);
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(json!({"message": "Vision analysis batch started", "limit": limit})),
    )
}

async fn analyze_single(State(state): State<AppState>, Path(id): Path<i64>) -> impl IntoResponse {
    match state.collector.analyze_single_listing(id).await {
        Ok(analysis) => (
            StatusCode::OK,
            Json(json!({
                "listing_id": id,
                "analysis": analysis
            })),
        ),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))),
    }
}
