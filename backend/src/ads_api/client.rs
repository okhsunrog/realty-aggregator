use std::num::NonZeroU32;
use std::sync::Arc;

use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};
use reqwest::Client;

use super::error::AdsApiError;
use super::models::{
    ApiResponse, CategoryParamsResponse, CheckFeedResponse, Listing, ListingsQuery,
    SingleListingResponse,
};

type AdsRateLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

pub struct AdsApiClient {
    client: Client,
    base_url: String,
    user: String,
    token: String,
    rate_limiter: Arc<AdsRateLimiter>,
}

impl AdsApiClient {
    pub fn new(user: String, token: String) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        // 1 request per 6 seconds (5 second minimum + 1 second buffer)
        let quota = Quota::with_period(std::time::Duration::from_secs(6))
            .expect("Invalid quota period")
            .allow_burst(NonZeroU32::new(1).unwrap());

        let rate_limiter = Arc::new(RateLimiter::direct(quota));

        Self {
            client,
            base_url: "https://ads-api.ru/main".to_string(),
            user,
            token,
            rate_limiter,
        }
    }

    async fn wait_for_rate_limit(&self) {
        self.rate_limiter.until_ready().await;
    }

    pub async fn get_listings(
        &self,
        mut query: ListingsQuery,
    ) -> Result<Vec<Listing>, AdsApiError> {
        self.wait_for_rate_limit().await;

        query.user = self.user.clone();
        query.token = self.token.clone();

        let url = format!("{}/api", self.base_url);
        let response = self.client.get(&url).query(&query).send().await?;

        let api_response: ApiResponse = response.json().await?;

        match api_response.code {
            200 => Ok(api_response.data.unwrap_or_default()),
            401 => Err(AdsApiError::InvalidCredentials),
            429 => Err(AdsApiError::RateLimitExceeded),
            code => Err(AdsApiError::ApiError {
                code,
                message: api_response
                    .message
                    .unwrap_or_else(|| "Unknown error".to_string()),
            }),
        }
    }

    pub async fn get_one(&self, id: i64) -> Result<Option<Listing>, AdsApiError> {
        self.wait_for_rate_limit().await;

        let url = format!("{}/apigetone", self.base_url);
        let response = self
            .client
            .get(&url)
            .query(&[
                ("user", &self.user),
                ("token", &self.token),
                ("id", &id.to_string()),
            ])
            .send()
            .await?;

        let api_response: SingleListingResponse = response.json().await?;

        match api_response.code {
            200 => Ok(api_response.data),
            401 => Err(AdsApiError::InvalidCredentials),
            429 => Err(AdsApiError::RateLimitExceeded),
            code => Err(AdsApiError::ApiError {
                code,
                message: api_response
                    .message
                    .unwrap_or_else(|| "Unknown error".to_string()),
            }),
        }
    }

    pub async fn get_category_params(
        &self,
        category_id: i32,
    ) -> Result<Vec<super::models::CategoryParam>, AdsApiError> {
        self.wait_for_rate_limit().await;

        let url = format!("{}/apigetparams", self.base_url);
        let response = self
            .client
            .get(&url)
            .query(&[
                ("user", &self.user),
                ("token", &self.token),
                ("category_id", &category_id.to_string()),
            ])
            .send()
            .await?;

        let api_response: CategoryParamsResponse = response.json().await?;

        match api_response.code {
            200 => Ok(api_response.data.unwrap_or_default()),
            401 => Err(AdsApiError::InvalidCredentials),
            429 => Err(AdsApiError::RateLimitExceeded),
            code => Err(AdsApiError::ApiError {
                code,
                message: api_response
                    .message
                    .unwrap_or_else(|| "Unknown error".to_string()),
            }),
        }
    }

    /// Получить фид проверки актуальности
    pub async fn get_check_feed(
        &self,
        mut query: super::models::CheckFeedQuery,
    ) -> Result<Vec<super::models::CheckFeedItem>, AdsApiError> {
        self.wait_for_rate_limit().await;

        query.user = self.user.clone();
        query.token = self.token.clone();

        let url = format!("{}/apigetcheckfeed", self.base_url);
        let response = self.client.get(&url).query(&query).send().await?;

        let api_response: CheckFeedResponse = response.json().await?;

        match api_response.code {
            200 => Ok(api_response.data.unwrap_or_default()),
            401 => Err(AdsApiError::InvalidCredentials),
            429 => Err(AdsApiError::RateLimitExceeded),
            code => Err(AdsApiError::ApiError {
                code,
                message: api_response
                    .message
                    .unwrap_or_else(|| "Unknown error".to_string()),
            }),
        }
    }

    /// Получить фид проверки актуальности за последние N часов
    pub async fn get_check_feed_since_hours(
        &self,
        hours: u32,
    ) -> Result<Vec<super::models::CheckFeedItem>, AdsApiError> {
        use chrono::{Duration, Utc};

        let now = Utc::now();
        let since = now - Duration::hours(hours as i64);

        let query = super::models::CheckFeedQuery {
            date1: Some(since.format("%Y-%m-%d %H:%M:%S").to_string()),
            date2: Some(now.format("%Y-%m-%d %H:%M:%S").to_string()),
            limit: Some(1000),
            ..Default::default()
        };

        self.get_check_feed(query).await
    }

    /// Проверить актуальность по конкретным ID (макс ~500 за раз)
    pub async fn check_actuality_by_ids(
        &self,
        ids: &[i64],
    ) -> Result<Vec<super::models::CheckFeedItem>, AdsApiError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }

        let ids_str = ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");

        let query = super::models::CheckFeedQuery {
            id: Some(ids_str),
            limit: Some(1000),
            ..Default::default()
        };

        self.get_check_feed(query).await
    }
}
