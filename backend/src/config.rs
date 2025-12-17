use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub api: ApiConfig,
    pub storage: StorageConfig,
    pub vision: VisionConfig,
}

#[derive(Debug, Deserialize)]
pub struct ApiConfig {
    /// Collection interval in minutes
    #[serde(default = "default_collect_interval")]
    pub collect_interval_minutes: u32,

    /// Delay between API requests in seconds
    #[serde(default = "default_request_delay")]
    pub request_delay_seconds: u32,

    /// Actuality sync settings
    #[serde(default)]
    pub sync: ApiSyncConfig,

    /// API query parameters
    pub query: ApiQueryConfig,
}

#[derive(Debug, Deserialize)]
pub struct ApiSyncConfig {
    /// Enable actuality sync
    #[serde(default = "default_sync_enabled")]
    pub enabled: bool,

    /// Sync interval in hours
    #[serde(default = "default_sync_interval")]
    pub interval_hours: u32,
}

impl Default for ApiSyncConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_hours: 24,
        }
    }
}

fn default_sync_enabled() -> bool {
    true
}

fn default_sync_interval() -> u32 {
    24
}

fn default_collect_interval() -> u32 {
    60
}

fn default_request_delay() -> u32 {
    6
}

#[derive(Debug, Deserialize)]
pub struct ApiQueryConfig {
    /// Category ID (2 = apartments)
    pub category_id: i32,

    /// Deal type (1 = sell)
    pub nedvigimost_type: i32,

    /// City/region
    pub city: String,

    /// Actuality filter
    #[serde(default)]
    pub is_actual: Option<String>,

    /// Price from
    #[serde(default)]
    pub price1: Option<i64>,

    /// Price to
    #[serde(default)]
    pub price2: Option<i64>,

    /// Area from (m²)
    #[serde(default)]
    pub area_from: Option<i32>,

    /// Area to (m²)
    #[serde(default)]
    pub area_to: Option<i32>,

    /// Seller type
    #[serde(default)]
    pub person_type: Option<i32>,

    /// Sources (1=avito, 4=cian)
    #[serde(default)]
    pub source: Option<String>,

    /// Only with coordinates
    #[serde(default)]
    pub withcoords: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct StorageConfig {
    /// Image path template
    #[serde(default = "default_image_path_template")]
    pub image_path_template: String,
}

fn default_image_path_template() -> String {
    "{listing_id}/{index}.jpg".to_string()
}

#[derive(Debug, Deserialize)]
pub struct VisionConfig {
    /// Enable automatic analysis after collection
    #[serde(default)]
    pub auto_analyze: bool,

    /// Model for per-image analysis (first layer)
    #[serde(default = "default_image_model")]
    pub image_model: String,

    /// Model for listing-level analysis (second layer)
    #[serde(default = "default_listing_model")]
    pub listing_model: String,

    /// Max retries before failed status
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// Pre-analysis filter
    #[serde(default)]
    pub filter: VisionFilterConfig,
}

fn default_image_model() -> String {
    "google/gemma-3-4b-it".to_string()
}

fn default_listing_model() -> String {
    "google/gemini-2.5-flash-lite".to_string()
}

fn default_max_retries() -> u32 {
    3
}

#[derive(Debug, Deserialize, Default)]
pub struct VisionFilterConfig {
    /// Enable filtering
    #[serde(default = "default_filter_enabled")]
    pub enabled: bool,

    /// Minimum photos
    #[serde(default = "default_min_photos")]
    pub min_photos: usize,

    /// Maximum photos to analyze
    #[serde(default)]
    pub max_photos: Option<usize>,

    /// Minimum price
    #[serde(default)]
    pub min_price: Option<i64>,

    /// Maximum price
    #[serde(default)]
    pub max_price: Option<i64>,

    /// Minimum area
    #[serde(default)]
    pub min_area: Option<f64>,

    /// Maximum area
    #[serde(default)]
    pub max_area: Option<f64>,

    /// Allowed sources
    #[serde(default)]
    pub sources: Option<Vec<String>>,

    /// Require coordinates
    #[serde(default)]
    pub require_coords: bool,
}

fn default_filter_enabled() -> bool {
    true
}

fn default_min_photos() -> usize {
    1
}

impl Config {
    /// Load config from file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path.as_ref())
            .map_err(|e| ConfigError::FileRead(e.to_string()))?;

        toml::from_str(&content).map_err(|e| ConfigError::Parse(e.to_string()))
    }

    /// Load from default path (config.toml)
    pub fn load_default() -> Result<Self, ConfigError> {
        Self::load("config.toml")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    FileRead(String),

    #[error("Failed to parse config: {0}")]
    Parse(String),
}

impl VisionFilterConfig {
    /// Check if listing passes filter
    pub fn passes_filter(
        &self,
        photos_count: usize,
        price: Option<i64>,
        area: Option<f64>,
        source: Option<&str>,
        has_coords: bool,
    ) -> bool {
        if !self.enabled {
            return true;
        }

        if photos_count < self.min_photos {
            return false;
        }

        if let Some(min_price) = self.min_price {
            if price.map(|p| p < min_price).unwrap_or(true) {
                return false;
            }
        }
        if let Some(max_price) = self.max_price {
            if price.map(|p| p > max_price).unwrap_or(false) {
                return false;
            }
        }

        if let Some(min_area) = self.min_area {
            if area.map(|a| a < min_area).unwrap_or(true) {
                return false;
            }
        }
        if let Some(max_area) = self.max_area {
            if area.map(|a| a > max_area).unwrap_or(false) {
                return false;
            }
        }

        if let Some(allowed_sources) = &self.sources {
            if let Some(src) = source {
                if !allowed_sources.iter().any(|s| s.eq_ignore_ascii_case(src)) {
                    return false;
                }
            } else {
                return false;
            }
        }

        if self.require_coords && !has_coords {
            return false;
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_min_photos() {
        let filter = VisionFilterConfig {
            enabled: true,
            min_photos: 3,
            ..Default::default()
        };

        assert!(!filter.passes_filter(2, None, None, None, false));
        assert!(filter.passes_filter(3, None, None, None, false));
        assert!(filter.passes_filter(5, None, None, None, false));
    }

    #[test]
    fn test_filter_price_range() {
        let filter = VisionFilterConfig {
            enabled: true,
            min_photos: 1,
            min_price: Some(1_000_000),
            max_price: Some(5_000_000),
            ..Default::default()
        };

        assert!(!filter.passes_filter(1, Some(500_000), None, None, false));
        assert!(filter.passes_filter(1, Some(2_000_000), None, None, false));
        assert!(!filter.passes_filter(1, Some(6_000_000), None, None, false));
    }

    #[test]
    fn test_filter_disabled() {
        let filter = VisionFilterConfig {
            enabled: false,
            min_photos: 10,
            min_price: Some(100_000_000),
            require_coords: true,
            ..Default::default()
        };

        assert!(filter.passes_filter(0, Some(100), None, None, false));
    }
}
