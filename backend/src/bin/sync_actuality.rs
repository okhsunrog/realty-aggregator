use backend::ads_api::{AdsApiClient, models::ListingsQuery};
use backend::db::Repository;
use chrono::{Duration, Utc};
use clap::Parser;
use std::collections::HashSet;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Sync actuality by comparing DB with current API data"
)]
struct Args {
    /// Number of days to fetch from API (default: 60)
    #[arg(short, long, default_value = "60")]
    days: i64,

    /// Actually apply changes (without this flag, only shows what would be changed)
    #[arg(short, long)]
    apply: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    let user = std::env::var("ADS_API_USER").expect("ADS_API_USER not set");
    let token = std::env::var("ADS_API_TOKEN").expect("ADS_API_TOKEN not set");
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");

    let client = AdsApiClient::new(user, token);
    let pool = sqlx::PgPool::connect(&database_url).await?;
    let repo = Repository::new(pool);

    println!(
        "=== Syncing actuality with API (last {} days) ===\n",
        args.days
    );

    // Moscow time is UTC+3, API time is 30 min behind
    let now_moscow = Utc::now() + Duration::hours(3) - Duration::minutes(30);
    let date1 = (now_moscow - Duration::days(args.days))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let date2 = now_moscow.format("%Y-%m-%d %H:%M:%S").to_string();

    println!("Fetching from API: {} to {}", date1, date2);

    // Collect all IDs from API
    let mut api_ids: HashSet<i64> = HashSet::new();
    let mut current_date2 = date2.clone();
    let mut total_fetched = 0;

    loop {
        println!("Fetching batch (date2={})...", current_date2);

        let query = ListingsQuery {
            user: String::new(),
            token: String::new(),
            category_id: Some("2".to_string()),
            city: Some("Кабардино-Балкария".to_string()),
            nedvigimost_type: Some("1".to_string()),
            is_actual: Some("11,1".to_string()),
            price1: Some(1400000),
            price2: Some(2100000),
            withphone: Some(0),
            limit: Some(1000),
            date1: Some(date1.clone()),
            date2: Some(current_date2.clone()),
            sort: Some("desc".to_string()),
            ..Default::default()
        };

        let listings = client.get_listings(query).await?;
        let batch_size = listings.len();
        total_fetched += batch_size;

        for listing in &listings {
            api_ids.insert(listing.id);
        }

        println!(
            "  Got {} listings, total unique IDs: {}",
            batch_size,
            api_ids.len()
        );

        if batch_size < 1000 {
            break;
        }

        // Get oldest time for next batch
        if let Some(oldest) = listings.last().and_then(|l| l.time.as_ref()) {
            current_date2 = oldest.clone();
        } else {
            break;
        }

        tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    }

    println!(
        "\nTotal fetched from API: {} (unique IDs: {})",
        total_fetched,
        api_ids.len()
    );

    // Get all active listings from our DB
    let db_listings = sqlx::query_scalar!("SELECT id FROM listings WHERE is_active = true")
        .fetch_all(repo.pool())
        .await?;

    let db_ids: HashSet<i64> = db_listings.into_iter().collect();

    println!("Active listings in DB: {}", db_ids.len());

    // Find IDs that are in DB but not in API
    let to_deactivate: Vec<i64> = db_ids.difference(&api_ids).copied().collect();
    let still_active: Vec<i64> = db_ids.intersection(&api_ids).copied().collect();

    println!("\n=== Results ===");
    println!("Still active (in both DB and API): {}", still_active.len());
    println!(
        "To deactivate (in DB but not in API): {}",
        to_deactivate.len()
    );

    if to_deactivate.is_empty() {
        println!("\nNo listings to deactivate!");
        return Ok(());
    }

    // Show some examples
    println!(
        "\nFirst 10 IDs to deactivate: {:?}",
        &to_deactivate[..to_deactivate.len().min(10)]
    );

    if args.apply {
        println!("\nApplying changes...");

        // Deactivate in batches
        for chunk in to_deactivate.chunks(100) {
            sqlx::query!(
                "UPDATE listings SET is_active = false WHERE id = ANY($1)",
                chunk
            )
            .execute(repo.pool())
            .await?;
        }

        println!("Deactivated {} listings", to_deactivate.len());
    } else {
        println!("\nDry run - no changes applied. Use --apply to actually deactivate.");
    }

    Ok(())
}
