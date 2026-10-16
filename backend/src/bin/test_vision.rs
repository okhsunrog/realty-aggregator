use std::path::PathBuf;

use backend::config::Config;
use backend::db::{DbImage, ImageStatus, ListingForAnalysis};
use backend::vision::{ImageAnalyzer, ListingAnalyzer};
use base64::Engine;
use clap::Parser;

#[derive(Parser)]
#[command(about = "Analyze a JPEG with both configured model layers without changing the database")]
struct Args {
    image: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let args = Args::parse();
    let config = Config::load_default()?;
    let api_key = std::env::var(&config.vision.api_key_env)
        .map_err(|_| anyhow::anyhow!("{} is not set", config.vision.api_key_env))?;
    let image = std::fs::read(&args.image)?;
    anyhow::ensure!(
        image.starts_with(&[0xff, 0xd8, 0xff]),
        "Provide a JPEG image"
    );
    let encoded = base64::engine::general_purpose::STANDARD.encode(image);
    let image_analyzer = ImageAnalyzer::new(
        api_key.clone(),
        config.vision.image_model.clone(),
        config.vision.api_base_url.clone(),
    );
    let image_analysis = image_analyzer.analyze_base64(&encoded).await?;
    println!("Image analysis ({}):", image_analyzer.model());
    println!("{}", serde_json::to_string_pretty(&image_analysis)?);

    // Synthetic inputs verify the second layer without collecting real listings.
    let image_record: DbImage = serde_json::from_value(serde_json::json!({
        "id": 1,
        "listing_id": 1,
        "image_index": 0,
        "source_url": "https://example.test/photo.jpg",
        "status": ImageStatus::Downloaded,
        "download_attempts": 1,
        "ai_image_type": image_analysis.image_type,
        "ai_room": image_analysis.room,
        "ai_condition": image_analysis.condition,
        "ai_confidence": image_analysis.confidence,
        "ai_issues": image_analysis.issues,
        "ai_description": image_analysis.description
    }))?;
    let listing: ListingForAnalysis = serde_json::from_value(serde_json::json!({
        "id": 1,
        "source": "test",
        "title": "Тест подключения модели",
        "description": "Синтетическое объявление для проверки подключения. Не реальная квартира."
    }))?;
    let listing_analyzer = ListingAnalyzer::new(
        api_key,
        config.vision.listing_model,
        config.vision.api_base_url,
    );
    let listing_analysis = listing_analyzer.analyze(&listing, &[image_record]).await?;
    println!("Listing analysis ({}):", listing_analyzer.model());
    println!("{}", serde_json::to_string_pretty(&listing_analysis)?);
    Ok(())
}
