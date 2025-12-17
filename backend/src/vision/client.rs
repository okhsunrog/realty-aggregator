use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const OPENROUTER_API_URL: &str = "https://openrouter.ai/api/v1/chat/completions";

#[derive(Error, Debug)]
pub enum OpenRouterError {
    #[error("HTTP request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),

    #[error("API error: {0}")]
    ApiError(String),

    #[error("Failed to parse response: {0}")]
    ParseError(#[from] serde_json::Error),

    #[error("No response from model")]
    EmptyResponse,
}

#[derive(Debug, Serialize)]
struct ChatRequest {
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
    ImageUrl { image_url: ImageUrlData },
}

#[derive(Debug, Serialize)]
struct ImageUrlData {
    url: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
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

/// Client for OpenRouter API
pub struct OpenRouterClient {
    client: Client,
    api_key: String,
    model: String,
}

impl OpenRouterClient {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model,
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Analyze a single image with a prompt
    pub async fn analyze_image(
        &self,
        image_base64: &str,
        prompt: &str,
    ) -> Result<String, OpenRouterError> {
        let content = vec![
            Content::ImageUrl {
                image_url: ImageUrlData {
                    url: format!("data:image/jpeg;base64,{}", image_base64),
                },
            },
            Content::Text {
                text: prompt.to_string(),
            },
        ];

        self.send_request(content, 500).await
    }

    /// Send a text-only request (for listing-level analysis)
    pub async fn analyze_text(&self, prompt: &str) -> Result<String, OpenRouterError> {
        let content = vec![Content::Text {
            text: prompt.to_string(),
        }];

        self.send_request(content, 1000).await
    }

    /// Send request to OpenRouter API
    async fn send_request(
        &self,
        content: Vec<Content>,
        max_tokens: u32,
    ) -> Result<String, OpenRouterError> {
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content,
            }],
            max_tokens,
        };

        let response = self
            .client
            .post(OPENROUTER_API_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        let text = response.text().await?;

        if !status.is_success() {
            return Err(OpenRouterError::ApiError(format!(
                "HTTP {}: {}",
                status, text
            )));
        }

        let parsed: ChatResponse = serde_json::from_str(&text)?;

        if let Some(error) = parsed.error {
            return Err(OpenRouterError::ApiError(format!("{:?}", error)));
        }

        parsed
            .choices
            .and_then(|choices| choices.into_iter().next())
            .map(|choice| choice.message.content)
            .ok_or(OpenRouterError::EmptyResponse)
    }
}
