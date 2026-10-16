use backend::{
    actuality::{SourceCheck, SourceStatus},
    db::{DbListing, Repository},
};
use chrono::{Duration, Utc};
use sqlx::PgPool;

fn listing(id: i64, age_hours: i64) -> DbListing {
    let raw = serde_json::from_value(serde_json::json!({"id":id,"url":format!("https://realty.ya.ru/offer/{id}/"),"source":"realty.yandex.ru","is_actual":11})).unwrap();
    let (mut l, _) = DbListing::from_api_listing(&raw, "Кабардино-Балкария");
    l.source_time = Some(Utc::now() - Duration::hours(age_hours));
    l
}
fn check(status: SourceStatus, hours_ago: i64) -> SourceCheck {
    SourceCheck {
        status,
        checked_at: Utc::now() - Duration::hours(hours_ago),
        http_status: Some(200),
        reason: "fixture".into(),
        failure_kind: None,
    }
}

#[sqlx::test]
async fn unknown_attempt_preserves_positive_evidence(pool: PgPool) {
    let repo = Repository::new(pool);
    repo.upsert_listing(&listing(1, 48)).await.unwrap();
    repo.save_source_check(1, &check(SourceStatus::Active, 1))
        .await
        .unwrap();
    repo.save_source_check(1, &check(SourceStatus::Unknown, 0))
        .await
        .unwrap();
    let l = repo.get_listing_by_id(1).await.unwrap().unwrap();
    assert_eq!(l.source_check_status.as_deref(), Some("unknown"));
    assert_eq!(l.source_confirmed_status.as_deref(), Some("active"));
    assert_eq!(l.is_active, Some(true));
    let stats = repo
        .get_availability_stats(Utc::now() - Duration::hours(24))
        .await
        .unwrap();
    assert_eq!(stats[0].confirmed, 1);
}

#[sqlx::test]
async fn newer_publication_and_newer_page_win_over_old_feed(pool: PgPool) {
    let repo = Repository::new(pool);
    let mut l = listing(1, 2);
    repo.upsert_listing(&l).await.unwrap();
    assert!(
        !repo
            .apply_actuality(1, false, Utc::now() - Duration::hours(24))
            .await
            .unwrap()
    );
    repo.save_source_check(1, &check(SourceStatus::Inactive, 1))
        .await
        .unwrap();
    // Re-reading the old active snapshot must not resurrect a removed listing.
    repo.upsert_listing(&l).await.unwrap();
    assert_eq!(
        repo.get_listing_by_id(1).await.unwrap().unwrap().is_active,
        Some(false)
    );
    l.source_time = Some(Utc::now());
    repo.upsert_listing(&l).await.unwrap();
    assert_eq!(
        repo.get_listing_by_id(1).await.unwrap().unwrap().is_active,
        Some(true)
    );
    assert_eq!(
        repo.get_availability_stats(Utc::now() - Duration::hours(24))
            .await
            .unwrap()[0]
            .confirmed,
        0
    );
}

#[sqlx::test]
async fn source_pause_and_favorites_are_independent(pool: PgPool) {
    let repo = Repository::new(pool.clone());
    repo.upsert_listing(&listing(1, 48)).await.unwrap();
    repo.upsert_listing(&listing(2, 1)).await.unwrap();
    sqlx::query!("INSERT INTO listing_reviews(listing_id,favorite) VALUES (1,true)")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.get_source_check_queue("realty.yandex.ru", 1, 24)
            .await
            .unwrap(),
        vec![1]
    );
    let mut blocked = check(SourceStatus::Unknown, 0);
    blocked.failure_kind = Some("blocked");
    repo.record_source_attempt("cian.ru", &blocked)
        .await
        .unwrap();
    assert!(!repo.source_ready("cian.ru").await.unwrap());
    assert!(repo.source_ready("realty.yandex.ru").await.unwrap());
    assert_eq!(repo.feed_sync_ids(true).await.unwrap(), vec![1, 2]);
    repo.mark_feed_history_synced(&[1], Utc::now())
        .await
        .unwrap();
    assert_eq!(repo.feed_sync_ids(true).await.unwrap(), vec![2]);
    assert_eq!(repo.recent_reparse_request(1).await.unwrap(), None);
    repo.save_reparse_request(1, true).await.unwrap();
    assert_eq!(repo.recent_reparse_request(1).await.unwrap(), Some(true));
}
