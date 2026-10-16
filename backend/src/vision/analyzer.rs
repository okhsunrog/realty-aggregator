use base64::Engine;
use reqwest::Client;
use thiserror::Error;
use tracing::debug;

use super::client::{OpenAiClient, OpenAiError};
use crate::db::ImageAnalysisResult;

const IMAGE_ANALYSIS_PROMPT: &str = r#"Analyze this apartment photo. JSON only, no markdown. Write issues and description in Russian; keep enum values in English.

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

    #[error("OpenAI-compatible API API error: {0}")]
    ApiError(#[from] OpenAiError),

    #[error("Failed to parse analysis result: {0}")]
    ParseError(String),

    #[error("Image too large")]
    ImageTooLarge,
}

/// Per-image analyzer using the configured vision model
pub struct ImageAnalyzer {
    vision_api: OpenAiClient,
    http_client: Client,
}

impl ImageAnalyzer {
    pub fn new(api_key: String, model: String, api_base_url: String) -> Self {
        let http_client = Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            vision_api: OpenAiClient::new(api_key, model, api_base_url),
            http_client,
        }
    }

    pub fn model(&self) -> &str {
        self.vision_api.model()
    }

    /// Analyze a single image from URL
    pub async fn analyze_from_url(
        &self,
        image_url: &str,
    ) -> Result<ImageAnalysisResult, AnalyzerError> {
        let image_base64 = self.download_image(image_url).await?;
        self.analyze_base64(&image_base64).await
    }

    /// Analyze a single image from base64
    pub async fn analyze_base64(
        &self,
        image_base64: &str,
    ) -> Result<ImageAnalysisResult, AnalyzerError> {
        debug!("Sending image to OpenAI-compatible API for analysis");

        let response = self
            .vision_api
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

        let raw: serde_json::Value =
            serde_json::from_str(json_str).map_err(|e| AnalyzerError::ParseError(e.to_string()))?;
        for field in [
            "image_type",
            "room",
            "condition",
            "confidence",
            "issues",
            "description",
        ] {
            if raw.get(field).is_none() {
                return Err(AnalyzerError::ParseError(format!("Missing {field}")));
            }
        }
        let result: ImageAnalysisResult =
            serde_json::from_value(raw).map_err(|e| AnalyzerError::ParseError(e.to_string()))?;
        if !["interior", "exterior", "floor_plan", "other"].contains(&result.image_type.as_str())
            || !(0..=100).contains(&result.confidence)
            || result.condition.is_some_and(|n| !(0..=100).contains(&n))
            || (result.image_type != "interior"
                && (result.condition.is_some() || result.room.is_some()))
            || result.room.as_deref().is_some_and(|r| {
                !["bathroom", "kitchen", "living", "bedroom", "hallway"].contains(&r)
            })
            || result.description.trim().is_empty()
        {
            return Err(AnalyzerError::ParseError(
                "Invalid image analysis fields".to_owned(),
            ));
        }
        Ok(result)
    }
}
