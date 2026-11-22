use super::*;
use crate::batch::{Claim, Outcome};
use crate::{MediaKind, MediaProvider};
use serde_json::json;
use sqlx::{AssertSqlSafe, PgPool, postgres::PgConnectOptions};

async fn test_database() -> (PgPool, PgConnectOptions, String) {
    let options = std::env::var("COLLECT_METADATA_TEST_POSTGRES")
        .unwrap()
        .parse::<PgConnectOptions>()
        .unwrap();
    let admin = PgPool::connect_with(options.clone()).await.unwrap();
    // The identifier contains a fixed prefix and a hexadecimal UUID.
    let schema = format!("batch_test_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&admin)
        .await
        .unwrap();
    (admin, options.options([("search_path", &schema)]), schema)
}

#[cfg(feature = "web")]
#[tokio::test]
#[ignore = "需要 COLLECT_METADATA_TEST_POSTGRES 指向可创建测试 schema 的 PostgreSQL"]
async fn complete_bangumi_archives_keep_part_order_and_ignore_stale_rows() {
    let (admin, options, schema) = test_database().await;
    let store = Store::connect(&options).await.unwrap();
    sqlx::raw_sql(include_str!("../../../tools/anime-catalog/schema.sql"))
        .execute(&store.pool)
        .await
        .unwrap();
    sqlx::raw_sql(
        "INSERT INTO collect_metadata_anime_subjects
            (bangumi_id,title,format,is_adult,expected_episodes,archive_version)
         VALUES (12,'First','TV',false,2,'current'),(11,'Second','TV',false,1,'current');
         INSERT INTO collect_metadata_anime_episodes
            (bangumi_episode_id,bangumi_id,kind,source_number,archive_version)
         VALUES (50,12,0,25,'current'),(51,12,0,26,'current'),(60,11,0,27,'current'),
                (52,12,1,1,'current'),(53,12,0,28,'older');",
    )
    .execute(&store.pool)
    .await
    .unwrap();
    let episodes = store.archived_bangumi_episodes(&[12, 11]).await.unwrap();
    let sorts: Vec<_> = episodes.iter().map(|entry| entry["sort"].as_u64().unwrap()).collect();
    assert_eq!(sorts, [25, 26, 27]);
    assert!(store.archived_bangumi_episodes(&[12, 99]).await.unwrap().is_empty());
    sqlx::query("UPDATE collect_metadata_anime_subjects SET expected_episodes=2 WHERE bangumi_id=11")
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(store.archived_bangumi_episodes(&[12, 11]).await.unwrap().is_empty());
    store.pool.close().await;
    sqlx::query(AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&admin)
        .await
        .unwrap();
}

async fn claim_when_free(store: &Store) -> Claim {
    for _ in 0..50 {
        if let Some(claim) = store.claim(true, None).await.unwrap() {
            return claim;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("expected a claim");
}

#[tokio::test]
#[ignore = "需要 COLLECT_METADATA_TEST_POSTGRES 指向可创建测试 schema 的 PostgreSQL"]
async fn clearing_history_deletes_standalone_snapshots_and_keeps_batch_results() {
    let (admin, options, schema) = test_database().await;
    let store = Store::connect(&options).await.unwrap();
    let document = json!({
        "query": "ABC-001",
        "fields": {"title": "Fixture", "original_languages": ["ja"]}
    });
    let standalone = store.save("ABC-001", &document).await.unwrap();
    let batch = store
        .create_batch(
            vec![BatchItemInput {
                kind: MediaKind::Av,
                query: "ABC-001".into(),
                source_files: Vec::new(),
            }],
            false,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let claim = claim_when_free(&store).await;
    let outcome = Outcome {
        stage: "success",
        message: None,
        candidates: None,
        result: Some(LookupResponse {
            record: document.clone(),
            saved: None,
            storage_error: None,
        }),
        remembered_match: None,
        output: None,
    };
    assert!(store.finish(&claim, outcome).await.unwrap());

    sqlx::query("ALTER TABLE collect_metadata_snapshots DROP COLUMN history_visible")
        .execute(&store.pool)
        .await
        .unwrap();
    let store = Store::connect(&options).await.unwrap();
    assert_eq!(store.history(None, None, None).await.unwrap().len(), 2);
    assert_eq!(store.history_languages().await.unwrap(), ["ja"]);
    let (first, second) = tokio::join!(store.clear_history(), store.clear_history());
    assert_eq!(first.unwrap() + second.unwrap(), 2);
    assert!(store.history(None, None, None).await.unwrap().is_empty());
    assert!(store.history_languages().await.unwrap().is_empty());
    assert!(store.load(standalone.id, true).await.is_err());
    let batch_result = store
        .batch_row_result(batch, claim.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(batch_result.record, document);
    let retained = batch_result.saved.unwrap().id;
    assert!(store.load(retained, false).await.is_err());

    let fresh = store.save("ABC-001", &document).await.unwrap();
    let history = store
        .history(None, Some(MediaKind::Av), Some("ja"))
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, fresh.id);
    assert!(fresh.id > retained);
    store.pool.close().await;
    sqlx::query(AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&admin)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "需要 COLLECT_METADATA_TEST_POSTGRES 指向可创建测试 schema 的 PostgreSQL"]
async fn correction_reuses_the_row_and_saves_only_a_successful_match() {
    let (admin, options, schema) = test_database().await;
    let store = Store::connect(&options).await.unwrap();
    let files = vec!["Movies/Dark (2017)/Dark S01E01.mkv".to_owned()];
    let batch = store
        .create_batch(
            vec![BatchItemInput {
                kind: MediaKind::Series,
                query: "Dark (2017)".into(),
                source_files: files.clone(),
            }],
            false,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let claim = claim_when_free(&store).await;
    let failure = || Outcome {
        stage: "not_found",
        message: None,
        candidates: None,
        result: None,
        remembered_match: None,
        output: None,
    };
    assert!(store.finish(&claim, failure()).await.unwrap());
    let candidate = CandidateChoice {
        provider: MediaProvider::Tvmaze,
        id: "16788".into(),
    };
    let key = "folder:6:Movies:dark (2017)";
    assert!(
        store
            .correct_batch_match(batch, claim.id, candidate.clone())
            .await
            .unwrap()
    );
    let corrected = claim_when_free(&store).await;
    assert_eq!(corrected.id, claim.id);
    assert_eq!(corrected.query, claim.query);
    assert_eq!(corrected.folder_root.as_deref(), Some("Movies"));
    assert_eq!(corrected.export_revision, 1);
    assert!(corrected.remember_match);
    assert_eq!(corrected.candidate.as_ref().unwrap().id, candidate.id);
    assert_eq!(
        store.batch_files(batch, claim.id).await.unwrap().unwrap(),
        files
    );
    assert!(
        store
            .remembered_match(MediaKind::Series, key)
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.finish(&corrected, failure()).await.unwrap());
    assert!(
        store
            .remembered_match(MediaKind::Series, key)
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.retry_batch_row(batch, claim.id).await.unwrap());
    let retried = claim_when_free(&store).await;
    let outcome = Outcome {
        stage: "success",
        message: None,
        candidates: None,
        output: None,
        result: Some(LookupResponse {
            record: json!({"media_type":"series", "query":"Dark", "fields":{"title":"Dark"}}),
            saved: None,
            storage_error: None,
        }),
        remembered_match: Some((key.into(), candidate)),
    };
    assert!(store.finish(&retried, outcome).await.unwrap());
    let reconnected = Store::connect(&options).await.unwrap();
    assert_eq!(
        reconnected
            .remembered_match(MediaKind::Series, key)
            .await
            .unwrap()
            .unwrap()
            .id,
        "16788"
    );
    assert!(
        reconnected
            .remembered_match(MediaKind::Movie, key)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        reconnected.batch_view(batch).await.unwrap().unwrap().rows[0].stage,
        "success"
    );
    sqlx::query(AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&admin)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "需要 COLLECT_METADATA_TEST_POSTGRES 指向可创建测试 schema 的 PostgreSQL"]
async fn expired_claims_are_reclaimed_and_finishing_is_atomic() {
    let (admin, options, schema) = test_database().await;
    let first = Store::connect(&options).await.unwrap();
    let second = Store::connect(&options).await.unwrap();
    let batch = first
        .create_batch(
            (0..4)
                .map(|index| BatchItemInput {
                    kind: MediaKind::Av,
                    query: format!("ABC-{index}"),
                    source_files: Vec::new(),
                })
                .collect(),
            false,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    let (one, two, three) = tokio::join!(
        claim_when_free(&first),
        claim_when_free(&second),
        claim_when_free(&first)
    );
    let mut active = [one, two, three];
    active.sort_unstable_by_key(|item| item.id);
    assert!(second.claim(true, None).await.unwrap().is_none());

    let expired = &active[0];
    sqlx::query(
        "UPDATE collect_metadata_batch_rows SET lease_until=now()-interval '1 second' WHERE id=$1",
    )
    .bind(expired.id)
    .execute(&first.pool)
    .await
    .unwrap();
    let resumed = claim_when_free(&second).await;
    assert_eq!(resumed.id, expired.id);
    assert_eq!(resumed.attempt, expired.attempt + 1);
    assert!(!first.renew(expired).await.unwrap());
    let success = || Outcome {
        remembered_match: None,
        output: None,
        stage: "success",
        message: None,
        candidates: None,
        result: Some(LookupResponse {
            record: json!({"query":"ABC-0","fields":{"dvd_id":"ABC-0","title":"Fixture"}}),
            saved: None,
            storage_error: None,
        }),
    };
    first.finish(expired, success()).await.unwrap();
    second.finish(&resumed, success()).await.unwrap();
    second.finish(&resumed, success()).await.unwrap();
    let result = first
        .batch_row_result(batch, resumed.id)
        .await
        .unwrap()
        .unwrap();
    assert!(result.saved.is_some());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM collect_metadata_snapshots")
        .fetch_one(&first.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        first.batch_view(batch).await.unwrap().unwrap().rows[0].stage,
        "success"
    );
    claim_when_free(&first).await;

    let review = &active[1];
    first
        .finish(
            review,
            Outcome {
                remembered_match: None,
                output: None,
                stage: "choice",
                message: None,
                result: None,
                candidates: Some(json!([{"provider":"wikidata","id":"Q42"}])),
            },
        )
        .await
        .unwrap();
    let third = Store::connect(&options).await.unwrap();
    let rows = third.batch_view(batch).await.unwrap().unwrap().rows;
    let waiting = rows.iter().find(|row| row.id == review.id).unwrap();
    assert_eq!(waiting.stage, "choice");
    assert_eq!(waiting.candidates.as_ref().unwrap()[0]["id"], "Q42");
    assert!(
        !third
            .select_batch_candidate(
                batch,
                review.id,
                CandidateChoice {
                    provider: MediaProvider::Wikidata,
                    id: "Q24".into(),
                }
            )
            .await
            .unwrap()
    );
    assert!(
        third
            .select_batch_candidate(
                batch,
                review.id,
                CandidateChoice {
                    provider: MediaProvider::Wikidata,
                    id: "Q42".into(),
                }
            )
            .await
            .unwrap()
    );
    let selected = claim_when_free(&third).await;
    assert_eq!(selected.id, review.id);
    assert_eq!(selected.candidate.unwrap().id, "Q42");
    third.pool.close().await;
    first.pool.close().await;
    second.pool.close().await;
    sqlx::query(AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&admin)
        .await
        .unwrap();
}
#[tokio::test]
#[ignore = "需要 COLLECT_METADATA_TEST_POSTGRES 指向可创建测试 schema 的 PostgreSQL"]
async fn batch_controls_are_persisted_and_revoke_active_claims() {
    let (admin, options, schema) = test_database().await;
    let store = Store::connect(&options).await.unwrap();
    let id = store
        .create_batch(
            (0..7)
                .map(|index| BatchItemInput {
                    kind: MediaKind::Movie,
                    query: format!("Movie {index}"),
                    source_files: Vec::new(),
                })
                .collect(),
            true,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(store.claim(false, None).await.unwrap().is_none());
    let completed = claim_when_free(&store).await;
    let ready = || Outcome {
        remembered_match: None,
        output: None,
        stage: "success",
        message: None,
        candidates: None,
        result: Some(LookupResponse {
            record: json!({"query":"Movie 0","fields":{"title":"Fixture"}}),
            saved: None,
            storage_error: None,
        }),
    };
    assert!(store.finish(&completed, ready()).await.unwrap());
    for stage in ["failed", "choice", "not_found"] {
        let claim = claim_when_free(&store).await;
        assert!(
            store
                .finish(
                    &claim,
                    Outcome {
                        remembered_match: None,
                        output: None,
                        stage,
                        message: None,
                        result: None,
                        candidates: (stage == "choice")
                            .then(|| json!([{"provider":"wikidata","id":"Q42"}])),
                    }
                )
                .await
                .unwrap()
        );
    }
    let old_claim = claim_when_free(&store).await;
    assert_eq!(
        store.control_batch(id, BatchAction::Pause).await.unwrap(),
        Some(4)
    );

    let restarted = Store::connect(&options).await.unwrap();
    let get = || restarted.batch_view(id);
    let snapshot = get().await.unwrap().unwrap();
    assert!(matches!(snapshot.status, BatchStatus::Paused));
    assert_eq!(
        snapshot
            .rows
            .iter()
            .map(|row| row.stage.as_str())
            .collect::<Vec<_>>(),
        [
            "success",
            "failed",
            "paused",
            "not_found",
            "paused",
            "paused",
            "paused"
        ]
    );
    assert_eq!(
        snapshot.rows[2].candidates.as_ref().unwrap()[0]["id"],
        "Q42"
    );
    assert!(restarted.claim(true, None).await.unwrap().is_none());
    assert!(!store.renew(&old_claim).await.unwrap());
    assert!(!store.finish(&old_claim, ready()).await.unwrap());
    store.release_revoked(&old_claim).await.unwrap();

    assert_eq!(
        restarted
            .control_batch(id, BatchAction::RetryFailed)
            .await
            .unwrap(),
        Some(1)
    );
    assert_eq!(get().await.unwrap().unwrap().rows[1].stage, "paused");
    assert!(restarted.claim(true, None).await.unwrap().is_none());
    assert_eq!(
        restarted
            .control_batch(id, BatchAction::Resume)
            .await
            .unwrap(),
        Some(5)
    );
    assert_eq!(get().await.unwrap().unwrap().rows[2].stage, "choice");
    let retry = claim_when_free(&restarted).await;
    assert_eq!(retry.query, "Movie 1");
    assert_eq!(
        restarted
            .control_batch(id, BatchAction::Cancel)
            .await
            .unwrap(),
        Some(5)
    );

    let after_cancel = Store::connect(&options).await.unwrap();
    let persisted = after_cancel.batch_view(id).await.unwrap().unwrap();
    assert!(matches!(persisted.status, BatchStatus::Cancelled));
    assert_eq!(persisted.rows[0].stage, "success");
    assert!(persisted.rows[0].has_result);
    assert_eq!(persisted.rows[3].stage, "not_found");
    assert!(after_cancel.claim(true, None).await.unwrap().is_none());
    assert!(
        after_cancel
            .control_batch(id, BatchAction::Resume)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        after_cancel
            .control_batch(id, BatchAction::RetryFailed)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!restarted.finish(&retry, ready()).await.unwrap());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM collect_metadata_snapshots")
        .fetch_one(&after_cancel.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    after_cancel.pool.close().await;
    restarted.pool.close().await;
    store.pool.close().await;
    sqlx::query(AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&admin)
        .await
        .unwrap();
}
