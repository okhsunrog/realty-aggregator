use base64::Engine;
use futures::future::join_all;
use reqwest::Client;
use thiserror::Error;
use tracing::{debug, warn};

use super::client::{OpenRouterClient, OpenRouterError};
use super::models::VisionAnalysis;

const ANALYSIS_PROMPT: &str = r#"Analyze these real estate apartment photos and respond ONLY with valid JSON (no markdown, no extra text).
Be objective. Base answers ONLY on what is visible in the photos; if not enough evidence, use "unknown" with low confidence.
Provide confidence scores 0.0-1.0 for every field.

JSON schema:
{
  "condition": { "state": "bare_walls|needs_renovation|livable|good|excellent|unknown", "confidence": 0.0 },
  "renovation": { "type": "none|cosmetic|standard|euro|designer|unknown", "confidence": 0.0 },
  "is_empty_shell": { "value": true|false|unknown, "confidence": 0.0 },
  "furniture": { "value": true|false|unknown, "confidence": 0.0 },
  "appliances": { "value": true|false|unknown, "confidence": 0.0 },
  "move_in_ready": { "value": true|false|unknown, "confidence": 0.0 },
  "overall_score": { "value": 1-10, "confidence": 0.0 },
  "images_used": { "value": 0, "confidence": 1.0 },
  "evidence": ["short visual cues, e.g. 'bare concrete walls', 'no flooring', 'installed toilet', 'finished kitchen'"],
  "notes": "brief neutral notes",
  "contradictions": ["optional: if photos show mixed states (e.g. one room finished, another bare shell)"]
}

Definitions:
- is_empty_shell=true ONLY if clear signs of no finishes are visible (bare concrete/brick walls, no flooring, exposed wiring/plumbing, unfinished ceilings).
- move_in_ready=true ONLY if finishes are complete AND at least one functional bathroom is visible AND either a kitchen area or cooking setup is visible; otherwise false/unknown.
- renovation.type: none = no finishes; cosmetic = repaint/wallpaper/laminate without major overhaul; standard = typical finished interior; euro/designer only if high quality materials/details are clearly visible; otherwise standard/unknown."#;

/// Maximum number of images to send in one request
const MAX_IMAGES: usize = 10;

/// Maximum image size in bytes (5MB)
const MAX_IMAGE_SIZE: usize = 5 * 1024 * 1024;

#[derive(Error, Debug)]
pub enum AnalyzerError {
    #[error("Failed to download image: {0}")]
    DownloadFailed(String),

    #[error("OpenRouter API error: {0}")]
    ApiError(#[from] OpenRouterError),

    #[error("Failed to parse analysis result: {0}")]
    ParseError(String),

    #[error("No images to analyze")]
    NoImages,
}

/// Анализатор изображений квартир
pub struct VisionAnalyzer {
    openrouter: OpenRouterClient,
    http_client: Client,
}

impl VisionAnalyzer {
    pub fn new(openrouter_api_key: String) -> Self {
        let http_client = Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            openrouter: OpenRouterClient::new(openrouter_api_key),
            http_client,
        }
    }

    pub fn with_model(openrouter_api_key: String, model: String) -> Self {
        let http_client = Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            openrouter: OpenRouterClient::new(openrouter_api_key).with_model(&model),
            http_client,
        }
    }

    pub fn model(&self) -> &str {
        self.openrouter.model()
    }

    /// Анализировать квартиру по URL изображений
    pub async fn analyze(&self, image_urls: &[String]) -> Result<VisionAnalysis, AnalyzerError> {
        if image_urls.is_empty() {
            return Err(AnalyzerError::NoImages);
        }

        // Ограничиваем количество изображений
        let urls_to_fetch: Vec<_> = image_urls.iter().take(MAX_IMAGES).collect();

        debug!("Downloading {} images for analysis", urls_to_fetch.len());

        // Скачиваем все изображения параллельно
        let download_futures: Vec<_> = urls_to_fetch
            .iter()
            .map(|url| self.download_image(url))
            .collect();

        let results = join_all(download_futures).await;

        // Фильтруем успешно скачанные
        let images_base64: Vec<String> = results
            .into_iter()
            .filter_map(|r| match r {
                Ok(img) => Some(img),
                Err(e) => {
                    warn!("Failed to download image: {}", e);
                    None
                }
            })
            .collect();

        if images_base64.is_empty() {
            return Err(AnalyzerError::NoImages);
        }

        debug!(
            "Successfully downloaded {} images, sending to OpenRouter",
            images_base64.len()
        );

        // Отправляем на анализ
        let response = self
            .openrouter
            .analyze_images(images_base64, ANALYSIS_PROMPT)
            .await?;

        // Парсим JSON ответ
        self.parse_response(&response)
    }

    /// Скачать изображение и конвертировать в base64
    async fn download_image(&self, url: &str) -> Result<String, AnalyzerError> {
        let response = self
            .http_client
            .get(url)
            .send()
            .await
            .map_err(|e| AnalyzerError::DownloadFailed(format!("{}: {}", url, e)))?;

        if !response.status().is_success() {
            return Err(AnalyzerError::DownloadFailed(format!(
                "{}: HTTP {}",
                url,
                response.status()
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| AnalyzerError::DownloadFailed(format!("{}: {}", url, e)))?;

        // Проверяем размер
        if bytes.len() > MAX_IMAGE_SIZE {
            return Err(AnalyzerError::DownloadFailed(format!(
                "{}: image too large ({} bytes)",
                url,
                bytes.len()
            )));
        }

        Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
    }

    /// Распарсить JSON ответ от модели
    fn parse_response(&self, response: &str) -> Result<VisionAnalysis, AnalyzerError> {
        // Убираем возможные markdown-обёртки
        let json_str = response
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        // Сначала парсим как serde_json::Value для нормализации
        let raw: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| AnalyzerError::ParseError(format!("Invalid JSON: {}", e)))?;

        // Нормализуем поля (модель может использовать разные названия)
        let normalized = self.normalize_response(raw)?;

        serde_json::from_value(normalized)
            .map_err(|e| AnalyzerError::ParseError(format!("Failed to deserialize: {}", e)))
    }

    /// Нормализовать ответ модели в ожидаемый формат
    fn normalize_response(
        &self,
        mut raw: serde_json::Value,
    ) -> Result<serde_json::Value, AnalyzerError> {
        let obj = raw
            .as_object_mut()
            .ok_or_else(|| AnalyzerError::ParseError("Response is not an object".to_string()))?;

        // Нормализуем renovation.type -> renovation.renovation_type
        if let Some(renovation) = obj.get_mut("renovation") {
            if let Some(renovation_obj) = renovation.as_object_mut() {
                if let Some(type_val) = renovation_obj.remove("type") {
                    renovation_obj.insert("renovation_type".to_string(), type_val);
                }
            }
        }

        // Нормализуем boolean поля (value -> value для furniture, appliances и т.д.)
        for field in ["furniture", "appliances", "is_empty_shell", "move_in_ready"] {
            if let Some(field_val) = obj.get_mut(field) {
                if let Some(field_obj) = field_val.as_object_mut() {
                    // Если есть has_furniture/has_appliances - переименовать в value
                    if let Some(val) = field_obj.remove(&format!("has_{}", field)) {
                        field_obj.insert("value".to_string(), val);
                    }
                    // Если поле "present" вместо "value"
                    if let Some(val) = field_obj.remove("present") {
                        field_obj.insert("value".to_string(), val);
                    }
                }
            }
        }

        Ok(raw)
    }
}
