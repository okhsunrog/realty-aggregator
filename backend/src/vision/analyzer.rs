use base64::Engine;
use reqwest::Client;
use thiserror::Error;
use tracing::{debug, warn};

use super::client::{OpenRouterClient, OpenRouterError};
use crate::db::ImageAnalysisResult;

const IMAGE_ANALYSIS_PROMPT: &str = r#"Analyze this apartment photo. JSON only, no markdown.

{
  "image_type": "interior|exterior|floor_plan|other",
  "room": "bathroom|kitchen|living|bedroom|hallway|null",
  "condition": 0-100 or null,
  "confidence": 0-100,
  "issues": [],
  "description": "string"
}

image_type:
- interior: room inside apartment
- exterior: building, yard, street view
- floor_plan: схема, plan drawing
- other: documents, unclear, unrelated

room: Which room is shown. Set null if not interior.

condition: Apartment finish quality, 0-100. Set null if not interior.
- 0 = bare shell (concrete walls, cement floor, exposed pipes/wires, no finishes)
- 25 = needs major renovation (very old, damaged, partially unfinished)
- 50 = needs cosmetic renovation (dated finishes, worn but functional)
- 75 = good condition (decent finishes, minor wear)
- 100 = excellent (new renovation, move-in ready)

confidence: Your certainty in this assessment, 0-100.

issues: Visible problems. Empty array if none.
Examples: water damage, mold, cracks, broken tiles, peeling paint.

description: One sentence describing what you see."#;

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

    #[error("Image too large")]
    ImageTooLarge,
}

/// Per-image analyzer using gemma-3-4b-it
pub struct ImageAnalyzer {
    openrouter: OpenRouterClient,
    http_client: Client,
}

impl ImageAnalyzer {
    pub fn new(api_key: String, model: String) -> Self {
        let http_client = Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            openrouter: OpenRouterClient::new(api_key, model),
            http_client,
        }
    }

    pub fn model(&self) -> &str {
        self.openrouter.model()
    }

    /// Analyze a single image from URL
    pub async fn analyze_from_url(&self, image_url: &str) -> Result<ImageAnalysisResult, AnalyzerError> {
        let image_base64 = self.download_image(image_url).await?;
        self.analyze_base64(&image_base64).await
    }

    /// Analyze a single image from base64
    pub async fn analyze_base64(&self, image_base64: &str) -> Result<ImageAnalysisResult, AnalyzerError> {
        debug!("Sending image to OpenRouter for analysis");

        let response = self
            .openrouter
            .analyze_image(image_base64, IMAGE_ANALYSIS_PROMPT)
            .await?;

        self.parse_response(&response)
    }

    /// Download image and convert to base64
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

        if bytes.len() > MAX_IMAGE_SIZE {
            return Err(AnalyzerError::ImageTooLarge);
        }

        Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
    }

    /// Parse JSON response from model
    fn parse_response(&self, response: &str) -> Result<ImageAnalysisResult, AnalyzerError> {
        // Remove possible markdown wrappers
        let json_str = response
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        // Parse as raw JSON first for flexibility
        let raw: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| AnalyzerError::ParseError(format!("Invalid JSON: {} - Response: {}", e, json_str)))?;

        let obj = raw
            .as_object()
            .ok_or_else(|| AnalyzerError::ParseError("Response is not an object".to_string()))?;

        // Extract fields with defaults
        let image_type = obj
            .get("image_type")
            .and_then(|v| v.as_str())
            .unwrap_or("other")
            .to_string();

        let room = obj
            .get("room")
            .and_then(|v| {
                if v.is_null() {
                    None
                } else {
                    v.as_str().map(|s| s.to_string())
                }
            });

        let condition = obj
            .get("condition")
            .and_then(|v| {
                if v.is_null() {
                    None
                } else {
                    v.as_i64().map(|n| n as i32)
                }
            });

        let confidence = obj
            .get("confidence")
            .and_then(|v| v.as_i64())
            .unwrap_or(50) as i32;

        let issues: Vec<String> = obj
            .get("issues")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let description = obj
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("No description")
            .to_string();

        Ok(ImageAnalysisResult {
            image_type,
            room,
            condition,
            confidence,
            issues,
            description,
        })
    }
}
