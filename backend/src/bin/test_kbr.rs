use backend::ads_api::{AdsApiClient, models::ListingsQuery};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let user = std::env::var("ADS_API_USER").expect("ADS_API_USER not set");
    let token = std::env::var("ADS_API_TOKEN").expect("ADS_API_TOKEN not set");

    let client = AdsApiClient::new(user, token);

    // Test 1: Вся республика
    println!("=== Тест 1: Вся Кабардино-Балкария ===\n");
    let query = ListingsQuery {
        category_id: Some("2".to_string()), // Квартиры
        city: Some("Кабардино-Балкария".to_string()),
        nedvigimost_type: Some("1".to_string()), // Продам
        limit: Some(10),
        withphone: Some(0),
        ..Default::default()
    };
    test_query(&client, query).await;

    // Пауза из-за rate limit
    println!("\n--- Ждём 6 сек (rate limit) ---\n");
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;

    // Test 2: Конкретный город - Нальчик
    println!("=== Тест 2: Только Нальчик ===\n");
    let query = ListingsQuery {
        category_id: Some("2".to_string()),
        city: Some("Нальчик".to_string()),
        nedvigimost_type: Some("1".to_string()),
        limit: Some(10),
        withphone: Some(0),
        ..Default::default()
    };
    test_query(&client, query).await;

    // Пауза
    println!("\n--- Ждём 6 сек (rate limit) ---\n");
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;

    // Test 3: Несколько городов через |
    println!("=== Тест 3: Нальчик или Прохладный ===\n");
    let query = ListingsQuery {
        category_id: Some("2".to_string()),
        city: Some("Нальчик|Прохладный".to_string()),
        nedvigimost_type: Some("1".to_string()),
        limit: Some(10),
        withphone: Some(0),
        ..Default::default()
    };
    test_query(&client, query).await;

    Ok(())
}

async fn test_query(client: &AdsApiClient, query: ListingsQuery) {
    match client.get_listings(query).await {
        Ok(listings) => {
            println!("Найдено: {} объявлений\n", listings.len());
            for (i, l) in listings.iter().take(5).enumerate() {
                println!(
                    "{}. {}",
                    i + 1,
                    l.title.as_deref().unwrap_or("Без названия")
                );
                println!("   Город: {:?}", l.city);
                println!("   Источник: {:?}", l.source);
                println!("   URL: {}", l.url);
                println!();
            }
            if listings.len() > 5 {
                println!("   ... и ещё {} объявлений", listings.len() - 5);
            }
        }
        Err(e) => {
            println!("Ошибка: {:?}", e);
        }
    }
}
