use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Статус vision-анализа объявления
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "vision_status", rename_all = "lowercase")]
pub enum VisionStatus {
    /// Ожидает анализа
    Pending,
    /// Успешно проанализировано
    Done,
    /// Не требует анализа (нет фото, не прошёл фильтр)
    NotNeeded,
    /// Неудачный анализ после max_retries попыток
    Failed,
}

impl Default for VisionStatus {
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
    pub photos: Option<serde_json::Value>,
    pub params: Option<serde_json::Value>,
    pub time_created: Option<DateTime<Utc>>,
    pub time_updated: Option<DateTime<Utc>>,
    pub fetched_at: DateTime<Utc>,
    pub is_active: Option<bool>,
}

impl DbListing {
    pub fn from_api_listing(listing: &crate::ads_api::models::Listing, city: &str) -> Self {
        let photos: Option<serde_json::Value> = listing
            .images
            .as_ref()
            .map(|imgs| {
                serde_json::to_value(imgs.iter().map(|i| &i.imgurl).collect::<Vec<_>>()).ok()
            })
            .flatten();

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

        Self {
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
            photos,
            params: listing.params.clone(),
            time_created,
            time_updated,
            fetched_at: Utc::now(),
            is_active: Some(listing.is_actual.map(|a| a != 0).unwrap_or(true)),
        }
    }
}

/// Структура для объявлений, ожидающих vision-анализ
#[derive(Debug, FromRow, Serialize, Deserialize)]
pub struct ListingForAnalysis {
    pub id: i64,
    pub photos: Option<serde_json::Value>,
    pub price: Option<i64>,
    pub area: Option<f64>,
    pub source: String,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub images_stored: bool,
    pub images_count: i32,
    pub vision_status: VisionStatus,
    pub vision_attempts: i32,
}

impl ListingForAnalysis {
    /// Получить список URL изображений из JSON
    pub fn get_photo_urls(&self) -> Vec<String> {
        self.photos
            .as_ref()
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Количество фото (из JSON или сохранённых)
    pub fn photos_count(&self) -> usize {
        if self.images_stored {
            self.images_count as usize
        } else {
            self.get_photo_urls().len()
        }
    }

    /// Есть ли координаты
    pub fn has_coords(&self) -> bool {
        self.lat.is_some() && self.lng.is_some()
    }
}

/// Структура для отслеживания последнего загруженного ID
#[derive(Debug, FromRow)]
pub struct CollectionState {
    pub id: i32,
    pub last_listing_id: Option<i64>,
    pub last_collection_at: Option<DateTime<Utc>>,
}
