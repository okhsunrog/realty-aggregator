use backend::{ads_api::AdsApiClient, db::Repository};
use chrono::{Duration, NaiveDateTime, Utc};
use clap::Parser;

#[derive(Parser)]
#[command(about = "Import recorded actuality checks; missing IDs remain unchanged")]
struct Args {
    #[arg(short, long, default_value = "60", value_parser = clap::value_parser!(u32).range(1..=3650))]
    days: u32,
    /// Apply recorded checks. Without this flag the database is unchanged.
    #[arg(short, long)]
    apply: bool,
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let args = Args::parse();
    let client = AdsApiClient::new(
        std::env::var("ADS_API_USER")?,
        std::env::var("ADS_API_TOKEN")?,
    );
    let repo = Repository::new(sqlx::PgPool::connect(&std::env::var("DATABASE_URL")?).await?);
    let ids = repo.get_listing_ids().await?;
    let through = Utc::now();
    let mut count = 0;
    for chunk in ids.chunks(250) {
        let checks = client
            .get_check_feed_between(through - Duration::days(args.days as i64), through, chunk)
            .await?;
        for check in checks {
            let Some(time) = check.time.as_deref() else {
                anyhow::bail!("Missing check timestamp");
            };
            let checked_at = NaiveDateTime::parse_from_str(time, "%Y-%m-%d %H:%M:%S")?.and_utc()
                - Duration::hours(3);
            if args.apply {
                repo.apply_actuality(check.id, check.is_actual != 0, checked_at)
                    .await?;
            }
            count += 1;
            println!(
                "{}: active={}, checked_at={}",
                check.id,
                check.is_actual != 0,
                checked_at
            );
        }
    }
    println!(
        "{} recorded checks; mode={}. Missing IDs unchanged.",
        count,
        if args.apply { "apply" } else { "dry run" }
    );
    Ok(())
}
