use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct ApiResponse {
    pub code: i32,
    pub data: Option<Vec<Listing>>,
    #[serde(alias = "error")]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Listing {
    pub id: i64,
    pub url: String,
    pub title: Option<String>,
    pub price: Option<i64>,
    pub time: Option<String>,
    pub phone: Option<String>,
    pub phone_operator: Option<String>,
    pub person: Option<String>,
    pub contactname: Option<String>,
    pub person_type: Option<String>,
    pub person_type_id: Option<i32>,
    pub city: Option<String>,
    pub metro: Option<String>,
    pub address: Option<String>,
    pub description: Option<String>,
    pub nedvigimost_type: Option<String>,
    pub nedvigimost_type_id: Option<serde_json::Value>, // can be string or int
    pub avitoid: Option<serde_json::Value>,             // can be string or int
    pub source: Option<String>,
    pub source_id: Option<i32>,
    pub images: Option<Vec<Image>>,
    pub params: Option<serde_json::Value>,
    pub params2: Option<serde_json::Value>,
    pub cat1_id: Option<i32>,
    pub cat2_id: Option<i32>,
    pub cat1: Option<String>,
    pub cat2: Option<String>,
    pub coords: Option<Coords>,
    pub region: Option<String>,
    pub city1: Option<String>,
    pub count_ads_same_phone: Option<i32>,
    pub phone_protected: Option<i32>,
    pub phone_region: Option<String>,
    pub km_do_metro: Option<f64>,
    pub price_metric: Option<String>,
    pub time_source_created: Option<String>,
    pub time_source_updated: Option<String>,
    pub is_actual: Option<i32>,
    pub metro_only: Option<String>,
    pub district_only: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Image {
    pub imgurl: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Coords {
    pub lat: Option<String>,
    pub lng: Option<String>,
}

#[derive(Debug, Serialize, Default)]
pub struct ListingsQuery {
    pub user: String,
    pub token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avitoid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date1: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date2: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price1: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price2: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person_type: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nedvigimost_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startid: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withphone: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withcoords: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_actual: Option<String>,
    /// Площадь от (param[1960] для категории "Квартиры продажа")
    #[serde(rename = "param[1960]", skip_serializing_if = "Option::is_none")]
    pub area_from: Option<i32>,
    /// Площадь до (param[1979] для категории "Квартиры продажа")
    #[serde(rename = "param[1979]", skip_serializing_if = "Option::is_none")]
    pub area_to: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct SingleListingResponse {
    pub code: i32,
    pub data: Option<Listing>,
    #[serde(alias = "error")]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CategoryParamsResponse {
    pub code: i32,
    pub data: Option<Vec<CategoryParam>>,
    #[serde(alias = "error")]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CategoryParam {
    pub id: i32,
    pub name: String,
    pub values: Option<Vec<ParamValue>>,
}

#[derive(Debug, Deserialize)]
pub struct ParamValue {
    pub id: i32,
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct CheckFeedResponse {
    pub code: i32,
    pub data: Option<Vec<CheckFeedItem>>,
    #[serde(alias = "error")]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CheckFeedItem {
    pub id: i64,
    pub avitoid: Option<String>,
    pub source_id: Option<i32>,
    pub is_actual: i32,
    pub time: Option<String>,
}

#[derive(Debug, Serialize, Default)]
pub struct CheckFeedQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    pub user: String,
    pub token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date1: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date2: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_actual: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avitoid: Option<String>,
}
