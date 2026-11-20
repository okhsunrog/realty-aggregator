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
    pub failure_kind: Option<&'static str>,
}
impl SourceCheck {
    fn new(status: SourceStatus, http: Option<u16>, reason: &str) -> Self {
        Self {
            status,
            checked_at: Utc::now(),
            http_status: http.map(i32::from),
            reason: reason.to_owned(),
            failure_kind: None,
        }
    }
    fn failed(mut self, kind: &'static str) -> Self {
        self.failure_kind = Some(kind);
        self
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
                )
                .failed("network");
            }
        };
        let status = response.status();
        if matches!(status.as_u16(), 401 | 403 | 429 | 439)
            || status.is_server_error()
            || status.is_redirection()
        {
            return SourceCheck::new(
                SourceStatus::Unknown,
                Some(status.as_u16()),
                "Источник ограничил доступ или временно недоступен",
            )
            .failed(if status.is_server_error() {
                "network"
            } else {
                "blocked"
            });
        }
        if response.url().path().to_lowercase().contains("captcha") {
            return SourceCheck::new(
                SourceStatus::Unknown,
                Some(status.as_u16()),
                "Источник направил на капчу",
            )
            .failed("blocked");
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
        } else if host == "youla.io" {
            "youla.ru".to_owned()
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
        )
        .failed("blocked");
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
    if matches!(
        url.host_str(),
        Some("youla.ru" | "www.youla.ru" | "youla.io" | "www.youla.io")
    ) && let Some(status) = youla_status(&document, url)
    {
        return SourceCheck::new(
            status,
            Some(http.as_u16()),
            "Состояние исходной карточки Юлы",
        );
    }
    let mut observations = Vec::new();
    for script in document.select(&selector("script[type='application/ld+json']")) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&script.inner_html()) {
            structured_statuses(&value, url, false, &mut observations);
        }
    }
    if let Some(&status) = observations.first() {
        if observations.iter().all(|v| *v == status) {
            return SourceCheck::new(
                status,
                Some(http.as_u16()),
                "Статус предложения в структурированных данных исходной карточки",
            );
        }
        return SourceCheck::new(
            SourceStatus::Unknown,
            Some(http.as_u16()),
            "Источник вернул противоречивые статусы предложения",
        );
    }
    SourceCheck::new(
        SourceStatus::Unknown,
        Some(http.as_u16()),
        "На странице нет однозначного подтверждения актуальности",
    )
}

fn youla_status(document: &Html, url: &Url) -> Option<SourceStatus> {
    let id = url.path().trim_end_matches('/').rsplit('-').next()?;
    if id.len() != 24 || !id.bytes().all(|v| v.is_ascii_hexdigit()) {
        return None;
    }
    for script in document.select(&selector("script")) {
        let text = script.inner_html();
        let Some(state) = text
            .split("window.__YOULA_STATE__")
            .skip(1)
            .find_map(|state| state.trim_start().strip_prefix('=').map(str::trim_start))
        else {
            continue;
        };
        // Parse only the JSON value; never execute inline JavaScript.
        let Some(Ok(value)) = serde_json::Deserializer::from_str(state)
            .into_iter::<serde_json::Value>()
            .next()
        else {
            continue;
        };
        let Some(products) = value
            .pointer("/entities/products")
            .and_then(|v| v.as_array())
        else {
            continue;
        };
        let Some(product) = products
            .iter()
            .find(|v| v.get("id").and_then(|v| v.as_str()) == Some(id))
        else {
            continue;
        };
        let flag = |key: &str| product.get(key).and_then(|v| v.as_bool());
        if ["isArchived", "isDeleted", "isSold", "isBlocked"]
            .iter()
            .any(|key| flag(key) == Some(true))
            || flag("isPublished") == Some(false)
        {
            return Some(SourceStatus::Inactive);
        }
        if flag("isPublished") == Some(true)
            && ["isArchived", "isDeleted", "isSold", "isBlocked"]
                .iter()
                .all(|key| flag(key) == Some(false))
        {
            return Some(SourceStatus::Active);
        }
    }
    None
}

