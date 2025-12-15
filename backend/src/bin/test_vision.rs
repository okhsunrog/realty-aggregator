use base64::Engine;
use serde::{Deserialize, Serialize};

const ANALYSIS_PROMPT: &str = r#"Analyze this real estate apartment photo and provide a JSON response with the following structure. Be objective and provide confidence scores (0.0-1.0) for each assessment.

{
  "apartment_condition": {
    "state": "bare_walls|needs_renovation|livable|good|excellent",
    "confidence": 0.0-1.0,
    "description": "brief description"
  },
  "renovation_type": {
    "type": "none|cosmetic|standard|euro|designer",
    "confidence": 0.0-1.0
  },
  "is_new_construction_empty": {
    "value": true/false,
    "confidence": 0.0-1.0,
    "description": "if true, describe why (bare concrete, no finishes, etc)"
  },
  "furniture": {
    "has_furniture": true/false,
    "confidence": 0.0-1.0,
    "items": ["list", "of", "visible", "furniture"]
  },
  "appliances": {
    "has_appliances": true/false,
    "confidence": 0.0-1.0,
    "items": ["list", "of", "visible", "appliances"]
  },
  "room_type": {
    "type": "kitchen|living_room|bedroom|bathroom|hallway|balcony|other",
    "confidence": 0.0-1.0
  },
  "photo_quality": {
    "quality": "poor|amateur|decent|professional",
    "confidence": 0.0-1.0
  },
  "window_view": {
    "visible": true/false,
    "description": "what can be seen if visible"
  },
  "overall_livability": {
    "score": 1-10,
    "confidence": 0.0-1.0,
    "can_move_in_immediately": true/false
  }
}

IMPORTANT:
- "bare_walls" = completely unfinished, concrete/brick walls, no flooring
- "needs_renovation" = has some finishes but in poor condition
- "livable" = basic finishes, can live but dated
- "good" = modern finishes, well maintained
- "excellent" = high-end finishes, designer quality

Respond ONLY with valid JSON, no other text."#;

#[derive(Debug, Serialize)]
struct OpenRouterRequest {
    model: String,
    messages: Vec<Message>,
    max_tokens: u32,
}

#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: Vec<Content>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum Content {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Serialize)]
struct ImageUrl {
    url: String,
}

#[derive(Debug, Deserialize)]
struct OpenRouterResponse {
    choices: Option<Vec<Choice>>,
    error: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let openrouter_key =
        std::env::var("OPENROUTER_API_KEY").expect("OPENROUTER_API_KEY not set in .env");

    // Читаем тестовое изображение
    let image_path = "/tmp/realty_images/test_image.jpg";
    let image_bytes = std::fs::read(image_path)?;
    let base64_image = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

    println!("Изображение загружено: {} байт", image_bytes.len());
    println!("Отправляем на анализ...\n");

    let client = reqwest::Client::new();

    // Используем Gemini Flash - быстрый и дешёвый
    let model = "google/gemini-2.0-flash-lite-001";

    let request = OpenRouterRequest {
        model: model.to_string(),
        messages: vec![Message {
            role: "user".to_string(),
            content: vec![
                Content::ImageUrl {
                    image_url: ImageUrl {
                        url: format!("data:image/jpeg;base64,{}", base64_image),
                    },
                },
                Content::Text {
                    text: ANALYSIS_PROMPT.to_string(),
                },
            ],
        }],
        max_tokens: 1500,
    };

    let response = client
        .post("https://openrouter.ai/api/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", openrouter_key))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await?;

    let status = response.status();
    let text = response.text().await?;

    if !status.is_success() {
        println!("Ошибка API: {} - {}", status, text);
        return Ok(());
    }

    let parsed: OpenRouterResponse = serde_json::from_str(&text)?;

    if let Some(error) = parsed.error {
        println!("API Error: {:?}", error);
        return Ok(());
    }

    if let Some(choices) = parsed.choices {
        if let Some(choice) = choices.first() {
            println!("=== Результат анализа ===\n");
            println!("Модель: {}\n", model);

            // Пробуем распарсить JSON
            let content = &choice.message.content;

            // Убираем возможные markdown-обёртки
            let json_str = content
                .trim()
                .trim_start_matches("```json")
                .trim_start_matches("```")
                .trim_end_matches("```")
                .trim();

            match serde_json::from_str::<serde_json::Value>(json_str) {
                Ok(json) => {
                    println!("{}", serde_json::to_string_pretty(&json)?);
                }
                Err(_) => {
                    println!("Raw response:\n{}", content);
                }
            }
        }
    }

    Ok(())
}
