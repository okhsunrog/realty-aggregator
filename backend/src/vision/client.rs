use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum OpenAiError {
    #[error("HTTP request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),
    #[error("API error: {0}")]
    ApiError(String),
    #[error("Failed to parse response: {0}")]
    ParseError(#[from] serde_json::Error),
    #[error("No text response from model")]
    EmptyResponse,
    #[error("Model did not complete the response: {0}")]
    IncompleteResponse(String),
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    response_format: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct Message {
    role: &'static str,
    content: Vec<Content>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Content {
    Text { text: String },
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
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: Option<String>,
    refusal: Option<String>,
}

/// OpenAI-compatible Chat Completions client shared by both analysis layers.
pub struct OpenAiClient {
    client: Client,
    api_key: String,
    model: String,
    endpoint: String,
}

impl OpenAiClient {
    pub fn new(api_key: String, model: String, api_base_url: String) -> Self {
        Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(120))
                .build()
                .expect("Failed to create model API client"),
            api_key,
            model,
            endpoint: format!("{}/chat/completions", api_base_url.trim_end_matches('/')),
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub async fn analyze_image(
        &self,
        image_base64: &str,
        prompt: &str,
    ) -> Result<String, OpenAiError> {
        self.send_request(vec![
            Content::ImageUrl {
                image_url: ImageUrlData {
                    url: format!("data:image/jpeg;base64,{image_base64}"),
                },
            },
            Content::Text {
                text: prompt.to_owned(),
            },
        ])
        .await
    }

    pub async fn analyze_text(&self, prompt: &str) -> Result<String, OpenAiError> {
        self.send_request(vec![Content::Text {
            text: prompt.to_owned(),
        }])
        .await
    }

    async fn send_request(&self, content: Vec<Content>) -> Result<String, OpenAiError> {
        // The subscription proxy rejects token caps in strict mode. Do not send
        // max_tokens or imply that an ignored control caps generation.
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![Message {
                role: "user",
                content,
            }],
            response_format: serde_json::json!({"type": "json_object"}),
            reasoning_effort: (self.model == "gpt-6-luna").then_some("none"),
        };
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .header("x-proxy-compatibility", "strict")
            .json(&request)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            return Err(OpenAiError::ApiError(format!("HTTP {status}: {text}")));
        }
        Self::parse_response(&text)
    }

    fn parse_response(text: &str) -> Result<String, OpenAiError> {
        let parsed: ChatResponse = serde_json::from_str(text)?;
        if let Some(error) = parsed.error {
            return Err(OpenAiError::ApiError(error.to_string()));
        }
        let choice = parsed
            .choices
            .and_then(|choices| choices.into_iter().next())
            .ok_or(OpenAiError::EmptyResponse)?;
        if let Some(reason) = choice.finish_reason.filter(|reason| reason != "stop") {
            return Err(OpenAiError::IncompleteResponse(reason));
        }
        if let Some(refusal) = choice.message.refusal {
            return Err(OpenAiError::ApiError(refusal));
        }
        choice
            .message
            .content
            .filter(|content| !content.trim().is_empty())
            .ok_or(OpenAiError::EmptyResponse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn sends_proxy_compatible_image_request() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let header_end = loop {
                let mut chunk = [0; 4096];
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&chunk[..count]);
                if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]).to_lowercase();
            assert!(headers.starts_with("post /v1/chat/completions http/1.1"));
            assert!(headers.contains("authorization: bearer test-key"));
            assert!(headers.contains("x-proxy-compatibility: strict"));
            let length: usize = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap()
                .parse()
                .unwrap();
            while bytes.len() < header_end + length {
                let mut chunk = [0; 4096];
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&chunk[..count]);
            }
            let request: serde_json::Value = serde_json::from_slice(&bytes[header_end..]).unwrap();
            let body =
                r#"{"choices":[{"message":{"content":"{\"ok\":true}"},"finish_reason":"stop"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            request
        });
        let client = OpenAiClient::new("test-key".into(), "gpt-6-luna".into(), base_url);
        let result = client
            .analyze_image("test-image", "Return JSON.")
            .await
            .unwrap();
        assert_eq!(result, r#"{"ok":true}"#);
        let request = server.await.unwrap();
        assert_eq!(request["model"], "gpt-6-luna");
        assert_eq!(request["reasoning_effort"], "none");
        assert_eq!(request["response_format"]["type"], "json_object");
        assert_eq!(
            request["messages"][0]["content"][0]["image_url"]["url"],
            "data:image/jpeg;base64,test-image"
        );
        assert!(request.get("max_tokens").is_none());
        assert!(request.get("max_completion_tokens").is_none());
    }

    #[test]
    fn rejects_incomplete_or_refused_responses() {
        let truncated = r#"{"choices":[{"message":{"content":"{}"},"finish_reason":"length"}]}"#;
        assert!(matches!(
            OpenAiClient::parse_response(truncated),
            Err(OpenAiError::IncompleteResponse(_))
        ));
        let refusal = r#"{"choices":[{"message":{"content":null,"refusal":"Cannot assess"},"finish_reason":"stop"}]}"#;
        assert!(matches!(
            OpenAiClient::parse_response(refusal),
            Err(OpenAiError::ApiError(_))
        ));
    }
}
