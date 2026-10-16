use thiserror::Error;
use tracing::debug;

use super::client::{OpenAiClient, OpenAiError};
use crate::db::{DbImage, ListingAnalysisResult, ListingForAnalysis};

const LISTING_ANALYSIS_PROMPT_TEMPLATE: &str = r#"Проанализируй объявление о продаже квартиры. Ответ только JSON, без markdown.

АНАЛИЗ ФОТОГРАФИЙ:
{photo_analyses}

ДАННЫЕ ОБЪЯВЛЕНИЯ:
Заголовок: {title}
Описание: {description}
Цена: {price} руб.
Площадь: {area} м²
Комнат: {rooms}
Этаж: {floor}/{floors_total}
Адрес: {address}

{
  "overall_condition": 0-100,
  "is_shell": true|false,
  "move_in_ready": true|false,
  "red_flags": [],
  "summary": "2-3 предложения"
}

ВАЖНО: Сначала проверь типы фото (поле "type" в анализе):
- "interior" = фото интерьера квартиры
- "exterior" = фото здания/двора снаружи
- "floor_plan" = планировка
- "other" = прочее

Если НЕТ фото интерьера (все фото exterior/floor_plan/other):
- overall_condition = 0 (невозможно оценить)
- is_shell = true (предполагаем без отделки, раз не показывают)
- move_in_ready = false
- red_flags должен содержать "Нет фото интерьера"

overall_condition: Возьми СРЕДНЕЕ значение condition из анализа фото интерьера как базу. Корректируй только при серьёзных проблемах.
НЕ занижай оценку за "датированный" но функциональный ремонт - это нормальное состояние.
- 0 = нет фото интерьера
- 10-30 = требует капремонта (серьёзные повреждения, аварийное состояние)
- 30-50 = требует серьёзного ремонта (много проблем, сильный износ)
- 50-65 = требует косметического ремонта (устаревший но функциональный)
- 65-80 = хорошее состояние (ухоженная квартира, можно жить)
- 80-100 = отличное состояние (свежий/качественный ремонт)

is_shell: true ТОЛЬКО если голые стены без отделки или нет фото интерьера.

move_in_ready: true если квартира в жилом состоянии (можно заселяться без ремонта, даже если ремонт старый).

red_flags: ТОЛЬКО серьёзные проблемы на русском языке (1-3 слова):
- "Нет фото интерьера" (если только exterior)
- "Плесень", "Трещины", "Потёки воды", "Грибок"
- "Аварийное состояние"
- "Общежитие", "Коммуналка", "Доля", "Обременение"
НЕ добавляй "Старый ремонт" или "Требует ремонта" - это не red flag, это просто состояние.

summary: 2-3 предложения. Опиши общее впечатление от квартиры."#;

#[derive(Error, Debug)]
pub enum ListingAnalyzerError {
    #[error("OpenAI-compatible API API error: {0}")]
    ApiError(#[from] OpenAiError),

    #[error("Failed to parse analysis result: {0}")]
    ParseError(String),

    #[error("No image analyses available")]
    NoImageAnalyses,
}

/// Listing-level analyzer using the configured model
pub struct ListingAnalyzer {
    vision_api: OpenAiClient,
}

impl ListingAnalyzer {
    pub fn new(api_key: String, model: String, api_base_url: String) -> Self {
        Self {
            vision_api: OpenAiClient::new(api_key, model, api_base_url),
        }
    }

    pub fn model(&self) -> &str {
        self.vision_api.model()
    }

    /// Analyze a listing based on its image analyses and listing data
    pub async fn analyze(
        &self,
        listing: &ListingForAnalysis,
        image_analyses: &[DbImage],
    ) -> Result<ListingAnalysisResult, ListingAnalyzerError> {
        if image_analyses.is_empty() {
            return Err(ListingAnalyzerError::NoImageAnalyses);
        }

        let prompt = self.build_prompt(listing, image_analyses);
        debug!("Sending listing analysis request to OpenAI-compatible API");

        let response = self.vision_api.analyze_text(&prompt).await?;
        self.parse_response(&response)
    }

    /// Build prompt with listing data and image analyses
    fn build_prompt(&self, listing: &ListingForAnalysis, image_analyses: &[DbImage]) -> String {
        // Format image analyses as compact JSON
        let photo_analyses: Vec<serde_json::Value> = image_analyses
            .iter()
            .filter_map(|img| {
                Some(serde_json::json!({
                    "index": img.image_index,
                    "type": img.ai_image_type.as_ref()?,
                    "room": img.ai_room,
                    "condition": img.ai_condition,
                    "issues": img.ai_issues.as_ref().unwrap_or(&vec![]),
                    "description": img.ai_description.as_ref()?
                }))
            })
            .collect();

        let photo_analyses_json = serde_json::to_string_pretty(&photo_analyses)
            .unwrap_or_else(|_| "[]".to_string());

        LISTING_ANALYSIS_PROMPT_TEMPLATE
            .replace("{photo_analyses}", &photo_analyses_json)
            .replace("{title}", listing.title.as_deref().unwrap_or("Не указано"))
            .replace("{description}", listing.description.as_deref().unwrap_or("Нет описания"))
            .replace("{price}", &listing.price.map(|p| p.to_string()).unwrap_or_else(|| "Не указана".to_string()))
            .replace("{area}", &listing.area.map(|a| format!("{:.1}", a)).unwrap_or_else(|| "Не указана".to_string()))
            .replace("{rooms}", &listing.rooms.map(|r| r.to_string()).unwrap_or_else(|| "Не указано".to_string()))
            .replace("{floor}", &listing.floor.map(|f| f.to_string()).unwrap_or_else(|| "?".to_string()))
            .replace("{floors_total}", &listing.floors_total.map(|f| f.to_string()).unwrap_or_else(|| "?".to_string()))
            .replace("{address}", listing.address.as_deref().unwrap_or("Не указан"))
    }

    /// Parse JSON response from model
    fn parse_response(&self, response: &str) -> Result<ListingAnalysisResult, ListingAnalyzerError> {
        // Remove possible markdown wrappers
        let json_str = response
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        let raw: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| ListingAnalyzerError::ParseError(format!("Invalid JSON: {} - Response: {}", e, json_str)))?;

        let obj = raw
            .as_object()
            .ok_or_else(|| ListingAnalyzerError::ParseError("Response is not an object".to_string()))?;

        let overall_condition = obj
            .get("overall_condition")
            .and_then(|v| v.as_i64())
            .unwrap_or(50) as i32;

        let is_shell = obj
            .get("is_shell")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let move_in_ready = obj
            .get("move_in_ready")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let red_flags: Vec<String> = obj
            .get("red_flags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let summary = obj
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("Анализ не выполнен")
            .to_string();

        Ok(ListingAnalysisResult {
            overall_condition,
            is_shell,
            move_in_ready,
            red_flags,
            summary,
        })
    }
}
