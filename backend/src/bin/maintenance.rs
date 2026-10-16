use backend::{
    ads_api::{AdsApiClient, models::ListingsQuery},
    config::Config,
    db::{DbListing, Repository},
    jobs::Collector,
    storage::StorageClient,
    vision::{ImageAnalyzer, ListingAnalyzer},
};
use chrono::{Duration, Utc};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Download the cover photograph for selected records without model requests.
    Covers {
        #[arg(long, value_delimiter = ',', required = true)]
        ids: Vec<i64>,
    },
    /// Export explicitly selected records and downloaded photographs without credentials.
    Export {
        #[arg(long, value_delimiter = ',', required = true)]
        ids: Vec<i64>,
        #[arg(long)]
        directory: PathBuf,
    },
    /// Import a locally exported sample, retaining analyses without calling the model again.
    Import {
        #[arg(long)]
        directory: PathBuf,
    },
    /// Import one bounded page without moving the full collection checkpoint.
    Sample {
        #[arg(long, default_value="20", value_parser=clap::value_parser!(u32).range(1..=100))]
        limit: u32,
        #[arg(long, default_value="7", value_parser=clap::value_parser!(u32).range(1..=90))]
        days: u32,
    },
    /// Check original pages; omit IDs to process the next configured queue batch.
    CheckSources {
        #[arg(long, value_delimiter = ',')]
        ids: Vec<i64>,
    },
    /// Download selected photographs and run both analysis layers for one listing.
    Analyze { id: i64 },
}
#[derive(Serialize, Deserialize)]
struct SnapshotEntry {
    listing: DbListing,
    images: Vec<backend::db::DbImage>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter("backend=info")
        .init();
    let args = Args::parse();
    let config = Arc::new(Config::load_default()?);
    let repository = Arc::new(Repository::new(
        sqlx::PgPool::connect(&std::env::var("DATABASE_URL")?).await?,
    ));
    sqlx::migrate!("./migrations")
        .run(repository.pool())
        .await?;
    if let Command::Covers { ids } = &args.command {
        anyhow::ensure!(ids.len() <= 100, "At most 100 covers");
        let storage = StorageClient::from_env(config.storage.image_path_template.clone())?;
        storage.ensure_bucket_exists().await?;
        let mut downloaded = 0;
        for id in ids {
            let Some(image) = repository
                .get_images_for_listing(*id)
                .await?
                .into_iter()
                .next()
            else {
                continue;
            };
            if image.status == backend::db::ImageStatus::Downloaded || image.download_attempts >= 3
            {
                continue;
            }
            let result = storage
                .download_and_store_single(*id, image.image_index, &image.source_url)
                .await;
            let status = if result.success {
                backend::db::ImageStatus::Downloaded
            } else {
                match result.http_status {
                    Some(404) => backend::db::ImageStatus::NotFound,
                    Some(410) => backend::db::ImageStatus::Gone,
                    _ if image.download_attempts >= 2 => backend::db::ImageStatus::Failed,
                    _ => backend::db::ImageStatus::Pending,
                }
            };
            repository
                .update_image_download(
                    image.id,
                    status,
                    result.s3_key.as_deref(),
                    result.http_status.map(i32::from),
                    result.error.as_deref(),
                )
                .await?;
            if result.success {
                downloaded += 1;
            }
        }
        println!("Downloaded {downloaded} cover photographs");
        return Ok(());
    }
    if let Command::Export { ids, directory } = &args.command {
        anyhow::ensure!(ids.len() <= 100, "At most 100 records per export");
        std::fs::create_dir_all(directory)?;
        let storage = StorageClient::from_env(config.storage.image_path_template.clone())?;
        let mut entries = Vec::new();
        for id in ids {
            let listing = repository
                .get_listing_by_id(*id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Missing listing {id}"))?;
            let images = repository.get_images_for_listing(*id).await?;
            for image in &images {
                if let Some(key) = image
                    .s3_key
                    .as_deref()
                    .filter(|_| image.status == backend::db::ImageStatus::Downloaded)
                {
                    let bytes = storage.get_image_by_key(key).await?;
                    std::fs::write(directory.join(format!("{}.image", image.id)), bytes)?;
                }
            }
            entries.push(SnapshotEntry { listing, images });
        }
        std::fs::write(
            directory.join("snapshot.json"),
            serde_json::to_vec(&entries)?,
        )?;
        println!("Exported {} records", entries.len());
        return Ok(());
    }
    if let Command::Import { directory } = &args.command {
        let entries: Vec<SnapshotEntry> =
            serde_json::from_slice(&std::fs::read(directory.join("snapshot.json"))?)?;
        anyhow::ensure!(entries.len() <= 100, "At most 100 records per import");
        let storage = StorageClient::from_env(config.storage.image_path_template.clone())?;
        storage.ensure_bucket_exists().await?;
        for entry in &entries {
            let l = &entry.listing;
            repository
                .upsert_listing_with_images(
                    l,
                    &entry
                        .images
                        .iter()
                        .map(|image| image.source_url.clone())
                        .collect::<Vec<_>>(),
                )
                .await?;
            let current = repository
                .get_listing_by_id(l.id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Missing imported listing"))?;
            if current.source_time != l.source_time {
                continue;
            }
            let targets = repository.get_images_for_listing(l.id).await?;
            for image in &entry.images {
                let target = targets
                    .iter()
                    .find(|target| target.image_index == image.image_index)
                    .ok_or_else(|| anyhow::anyhow!("Missing target image"))?;
                if target.source_url != image.source_url {
                    continue;
                }
                if image.status == backend::db::ImageStatus::Downloaded {
                    let key = storage.image_path(l.id, image.image_index);
                    let bytes = std::fs::read(directory.join(format!("{}.image", image.id)))?;
                    storage.put_image_by_key(&key, &bytes).await?;
                    repository
                        .update_image_download(
                            target.id,
                            backend::db::ImageStatus::Downloaded,
                            Some(&key),
                            Some(200),
                            None,
                        )
                        .await?;
                    if let (Some(kind), Some(confidence), Some(description), Some(model)) = (
                        &image.ai_image_type,
                        image.ai_confidence,
                        &image.ai_description,
                        &image.ai_model,
                    ) {
                        if image.ai_analyzed_at.is_some() {
                            repository
                                .save_image_analysis(
                                    target.id,
                                    &backend::db::ImageAnalysisResult {
                                        image_type: kind.clone(),
                                        room: image.ai_room.clone(),
                                        condition: image.ai_condition,
                                        confidence,
                                        description: description.clone(),
                                        issues: image.ai_issues.clone().unwrap_or_default(),
                                    },
                                    model,
                                )
                                .await?;
                        }
                    }
                }
            }
            if let (Some(summary), Some(model)) = (&l.analysis_summary, &l.analysis_model) {
                if l.analysis_analyzed_at.is_some() {
                    repository
                        .save_listing_analysis(
                            l.id,
                            &backend::db::ListingAnalysisResult {
                                overall_condition: l.analysis_condition,
                                is_shell: l.analysis_is_shell,
                                move_in_ready: l.analysis_move_in_ready,
                                red_flags: l.analysis_red_flags.clone().unwrap_or_default(),
                                summary: summary.clone(),
                            },
                            model,
                        )
                        .await?;
                }
            }
            if let (Some(status), Some(checked_at)) = (&l.source_check_status, l.source_checked_at)
            {
                let status = match status.as_str() {
                    "active" => backend::actuality::SourceStatus::Active,
                    "inactive" => backend::actuality::SourceStatus::Inactive,
                    _ => backend::actuality::SourceStatus::Unknown,
                };
                repository
                    .save_source_check(
                        l.id,
                        &backend::actuality::SourceCheck {
                            status,
                            checked_at,
                            http_status: l.source_check_http_status,
                            reason: l.source_check_reason.clone().unwrap_or_default(),
                        },
                    )
                    .await?;
            }
        }
        println!(
            "Imported {} records; collection checkpoint unchanged",
            entries.len()
        );
        return Ok(());
    }
    let api = Arc::new(
        AdsApiClient::new(
            std::env::var("ADS_API_USER")?,
            std::env::var("ADS_API_TOKEN")?,
        )
        .with_request_delay(config.api.request_delay_seconds),
    );
    if let Command::Sample { limit, days } = args.command {
        let c = &config.api.query;
        let through = Utc::now() + Duration::minutes(150);
        let rows = api
            .get_listings(ListingsQuery {
                limit: Some(limit as i32),
                withphone: Some(0),
                category_id: Some(c.category_id.to_string()),
                nedvigimost_type: Some(c.nedvigimost_type.to_string()),
                city: Some(c.city.clone()),
                is_actual: c.is_actual.clone(),
                price1: c.price1,
                price2: c.price2,
                area_from: c.area_from,
                area_to: c.area_to,
                person_type: c.person_type,
                source: c.source.clone(),
                withcoords: c.withcoords,
                date1: Some(
                    (through - Duration::days(days as i64))
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string(),
                ),
                date2: Some(through.format("%Y-%m-%d %H:%M:%S").to_string()),
                sort: Some("desc".to_owned()),
                ..Default::default()
            })
            .await?;
        let mut ids = Vec::new();
        for row in rows {
            let (listing, photos) = DbListing::from_api_listing(&row, &c.city);
            repository
                .upsert_listing_with_images(&listing, &photos)
                .await?;
            ids.push(listing.id);
        }
        ids.sort_unstable();
        ids.dedup();
        println!(
            "{}",
            serde_json::json!({"imported_ids":ids,"checkpoint_changed":false})
        );
        return Ok(());
    }
    let mut collector = Collector::new(api, repository, config.clone());
    match args.command {
        Command::CheckSources { ids } => {
            anyhow::ensure!(ids.len() <= 100, "At most 100 IDs");
            let count = collector
                .run_source_checks(&ids)
                .await
                .map_err(anyhow::Error::msg)?;
            println!("Checked {count} source pages");
        }
        Command::Analyze { id } => {
            let storage = Arc::new(StorageClient::from_env(
                config.storage.image_path_template.clone(),
            )?);
            storage.ensure_bucket_exists().await?;
            let key = std::env::var(&config.vision.api_key_env)?;
            collector = collector
                .with_storage_client(storage)
                .with_image_analyzer(Arc::new(ImageAnalyzer::new(
                    key.clone(),
                    config.vision.image_model.clone(),
                    config.vision.api_base_url.clone(),
                )))
                .with_listing_analyzer(Arc::new(ListingAnalyzer::new(
                    key.clone(),
                    config.vision.listing_model.clone(),
                    config.vision.api_base_url.clone(),
                )))
                .with_vision_api_key(key);
            collector
                .run_listing_pipeline(id)
                .await
                .map_err(anyhow::Error::msg)?;
            println!("Analyzed listing {id}");
        }
        Command::Covers { .. }
        | Command::Sample { .. }
        | Command::Export { .. }
        | Command::Import { .. } => unreachable!(),
    }
    Ok(())
}
