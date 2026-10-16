use backend::{ads_api::AdsApiClient, config::Config, db::Repository, jobs::Collector};
use chrono::{Duration, NaiveDateTime, Utc};
use clap::Parser;
use std::sync::Arc;

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
    let client = Arc::new(AdsApiClient::new(
        std::env::var("ADS_API_USER")?,
        std::env::var("ADS_API_TOKEN")?,
    ));
    let repo = Arc::new(Repository::new(
        sqlx::PgPool::connect(&std::env::var("DATABASE_URL")?).await?,
    ));
    let collector = Collector::new(
        client.clone(),
        repo.clone(),
        Arc::new(Config::load_default()?),
    );
    let ids = repo.get_listing_ids().await?;
    let through = Utc::now();
    let mut count = 0;
    for chunk in ids.chunks(250) {
        let checks = client
            .get_check_feed_between(through - Duration::days(args.days as i64), through, chunk)
            .await?;
        if args.apply {
            collector
                .import_recorded_checks(checks.clone())
                .await
                .map_err(anyhow::Error::msg)?;
        }
        for check in checks {
            let Some(time) = check.time.as_deref() else {
                anyhow::bail!("Missing check timestamp");
            };
            let checked_at = NaiveDateTime::parse_from_str(time, "%Y-%m-%d %H:%M:%S")?.and_utc()
                - Duration::hours(3);
            count += 1;
            println!(
                "{}: provider_status={}, checked_at={}",
                check.id, check.is_actual, checked_at
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
