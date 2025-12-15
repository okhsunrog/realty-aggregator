use thiserror::Error;

#[derive(Error, Debug)]
pub enum AdsApiError {
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("API returned error code {code}: {message}")]
    ApiError { code: i32, message: String },

    #[error("Rate limit exceeded")]
    RateLimitExceeded,

    #[error("Invalid credentials")]
    InvalidCredentials,

    #[error("Missing required parameter: {0}")]
    MissingParameter(String),
}
