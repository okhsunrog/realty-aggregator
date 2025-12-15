use backend::ads_api::{AdsApiClient, models::ListingsQuery};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let user = std::env::var("ADS_API_USER").expect("ADS_API_USER not set");
    let token = std::env::var("ADS_API_TOKEN").expect("ADS_API_TOKEN not set");

    println!("Testing ads-api.ru with user: {}", user);

    let client = AdsApiClient::new(user, token);

    // Test: get 5 listings from Moscow, category 2 (apartments), type 1 (sale)
    let query = ListingsQuery {
        user: String::new(),
        token: String::new(),
        category_id: Some("2".to_string()),
        city: Some("Москва".to_string()),
        nedvigimost_type: Some("1".to_string()), // Продам
        limit: Some(5),
        withphone: Some(0),
        ..Default::default()
    };

    println!("\nFetching 5 apartments for sale in Moscow...\n");

    match client.get_listings(query).await {
        Ok(listings) => {
            println!("Got {} listings:\n", listings.len());
            for (i, listing) in listings.iter().enumerate() {
                println!(
                    "{}. {} ",
                    i + 1,
                    listing.title.as_deref().unwrap_or("No title")
                );
                println!("   Price: {:?}", listing.price);
                println!("   City: {:?}", listing.city);
                println!("   URL: {}", listing.url);
                println!("   Source: {:?}", listing.source);
                println!();
            }
        }
        Err(e) => {
            println!("Error: {:?}", e);
        }
    }

    Ok(())
}
