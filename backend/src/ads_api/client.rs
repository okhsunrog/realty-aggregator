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
        let client = crate::network::source_client_builder()
            .redirect(reqwest::redirect::Policy::none())
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

    pub fn with_request_delay(mut self, seconds: u32) -> Self {
        self.rate_limiter = Arc::new(RateLimiter::direct(
            Quota::with_period(std::time::Duration::from_secs(seconds.max(5) as u64)).unwrap(),
        ));
        self
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
        let response = self
            .client
            .get(&url)
            .query(&query)
            .send()
            .await
            .map_err(|error| AdsApiError::HttpError(error.without_url()))?
            .error_for_status()
            .map_err(|error| AdsApiError::HttpError(error.without_url()))?;

        let api_response: ApiResponse = response
            .json()
            .await
            .map_err(|error| AdsApiError::HttpError(error.without_url()))?;

        match api_response.code {
            200 => api_response.data.ok_or_else(|| {
                AdsApiError::MissingParameter("Missing data in successful response".to_owned())
            }),
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
            .await
            .map_err(|error| AdsApiError::HttpError(error.without_url()))?
            .error_for_status()
            .map_err(|error| AdsApiError::HttpError(error.without_url()))?;

        let api_response: SingleListingResponse = response
            .json()
            .await
            .map_err(|error| AdsApiError::HttpError(error.without_url()))?;

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
            .await
            .map_err(|error| AdsApiError::HttpError(error.without_url()))?
            .error_for_status()
            .map_err(|error| AdsApiError::HttpError(error.without_url()))?;

        let api_response: CategoryParamsResponse = response
            .json()
            .await
            .map_err(|error| AdsApiError::HttpError(error.without_url()))?;

        match api_response.code {
            200 => api_response.data.ok_or_else(|| {
                AdsApiError::MissingParameter("Missing data in successful response".to_owned())
            }),
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
        let response = self
            .client
            .get(&url)
            .query(&query)
            .send()
            .await
            .map_err(|error| AdsApiError::HttpError(error.without_url()))?
            .error_for_status()
            .map_err(|error| AdsApiError::HttpError(error.without_url()))?;

        let api_response: CheckFeedResponse = response
            .json()
            .await
            .map_err(|error| AdsApiError::HttpError(error.without_url()))?;

        match api_response.code {
            200 => api_response.data.ok_or_else(|| {
                AdsApiError::MissingParameter("Missing data in successful response".to_owned())
            }),
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

    pub async fn get_check_feed_between(
        &self,
        since: chrono::DateTime<chrono::Utc>,
        through: chrono::DateTime<chrono::Utc>,
        ids: &[i64],
    ) -> Result<Vec<super::models::CheckFeedItem>, AdsApiError> {
        use chrono::Duration;
        let mut out = Vec::new();
        let mut upper = through;
        loop {
            let rows = self
                .get_check_feed(super::models::CheckFeedQuery {
                    date1: Some(
                        (since + Duration::hours(3))
                            .format("%Y-%m-%d %H:%M:%S")
                            .to_string(),
                    ),
                    date2: Some(
                        (upper + Duration::hours(3))
                            .format("%Y-%m-%d %H:%M:%S")
                            .to_string(),
                    ),
                    id: if ids.is_empty() {
                        None
                    } else {
                        Some(
                            ids.iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(","),
                        )
                    },
                    sort: Some("desc".to_owned()),
                    limit: Some(1000),
                    ..Default::default()
                })
                .await?;
            let full = rows.len() == 1000;
            let next = if full {
                let time = rows
                    .last()
                    .and_then(|r| r.time.as_deref())
                    .and_then(|time| {
                        chrono::NaiveDateTime::parse_from_str(time, "%Y-%m-%d %H:%M:%S").ok()
                    })
                    .ok_or_else(|| {
                        AdsApiError::MissingParameter(
                            "Invalid feed pagination timestamp".to_owned(),
                        )
                    })?;
                Some(time.and_utc() - Duration::hours(3) + Duration::seconds(1))
            } else {
                None
            };
            out.extend(rows);
            let Some(next) = next else {
                break;
            };
            if next >= upper || next < since {
                return Err(AdsApiError::MissingParameter(
                    "Feed pagination did not advance".to_owned(),
                ));
            }
            upper = next;
        }
        Ok(out)
    }

    pub async fn get_check_feed_since_hours(
        &self,
        hours: u32,
    ) -> Result<Vec<super::models::CheckFeedItem>, AdsApiError> {
        let through = chrono::Utc::now();
        self.get_check_feed_between(
            through - chrono::Duration::hours(hours as i64),
            through,
            &[],
        )
        .await
    }

    /// Read recorded checks. A missing ID means no check, not an inactive listing.
    pub async fn check_actuality_by_ids(
        &self,
        ids: &[i64],
    ) -> Result<Vec<super::models::CheckFeedItem>, AdsApiError> {
        let mut rows = Vec::new();
        let through = chrono::Utc::now();
        for chunk in ids.chunks(250) {
            rows.extend(
                self.get_check_feed_between(chrono::DateTime::UNIX_EPOCH, through, chunk)
                    .await?,
            );
        }
        Ok(rows)
    }
}
