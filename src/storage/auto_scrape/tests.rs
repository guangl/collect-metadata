use super::*;
use sqlx::{AssertSqlSafe, PgPool, postgres::PgConnectOptions};

#[tokio::test]
#[ignore = "需要隔离的 COLLECT_METADATA_TEST_POSTGRES"]
async fn auto_retry_state_survives_restart_and_fences_expired_workers() {
    let options = std::env::var("COLLECT_METADATA_TEST_POSTGRES")
        .unwrap()
        .parse::<PgConnectOptions>()
        .unwrap();
    let admin = PgPool::connect_with(options.clone()).await.unwrap();
    let schema = format!("auto_retry_test_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&admin)
        .await
        .unwrap();
    let options = options.options([("search_path", &schema)]);
    let store = Store::connect(&options).await.unwrap();
    let event = |generation: &str| AutoEvent {
        name: "Episode-1908.event".into(),
        generation: generation.into(),
        kind: "Episode".into(),
        item_id: "1908".into(),
        backfill: false,
    };
    let queue = "fixture";
    store
        .sync_auto_events(queue, &[event("first")])
        .await
        .unwrap();
    let (a, b) = tokio::join!(store.claim_auto_jobs(queue), store.claim_auto_jobs(queue));
    let mut claims = a.unwrap();
    claims.extend(b.unwrap());
    assert_eq!(
        claims.len(),
        1,
        "concurrent workers must not claim the same job"
    );
    let first = claims.remove(0);
    assert_eq!(first.attempts, 1);
    assert!(
        store
            .finish_auto_job(
                &first,
                "retry",
                Some("emby_unavailable"),
                Some("HTTP 503"),
                600,
                true
            )
            .await
            .unwrap()
    );
    assert!(
        !store
            .finish_auto_job(&first, "completed", None, None, 0, false)
            .await
            .unwrap()
    );
    store.pool.close().await;
    let store = Store::connect(&options).await.unwrap();
    store
        .sync_auto_events(queue, &[event("first")])
        .await
        .unwrap();
    assert!(
        store.claim_auto_jobs(queue).await.unwrap().is_empty(),
        "restart must honor next_retry_at"
    );
    let row=sqlx::query("SELECT attempts,failures,last_error,next_retry_at>now() AS delayed FROM collect_metadata_auto_jobs").fetch_one(&store.pool).await.unwrap();
    assert_eq!(row.get::<i32, _>("attempts"), 1);
    assert_eq!(row.get::<i32, _>("failures"), 1);
    assert_eq!(row.get::<String, _>("last_error"), "HTTP 503");
    assert!(row.get::<bool, _>("delayed"));
    sqlx::query("UPDATE collect_metadata_auto_jobs SET next_retry_at=now()-interval '1 second'")
        .execute(&store.pool)
        .await
        .unwrap();
    let second = store.claim_auto_jobs(queue).await.unwrap().remove(0);
    assert_eq!(second.attempts, 2);
    assert_eq!(second.failures, 1);
    assert!(
        store.claim_auto_jobs(queue).await.unwrap().is_empty(),
        "live lease is exclusive"
    );
    sqlx::query("UPDATE collect_metadata_auto_jobs SET lease_until=now()-interval '1 second'")
        .execute(&store.pool)
        .await
        .unwrap();
    let resumed = store.claim_auto_jobs(queue).await.unwrap().remove(0);
    assert_eq!(resumed.attempts, 3);
    assert!(
        !store
            .finish_auto_job(&second, "failed", None, None, 0, true)
            .await
            .unwrap(),
        "old worker must not overwrite resumed work"
    );
    assert!(!store.renew_auto_job(&second).await.unwrap());
    assert!(store.renew_auto_job(&resumed).await.unwrap());
    assert!(
        store
            .finish_auto_job(&resumed, "completed", None, None, 0, false)
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .sync_auto_events(queue, &[event("first")])
            .await
            .unwrap()[0]
            .status,
        "completed",
        "committed success survives a crash before file cleanup"
    );
    assert!(store.claim_auto_jobs(queue).await.unwrap().is_empty());
    store
        .sync_auto_events(queue, &[event("second")])
        .await
        .unwrap();
    let fresh = store.claim_auto_jobs(queue).await.unwrap().remove(0);
    assert_eq!(fresh.attempts, 1, "a new file generation is a distinct job");
    assert_eq!(fresh.generation, "second");
    assert!(
        store
            .finish_auto_job(
                &fresh,
                "waiting_source",
                Some("awaiting_title"),
                Some("waiting"),
                1800,
                false
            )
            .await
            .unwrap()
    );
    store.sync_auto_events(queue, &[]).await.unwrap();
    let status: String =
        sqlx::query_scalar("SELECT status FROM collect_metadata_auto_jobs WHERE id=$1")
            .bind(fresh.id)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert_eq!(
        status, "waiting_source",
        "a concurrent directory scan must not retire another worker's newer event"
    );
    assert_eq!(store.auto_dependency_due(queue).await.unwrap(), None);
    store
        .set_auto_dependency(queue, Some("Emby restarting"))
        .await
        .unwrap();
    assert_eq!(store.auto_dependency_due(queue).await.unwrap(), Some(false));
    store.pool.close().await;
    let store = Store::connect(&options).await.unwrap();
    assert_eq!(
        store.auto_dependency_due(queue).await.unwrap(),
        Some(false),
        "readiness pause must survive process restart"
    );
    store.set_auto_dependency(queue, None).await.unwrap();
    assert_eq!(store.auto_dependency_due(queue).await.unwrap(), Some(true));
    let state = store.auto_scrape_state().await.unwrap();
    assert_eq!(state["counts"]["completed"], 1);
    assert_eq!(state["counts"]["waiting_source"], 1);
    store.pool.close().await;
    sqlx::query(AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&admin)
        .await
        .unwrap();
}
