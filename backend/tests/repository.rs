use backend::ads_api::models::Listing;
use backend::db::{DbListing, ImageAnalysisResult, ImageStatus, ListingAnalysisResult, Repository};
use sqlx::PgPool;

#[sqlx::test]
async fn listing_and_image_analysis_round_trip(pool: PgPool) -> anyhow::Result<()> {
    let repository = Repository::new(pool);
    // Missing optional source fields exercise SQL NULL binding and decoding.
    let source: Listing = serde_json::from_value(serde_json::json!({
        "id": 1,
        "url": "https://example.test/listing/1"
    }))?;
    let (listing, _) = DbListing::from_api_listing(&source, "Кабардино-Балкария");
    repository
        .upsert_listing_with_images(&listing, &["https://example.test/photo.jpg".to_owned()])
        .await?;

    let images = repository.get_images_for_listing(1).await?;
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].status, ImageStatus::Pending);
    assert_eq!(repository.get_images_for_download(10).await?.len(), 1);

    repository
        .update_image_download(
            images[0].id,
            ImageStatus::Downloaded,
            Some("1/0.jpg"),
            Some(200),
            None,
        )
        .await?;
    assert_eq!(repository.get_images_for_ai_analysis(10).await?.len(), 1);
    repository
        .save_image_analysis(
            images[0].id,
            &ImageAnalysisResult {
                image_type: "interior".to_owned(),
                room: None,
                condition: Some(75),
                confidence: 90,
                issues: vec![],
                description: "Жилая комната".to_owned(),
            },
            "test-model",
        )
        .await?;

    let analyzed_images = repository.get_listing_image_analyses(1).await?;
    assert_eq!(analyzed_images[0].status, ImageStatus::Downloaded);
    assert_eq!(analyzed_images[0].ai_condition, Some(75));
    assert_eq!(analyzed_images[0].ai_room, None);
    repository
        .save_listing_analysis(
            1,
            &ListingAnalysisResult {
                overall_condition: 75,
                is_shell: false,
                move_in_ready: true,
                red_flags: vec![],
                summary: "Можно заселяться".to_owned(),
            },
            "test-model",
        )
        .await?;

    // Both queries must read the migration's analyzed_at column into the public field.
    let saved = repository.get_listing_by_id(1).await?.unwrap();
    assert_eq!(saved.analysis_condition, Some(75));
    assert!(saved.analysis_analyzed_at.is_some());
    assert_eq!(saved.title, None);
    assert_eq!(saved.params, None);
    let (listings, total) = repository
        .get_listings_paginated(
            0, 20, None, None, None, true, true, true, None, None, None, None,
        )
        .await?;
    assert_eq!(total, 1);
    assert_eq!(listings.len(), 1);
    assert!(listings[0].analysis_analyzed_at.is_some());

    let image_stats = repository.get_images_stats().await?;
    assert_eq!(image_stats.total, 1);
    assert_eq!(image_stats.downloaded, 1);
    assert_eq!(image_stats.pending, 0);
    let analysis_stats = repository.get_analysis_stats().await?;
    assert_eq!(analysis_stats.images_analyzed, 1);
    assert_eq!(analysis_stats.listings_analyzed, 1);
    assert_eq!(analysis_stats.listings_pending, 0);
    Ok(())
}
