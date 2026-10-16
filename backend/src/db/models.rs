use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Image download status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "image_status", rename_all = "snake_case")]
pub enum ImageStatus {
    /// Waiting to be downloaded
    Pending,
    /// Successfully downloaded to S3
    Downloaded,
    /// Got 404, will retry once
    NotFound,
    /// Got 410 Gone, permanent failure
    Gone,
    /// Failed after max retries
    Failed,
}

impl Default for ImageStatus {
    fn default() -> Self {
        Self::Pending
    }
}

#[derive(Debug, FromRow, Serialize, Deserialize)]
pub struct CollectionConfig {
    pub id: i32,
    pub city: String,
    pub category_id: i32,
    pub is_active: Option<bool>,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NewCollectionConfig {
    pub city: String,
    pub category_id: Option<i32>,
}

/// Main listing struct (without photos - those are in images table)
#[derive(Debug, FromRow, Serialize, Deserialize)]
pub struct DbListing {
    pub id: i64,
    pub source: String,
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub price: Option<i64>,
    pub city: String,
    pub address: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub rooms: Option<i32>,
    pub area: Option<f64>,
    pub floor: Option<i32>,
    pub floors_total: Option<i32>,
    pub person_type: Option<i32>,
    pub params: Option<serde_json::Value>,
    pub time_created: Option<DateTime<Utc>>,
    pub time_updated: Option<DateTime<Utc>>,
    pub fetched_at: DateTime<Utc>,
    pub is_active: Option<bool>,
    // Analysis fields
    pub analysis_condition: Option<i32>,
    pub analysis_is_shell: Option<bool>,
    pub analysis_move_in_ready: Option<bool>,
    pub analysis_red_flags: Option<Vec<String>>,
    pub analysis_summary: Option<String>,
    pub analysis_model: Option<String>,
    pub analysis_analyzed_at: Option<DateTime<Utc>>,
}

impl DbListing {
    /// Create DbListing from API response, also returns photo URLs for images table
    pub fn from_api_listing(
        listing: &crate::ads_api::models::Listing,
        city: &str,
    ) -> (Self, Vec<String>) {
        let photo_urls: Vec<String> = listing
            .images
            .as_ref()
            .map(|imgs| imgs.iter().map(|i| i.imgurl.clone()).collect())
            .unwrap_or_default();

        let lat = listing
            .coords
            .as_ref()
            .and_then(|c| c.lat.as_ref())
            .and_then(|l| l.parse::<f64>().ok());

        let lng = listing
            .coords
            .as_ref()
            .and_then(|c| c.lng.as_ref())
            .and_then(|l| l.parse::<f64>().ok());

        let rooms = listing
            .params
            .as_ref()
            .and_then(|p| p.get("Количество комнат"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i32>().ok());

        let area = listing
            .params
            .as_ref()
            .and_then(|p| p.get("Площадь"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok());

        let floor = listing
            .params
            .as_ref()
            .and_then(|p| p.get("Этаж"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i32>().ok());

        let floors_total = listing
            .params
            .as_ref()
            .and_then(|p| p.get("Этажей в доме"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i32>().ok());

        let time_created = listing
            .time_source_created
            .as_ref()
            .and_then(|t| chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S").ok())
            .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc));

        let time_updated = listing
            .time_source_updated
            .as_ref()
            .and_then(|t| chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S").ok())
            .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc));

        let db_listing = Self {
            id: listing.id,
            source: listing.source.clone().unwrap_or_default(),
            url: listing.url.clone(),
            title: listing.title.clone(),
            description: listing.description.clone(),
            price: listing.price,
            city: listing.city.clone().unwrap_or_else(|| city.to_string()),
            address: listing.address.clone(),
            lat,
            lng,
            rooms,
            area,
            floor,
            floors_total,
            person_type: listing.person_type_id,
            params: listing.params.clone(),
            time_created,
            time_updated,
            fetched_at: Utc::now(),
            is_active: Some(listing.is_actual.map(|a| a != 0).unwrap_or(true)),
            // Analysis fields - populated separately
            analysis_condition: None,
            analysis_is_shell: None,
            analysis_move_in_ready: None,
            analysis_red_flags: None,
            analysis_summary: None,
            analysis_model: None,
            analysis_analyzed_at: None,
        };

        (db_listing, photo_urls)
    }
}

/// Image record in database
#[derive(Debug, FromRow, Serialize, Deserialize)]
pub struct DbImage {
    pub id: i64,
    pub listing_id: i64,
    pub image_index: i32,
    pub source_url: String,

    // Download status
    pub status: ImageStatus,
    pub s3_key: Option<String>,
    pub http_status: Option<i32>,
    pub download_attempts: i32,
    pub last_error: Option<String>,
    pub downloaded_at: Option<DateTime<Utc>>,

    // AI analysis
    pub ai_image_type: Option<String>,
    pub ai_room: Option<String>,
    pub ai_condition: Option<i32>,
    pub ai_confidence: Option<i32>,
    pub ai_issues: Option<Vec<String>>,
    pub ai_description: Option<String>,
    pub ai_model: Option<String>,
    pub ai_analyzed_at: Option<DateTime<Utc>>,

    pub created_at: Option<DateTime<Utc>>,
}

/// Lightweight image struct for download operations
#[derive(Debug, FromRow)]
pub struct ImageForDownload {
    pub id: i64,
    pub listing_id: i64,
    pub image_index: i32,
    pub source_url: String,
    pub status: ImageStatus,
    pub download_attempts: i32,
}

/// Image with S3 key for AI analysis
#[derive(Debug, FromRow)]
pub struct ImageForAnalysis {
    pub id: i64,
    pub listing_id: i64,
    pub image_index: i32,
    pub source_url: String,
    pub s3_key: Option<String>,
}

/// Listing data needed for second-layer analysis
#[derive(Debug, FromRow, Serialize, Deserialize)]
pub struct ListingForAnalysis {
    pub id: i64,
    pub title: Option<String>,
    pub description: Option<String>,
    pub price: Option<i64>,
    pub area: Option<f64>,
    pub rooms: Option<i32>,
    pub floor: Option<i32>,
    pub floors_total: Option<i32>,
    pub address: Option<String>,
    pub source: String,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
}

impl ListingForAnalysis {
    pub fn has_coords(&self) -> bool {
        self.lat.is_some() && self.lng.is_some()
    }
}

/// Collection state for incremental loading
#[derive(Debug, FromRow)]
pub struct CollectionState {
    pub id: i32,
    pub last_listing_id: Option<i64>,
    pub last_collection_at: Option<DateTime<Utc>>,
}

/// Per-image analysis result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAnalysisResult {
    pub image_type: String,
    pub room: Option<String>,
    pub condition: Option<i32>,
    pub confidence: i32,
    pub issues: Vec<String>,
    pub description: String,
}

/// Listing-level analysis result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListingAnalysisResult {
    pub overall_condition: i32,
    pub is_shell: bool,
    pub move_in_ready: bool,
    pub red_flags: Vec<String>,
    pub summary: String,
}

/// Minimal listing info for actuality checks
#[derive(Debug, FromRow, Serialize)]
pub struct OldListingInfo {
    pub id: i64,
    pub price: Option<i64>,
    pub created_at: Option<DateTime<Utc>>,
}