/// Identity used by the documented ADS API lookup; never infer activity from it.
pub fn source_identity(source: &str, link: &str) -> Option<(i32, String)> {
    let url = Url::parse(link).ok()?;
    if !supported_url(&url) {
        return None;
    }
    let tail = url.path().trim_end_matches('/').rsplit('/').next()?;
    let (code, id) = match source {
        "avito.ru" => (1, tail.rsplit('_').next()?),
        "realty.yandex.ru" => (3, tail),
        "cian.ru" => (4, tail),
        "youla.io" => (6, tail.rsplit('-').next()?),
        _ => return None,
    };
    if id.is_empty()
        || !id.bytes().all(|v| {
            if code == 6 {
                v.is_ascii_hexdigit()
            } else {
                v.is_ascii_digit()
            }
        })
    {
        return None;
    }
    Some((code, id.to_owned()))
}
fn structured_statuses(
    value: &serde_json::Value,
    expected: &Url,
    inherited_identity: bool,
    found: &mut Vec<SourceStatus>,
) {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                structured_statuses(value, expected, inherited_identity, found);
            }
        }
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
            let is_type = |name| {
                object.get("@type").is_some_and(|kind| {
                    kind.as_str() == Some(name)
                        || kind.as_array().is_some_and(|types| {
                            types.iter().any(|kind| kind.as_str() == Some(name))
                        })
                })
            };
            if identity
                && is_type("Offer")
                && let Some(availability) = object.get("availability").and_then(|v| v.as_str())
            {
                match availability.rsplit('/').next().unwrap_or(availability) {
                    "InStock" => found.push(SourceStatus::Active),
                    "SoldOut" | "OutOfStock" | "Discontinued" => found.push(SourceStatus::Inactive),
                    _ => {}
                }
            }
            for (key, value) in object {
                // Only a Product's own offers inherit its identity, never recommendations.
                structured_statuses(
                    value,
                    expected,
                    identity && is_type("Product") && key == "offers",
                    found,
                );
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youla_alias_preserves_identity() {
        let old = Url::parse("https://youla.io/city/flat-6a8bf9baf1f55495ad07c8dc").unwrap();
        let new = Url::parse("https://youla.ru/city/flat-6a8bf9baf1f55495ad07c8dc/").unwrap();
        assert!(same_listing(&old, &new));
        assert!(!same_listing(
            &old,
            &Url::parse("https://youla.ru/city/other-6ac0dabb7d4c6ad3890de0b4").unwrap()
        ));
    }

    #[test]
    fn archived_you_la_can_still_have_published_flag() {
        let url = Url::parse("https://youla.io/city/flat-6a8bf9baf1f55495ad07c8dc").unwrap();
        let body = r#"<title>Неактивно</title><script>const user = window.__YOULA_STATE__ && window.__YOULA_STATE__.auth;</script><script>const previous = window.__YOULA_STATE__; window.__YOULA_STATE__ = {"entities":{"products":[{"id":"6a8bf9baf1f55495ad07c8dc","isPublished":true,"isArchived":true,"isDeleted":false,"isSold":false,"isBlocked":false}]}}; window.other = {};</script>"#;
        assert_eq!(
            classify(StatusCode::OK, &url, body).status,
            SourceStatus::Inactive
        );
        assert_eq!(
            classify(
                StatusCode::OK,
                &url,
                &body.replace("\"isArchived\":true", "\"isArchived\":false")
            )
            .status,
            SourceStatus::Active
        );
    }

    #[test]
    fn youla_missing_flags_and_unrelated_product_are_unknown() {
        let url = Url::parse("https://youla.ru/city/flat-6a8bf9baf1f55495ad07c8dc").unwrap();
        for body in [
            r#"<script>window.__YOULA_STATE__ = {"entities":{"products":[{"id":"6a8bf9baf1f55495ad07c8dc","isPublished":true}]}};</script>"#,
            r#"<script>window.__YOULA_STATE__ = {"entities":{"products":[{"id":"6ac0dabb7d4c6ad3890de0b4","isArchived":true}]}};</script>"#,
        ] {
            assert_eq!(
                classify(StatusCode::OK, &url, body).status,
                SourceStatus::Unknown
            );
        }
    }

    #[test]
    fn recommendations_do_not_determine_listing_availability() {
        let url = Url::parse("https://realty.ya.ru/offer/123/").unwrap();
        let body = r#"<script type="application/ld+json">[{"@type":"Offer","url":"https://realty.yandex.ru/offer/456","availability":"https://schema.org/InStock"},{"@type":"Offer","url":"https://realty.yandex.ru/offer/123","availability":"https://schema.org/OutOfStock"}]</script>"#;
        assert_eq!(
            classify(StatusCode::OK, &url, body).status,
            SourceStatus::Inactive
        );
        assert_eq!(
            classify(
                StatusCode::OK,
                &url,
                &body.replace("/offer/123", "/offer/789")
            )
            .status,
            SourceStatus::Unknown
        );
    }

    #[test]
    fn captcha_and_unqualified_404_never_mean_removed() {
        let url = Url::parse("https://www.cian.ru/sale/flat/123/").unwrap();
        let check = classify(StatusCode::OK, &url, "<title>Вы не робот?</title>");
        assert_eq!(check.status, SourceStatus::Unknown);
        assert_eq!(check.failure_kind, Some("blocked"));
        assert_eq!(
            classify(StatusCode::NOT_FOUND, &url, "<title>Ошибка</title>").status,
            SourceStatus::Unknown
        );
    }

    #[test]
    fn conflicting_metadata_and_nested_recommendations_are_unknown() {
        let url = Url::parse("https://realty.ya.ru/offer/123/").unwrap();
        let conflict = r#"<script type="application/ld+json">[{"@type":"Offer","url":"/offer/123/","availability":"InStock"},{"@type":"Offer","url":"/offer/123/","availability":"OutOfStock"}]</script>"#;
        assert_eq!(
            classify(StatusCode::OK, &url, conflict).status,
            SourceStatus::Unknown
        );
        let unrelated = r#"<script type="application/ld+json">{"@type":"Product","url":"/offer/123/","recommendations":[{"@type":"Offer","availability":"InStock"}]}</script>"#;
        assert_eq!(
            classify(StatusCode::OK, &url, unrelated).status,
            SourceStatus::Unknown
        );
        assert_eq!(
            classify(
                StatusCode::OK,
                &url,
                &unrelated.replace("recommendations", "offers")
            )
            .status,
            SourceStatus::Active
        );
    }
}
