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
    /// Интервал сбора данных в минутах
    #[serde(default = "default_collect_interval")]
    pub collect_interval_minutes: u32,

    /// Задержка между запросами к API в секундах
    #[serde(default = "default_request_delay")]
    pub request_delay_seconds: u32,

    /// Настройки синхронизации актуальности
    #[serde(default)]
    pub sync: ApiSyncConfig,

    /// Параметры запроса к API
    pub query: ApiQueryConfig,
}

#[derive(Debug, Deserialize)]
pub struct ApiSyncConfig {
    /// Включена ли синхронизация актуальности
    #[serde(default = "default_sync_enabled")]
    pub enabled: bool,

    /// Интервал проверки актуальности в часах
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
    /// ID категории (2 = квартиры)
    pub category_id: i32,

    /// Тип сделки (1 = продам)
    pub nedvigimost_type: i32,

    /// Город/регион
    pub city: String,

    /// Фильтр актуальности
    #[serde(default)]
    pub is_actual: Option<String>,

    /// Цена от
    #[serde(default)]
    pub price1: Option<i64>,

    /// Цена до
    #[serde(default)]
    pub price2: Option<i64>,

    /// Площадь от (м²)
    #[serde(default)]
    pub area_from: Option<i32>,

    /// Площадь до (м²)
    #[serde(default)]
    pub area_to: Option<i32>,

    /// Тип продавца
    #[serde(default)]
    pub person_type: Option<i32>,

    /// Источники (1=avito, 4=cian)
    #[serde(default)]
    pub source: Option<String>,

    /// Только с координатами
    #[serde(default)]
    pub withcoords: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct StorageConfig {
    /// Шаблон пути для изображений
    #[serde(default = "default_image_path_template")]
    pub image_path_template: String,
}

fn default_image_path_template() -> String {
    "{listing_id}/{index}.jpg".to_string()
}

#[derive(Debug, Deserialize)]
pub struct VisionConfig {
    /// Включить автоматический анализ после сбора
    #[serde(default)]
    pub auto_analyze: bool,

    /// Модель для анализа
    #[serde(default = "default_vision_model")]
    pub model: String,

    /// Максимум попыток перед статусом failed
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// Максимум изображений в одном запросе
    #[serde(default = "default_max_images")]
    pub max_images_per_request: usize,

    /// Фильтрация перед анализом
    pub filter: VisionFilterConfig,
}

fn default_vision_model() -> String {
    "google/gemini-2.0-flash-lite-001".to_string()
}

fn default_max_retries() -> u32 {
    3
}

fn default_max_images() -> usize {
    10
}

#[derive(Debug, Deserialize)]
pub struct VisionFilterConfig {
    /// Включена ли фильтрация
    #[serde(default = "default_filter_enabled")]
    pub enabled: bool,

    /// Минимум фото
    #[serde(default = "default_min_photos")]
    pub min_photos: usize,

    /// Максимум фото для анализа
    #[serde(default)]
    pub max_photos: Option<usize>,

    /// Минимальная цена
    #[serde(default)]
    pub min_price: Option<i64>,

    /// Максимальная цена
    #[serde(default)]
    pub max_price: Option<i64>,

    /// Минимальная площадь
    #[serde(default)]
    pub min_area: Option<f64>,

    /// Максимальная площадь
    #[serde(default)]
    pub max_area: Option<f64>,

    /// Разрешённые источники
    #[serde(default)]
    pub sources: Option<Vec<String>>,

    /// Требовать координаты
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
    /// Загрузить конфигурацию из файла
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path.as_ref())
            .map_err(|e| ConfigError::FileRead(e.to_string()))?;

        toml::from_str(&content).map_err(|e| ConfigError::Parse(e.to_string()))
    }

    /// Загрузить из стандартного пути (config.toml)
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
    /// Проверить, проходит ли объявление фильтр
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

        // Проверка минимума фото
        if photos_count < self.min_photos {
            return false;
        }

        // Проверка цены
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

        // Проверка площади
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

        // Проверка источника
        if let Some(allowed_sources) = &self.sources {
            if let Some(src) = source {
                if !allowed_sources.iter().any(|s| s.eq_ignore_ascii_case(src)) {
                    return false;
                }
            } else {
                return false;
            }
        }

        // Проверка координат
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
            max_photos: None,
            min_price: None,
            max_price: None,
            min_area: None,
            max_area: None,
            sources: None,
            require_coords: false,
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
            max_photos: None,
            min_price: Some(1_000_000),
            max_price: Some(5_000_000),
            min_area: None,
            max_area: None,
            sources: None,
            require_coords: false,
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
            max_photos: None,
            min_price: Some(100_000_000),
            max_price: None,
            min_area: None,
            max_area: None,
            sources: None,
            require_coords: true,
        };

        // Даже с невыполнимыми условиями, фильтр пропускает всё
        assert!(filter.passes_filter(0, Some(100), None, None, false));
    }
}
