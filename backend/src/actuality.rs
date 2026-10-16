use chrono::{DateTime, Utc};
use reqwest::{StatusCode, Url, redirect::Policy};
use scraper::{Html, Selector};
use serde::Serialize;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    Active,
    Inactive,
    Unknown,
}
impl SourceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Unknown => "unknown",
        }
    }
}
#[derive(Debug, Serialize)]
pub struct SourceCheck {
    pub status: SourceStatus,
    pub checked_at: DateTime<Utc>,
    pub http_status: Option<i32>,
    pub reason: String,
}
impl SourceCheck {
    fn new(status: SourceStatus, http: Option<u16>, reason: &str) -> Self {
        Self {
            status,
            checked_at: Utc::now(),
            http_status: http.map(i32::from),
            reason: reason.to_owned(),
        }
    }
}

pub struct SourceChecker {
    client: reqwest::Client,
    next_request: Mutex<Instant>,
    delay: Duration,
}
impl SourceChecker {
    pub fn new(delay_seconds: u64) -> Result<Self, reqwest::Error> {
        let client = crate::network::source_client_builder()
            .timeout(Duration::from_secs(20))
            .redirect(Policy::custom(|attempt| {
                if attempt.previous().len() >= 4 || !supported_url(attempt.url()) {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .build()?;
        Ok(Self {
            client,
            next_request: Mutex::new(Instant::now()),
            delay: Duration::from_secs(delay_seconds.max(5)),
        })
    }
    pub async fn check(&self, url: &str) -> SourceCheck {
        let Ok(url) = Url::parse(url) else {
            return SourceCheck::new(SourceStatus::Unknown, None, "Некорректная ссылка");
        };
        if !supported_url(&url) {
            return SourceCheck::new(
                SourceStatus::Unknown,
                None,
                "Источник или адрес не поддерживается",
            );
        }
        {
            let mut next = self.next_request.lock().await;
            tokio::time::sleep(next.saturating_duration_since(Instant::now())).await;
            *next = Instant::now() + self.delay;
        }
        let mut response = match self.client.get(url.clone()).send().await {
            Ok(response) => response,
            Err(error) => {
                return SourceCheck::new(
                    SourceStatus::Unknown,
                    None,
                    if error.is_timeout() {
                        "Источник не ответил вовремя"
                    } else {
                        "Ошибка соединения с источником"
                    },
                );
            }
        };
        let status = response.status();
        if matches!(status.as_u16(), 401 | 403 | 429)
            || status.is_server_error()
            || status.is_redirection()
        {
            return SourceCheck::new(
                SourceStatus::Unknown,
                Some(status.as_u16()),
                "Источник ограничил доступ или временно недоступен",
            );
        }
        if response.url().path().to_lowercase().contains("captcha") {
            return SourceCheck::new(
                SourceStatus::Unknown,
                Some(status.as_u16()),
                "Источник направил на капчу",
            );
        }
        if !same_listing(response.url(), &url) {
            return SourceCheck::new(
                SourceStatus::Unknown,
                Some(status.as_u16()),
                "Переадресация на другую страницу",
            );
        }
        let mut bytes = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) if bytes.len() + chunk.len() <= 4 * 1024 * 1024 => {
                    bytes.extend_from_slice(&chunk)
                }
                Ok(Some(_)) => {
                    return SourceCheck::new(
                        SourceStatus::Unknown,
                        Some(status.as_u16()),
                        "Страница превышает лимит размера",
                    );
                }
                Ok(None) => break,
                Err(_) => {
                    return SourceCheck::new(
                        SourceStatus::Unknown,
                        Some(status.as_u16()),
                        "Не удалось прочитать страницу",
                    );
                }
            }
        }
        classify(status, &url, &String::from_utf8_lossy(&bytes))
    }
}

fn supported_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.host_str().is_some_and(|host| {
            [
                "avito.ru",
                "cian.ru",
                "youla.ru",
                "youla.io",
                "realty.yandex.ru",
                "realty.ya.ru",
                "domclick.ru",
                "sob.ru",
            ]
            .iter()
            .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
        })
}
fn same_listing(a: &Url, b: &Url) -> bool {
    let host = |url: &Url| {
        let host = url
            .host_str()
            .unwrap_or_default()
            .trim_start_matches("www.")
            .trim_start_matches("m.");
        if host == "realty.ya.ru" {
            "realty.yandex.ru".to_owned()
        } else if host.ends_with(".cian.ru") {
            "cian.ru".to_owned()
        } else {
            host.to_owned()
        }
    };
    host(a) == host(b) && a.path().trim_end_matches('/') == b.path().trim_end_matches('/')
}
fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("Static selector")
}
fn classify(http: StatusCode, url: &Url, body: &str) -> SourceCheck {
    let document = Html::parse_document(body);
    let headings = document.select(&selector("title, h1, [data-marker='item-view/item-closed-warning'], [data-name='OfferUnpublished']"))
        .map(|node| node.text().collect::<String>().to_lowercase()).collect::<Vec<_>>().join(" ");
    if [
        "доступ ограничен",
        "проверка браузера",
        "вы не робот",
        "подтвердите, что вы не робот",
        "проверка, что вы не робот",
        "just a moment",
        "captcha",
        "access denied",
    ]
    .iter()
    .any(|text| headings.contains(text))
    {
        return SourceCheck::new(
            SourceStatus::Unknown,
            Some(http.as_u16()),
            "Капча или ограничение доступа",
        );
    }
    if [
        "объявление снято",
        "объявление удалено",
        "объявление не найдено",
        "объявление больше недоступно",
        "объявление закрыто",
        "объявление в архиве",
    ]
    .iter()
    .any(|text| headings.contains(text))
    {
        return SourceCheck::new(
            SourceStatus::Inactive,
            Some(http.as_u16()),
            "Источник явно сообщает, что объявление недоступно",
        );
    }
    if http == StatusCode::GONE {
        return SourceCheck::new(
            SourceStatus::Inactive,
            Some(410),
            "Источник вернул HTTP 410 Gone",
        );
    }
    if !http.is_success() {
        return SourceCheck::new(
            SourceStatus::Unknown,
            Some(http.as_u16()),
            "HTTP-ошибка без подтверждения снятия объявления",
        );
    }
    for script in document.select(&selector("script[type='application/ld+json']")) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&script.inner_html()) {
            if let Some(status) = structured_status(&value, url, false) {
                return SourceCheck::new(
                    status,
                    Some(http.as_u16()),
                    "Статус предложения в структурированных данных исходной карточки",
                );
            }
        }
    }
    SourceCheck::new(
        SourceStatus::Unknown,
        Some(http.as_u16()),
        "На странице нет однозначного подтверждения актуальности",
    )
}
fn structured_status(
    value: &serde_json::Value,
    expected: &Url,
    inherited_identity: bool,
) -> Option<SourceStatus> {
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .find_map(|value| structured_status(value, expected, inherited_identity)),
        serde_json::Value::Object(object) => {
            let identity = object
                .get("url")
                .and_then(|v| v.as_str())
                .map(|link| {
                    expected
                        .join(link)
                        .is_ok_and(|url| same_listing(&url, expected))
                })
                .unwrap_or(inherited_identity);
            let is_offer = object.get("@type").is_some_and(|kind| {
                kind.as_str() == Some("Offer")
                    || kind.as_array().is_some_and(|types| {
                        types.iter().any(|kind| kind.as_str() == Some("Offer"))
                    })
            });
            let is_product = object.get("@type").and_then(|kind| kind.as_str()) == Some("Product");
            if identity && is_offer {
                if let Some(availability) =
                    object.get("availability").and_then(|value| value.as_str())
                {
                    match availability.rsplit('/').next().unwrap_or(availability) {
                        "InStock" => return Some(SourceStatus::Active),
                        "SoldOut" | "OutOfStock" | "Discontinued" => {
                            return Some(SourceStatus::Inactive);
                        }
                        _ => {}
                    }
                }
            }
            object.values().find_map(|value| {
                structured_status(
                    value,
                    expected,
                    identity && (is_offer || is_product || inherited_identity),
                )
            })
        }
        _ => None,
    }
}
