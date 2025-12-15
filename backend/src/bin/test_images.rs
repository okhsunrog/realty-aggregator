use backend::ads_api::{AdsApiClient, models::ListingsQuery};
use std::path::Path;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let user = std::env::var("ADS_API_USER").expect("ADS_API_USER not set");
    let token = std::env::var("ADS_API_TOKEN").expect("ADS_API_TOKEN not set");

    let client = AdsApiClient::new(user, token);

    // Получаем объявления с изображениями
    println!("Получаем объявления из Кабардино-Балкарии...\n");

    let query = ListingsQuery {
        category_id: Some("2".to_string()),
        city: Some("Кабардино-Балкария".to_string()),
        nedvigimost_type: Some("1".to_string()),
        limit: Some(5),
        withphone: Some(0),
        ..Default::default()
    };

    let listings = client.get_listings(query).await?;

    // Находим объявление с изображениями
    let listing_with_images = listings
        .iter()
        .find(|l| l.images.as_ref().map(|i| !i.is_empty()).unwrap_or(false));

    let Some(listing) = listing_with_images else {
        println!("Не найдено объявлений с изображениями");
        return Ok(());
    };

    println!(
        "Найдено объявление: {}",
        listing.title.as_deref().unwrap_or("Без названия")
    );
    println!("URL: {}", listing.url);

    let images = listing.images.as_ref().unwrap();
    println!("\nКоличество изображений: {}\n", images.len());

    // Показываем все URL изображений
    for (i, img) in images.iter().enumerate() {
        println!("  {}. {}", i + 1, img.imgurl);
    }

    // Пробуем скачать первое изображение
    if let Some(first_img) = images.first() {
        println!("\n--- Пробуем скачать первое изображение ---\n");
        println!("URL: {}", first_img.imgurl);

        let http_client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()?;

        match http_client.get(&first_img.imgurl).send().await {
            Ok(response) => {
                println!("HTTP статус: {}", response.status());
                println!("Content-Type: {:?}", response.headers().get("content-type"));
                println!(
                    "Content-Length: {:?}",
                    response.headers().get("content-length")
                );

                if response.status().is_success() {
                    let bytes = response.bytes().await?;
                    println!("\nСкачано: {} байт", bytes.len());

                    // Сохраняем для проверки
                    let test_dir = Path::new("/tmp/realty_images");
                    std::fs::create_dir_all(test_dir)?;

                    let filename = test_dir.join("test_image.jpg");
                    std::fs::write(&filename, &bytes)?;
                    println!("Сохранено в: {:?}", filename);

                    // Проверяем что это валидное изображение (первые байты)
                    if bytes.len() > 10 {
                        let magic = &bytes[0..3];
                        let format = match magic {
                            [0xFF, 0xD8, 0xFF] => "JPEG",
                            [0x89, 0x50, 0x4E] => "PNG",
                            [0x47, 0x49, 0x46] => "GIF",
                            [0x52, 0x49, 0x46] => "WebP",
                            _ => "Unknown",
                        };
                        println!("Формат изображения: {}", format);
                    }
                } else {
                    println!("\nОшибка загрузки: {}", response.status());
                    let text = response.text().await?;
                    println!("Ответ: {}", &text[..text.len().min(500)]);
                }
            }
            Err(e) => {
                println!("Ошибка запроса: {:?}", e);
            }
        }
    }

    Ok(())
}
