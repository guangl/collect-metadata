use std::time::{Duration, SystemTime};

use futures_util::{StreamExt, stream};
use sha2::{Digest, Sha256};

use super::{AutoScrape, Event, MISSING_ITEM_GRACE, Outcome};
use crate::{
    Collector, diagnostics,
    storage::{
        Store,
        auto_scrape::{AutoEvent, AutoJob},
    },
};

fn database_error(error: sqlx::Error) -> String {
    diagnostics::postgres_error("postgres_auto_scrape_failed", &error);
    "自动刮削任务状态保存失败，保留事件等待数据库恢复。".into()
}

async fn generation(path: &std::path::Path) -> std::io::Result<String> {
    let metadata = tokio::fs::metadata(path).await?;
    let modified = metadata
        .modified()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    Ok(format!("{}:{}", modified.as_nanos(), metadata.len()))
}

fn emby_unavailable(error: &str) -> bool {
    error.contains("无法连接 Emby")
        || error.contains("Emby 请求超时")
        || error.contains("Emby 请求失败（HTTP 5")
        || error.contains("Emby 尚未")
}

fn review_error(error: &str) -> Option<&'static str> {
    if error.contains("Emby 没有保存全部可写的补充字段") || error.contains("Emby 没有确认补全写入")
    {
        Some("writeback_incomplete")
    } else if error.contains("映射")
        || error.contains("未猜测匹配")
        || error.contains("需要核对")
        || error.contains("没有唯一的同名同年份")
        || error.contains("无法安全匹配")
    {
        Some("mapping_review")
    } else if error.contains("HTTP 401") || error.contains("HTTP 403") || error.contains("已锁定")
    {
        Some("configuration_review")
    } else {
        None
    }
}

fn retry_delay(job: &AutoJob) -> i64 {
    let base = (30_i64.saturating_mul(2_i64.pow((job.failures as u32).min(6)))).min(1800);
    // Stable jitter prevents hundreds of tasks returning together after an outage.
    let digest = Sha256::digest(format!("{}:{}", job.id, job.failures));
    base + i64::from(digest[0]) % ((base / 5).max(1))
}

impl AutoScrape {
    fn queue_key(&self) -> String {
        Sha256::digest(format!(
            "{}\n{}",
            self.spool.display(),
            self.url.as_deref().unwrap_or_default()
        ))
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
    }

    pub(super) async fn run_durable(self, collector: std::sync::Arc<Collector>) {
        eprintln!("Collect Metadata automatic scrape using PostgreSQL durable queue");
        loop {
            if let Err(error) = self.durable_cycle(&collector).await {
                eprintln!("Automatic scrape queue deferred: {error}");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    async fn durable_cycle(&self, collector: &Collector) -> Result<(), String> {
        let store = collector.require_store().await?;
        let queue = self.queue_key();
        let events = self
            .events()
            .await
            .map_err(|_| "无法读取自动刮削事件目录。")?;
        let mut inputs = Vec::with_capacity(events.len());
        for event in events {
            match generation(&event.path).await {
                Ok(generation) => inputs.push(AutoEvent {
                    name: event
                        .path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    generation,
                    kind: event.item_type.into(),
                    item_id: event.id,
                    backfill: event.backfill,
                }),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err("无法读取自动刮削事件信息。".into()),
            }
        }
        let terminal = store
            .sync_auto_events(&queue, &inputs)
            .await
            .map_err(database_error)?;
        for job in terminal {
            self.settle_file(&job).await?;
        }
        if !inputs.is_empty() {
            match store
                .auto_dependency_due(&queue)
                .await
                .map_err(database_error)?
            {
                Some(false) => return Ok(()),
                Some(true) => {}
                None => {
                    let connection = collector
                        .emby_settings()
                        .ok_or("Emby 设置不可用。")?
                        .automatic_connection(self.url.as_deref())?;
                    let result = connection.automatic_ready(&collector.client).await;
                    store
                        .set_auto_dependency(&queue, result.as_ref().err().map(String::as_str))
                        .await
                        .map_err(database_error)?;
                    if result.is_err() {
                        return Ok(());
                    }
                }
            }
        }
        let claims = store
            .claim_auto_jobs(&queue)
            .await
            .map_err(database_error)?;
        let queue_ref = queue.as_str();
        let mut running = stream::iter(claims)
            .map(|job| async move { self.run_job(collector, store, queue_ref, job).await })
            .buffer_unordered(4);
        while let Some(result) = running.next().await {
            result?;
        }
        Ok(())
    }

    async fn run_job(
        &self,
        collector: &Collector,
        store: &Store,
        queue: &str,
        mut job: AutoJob,
    ) -> Result<(), String> {
        let path = self.spool.join(&job.name);
        let current = match generation(&path).await {
            Ok(value) => Some(value),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("无法读取入库事件，保留任务等待文件访问恢复。".into()),
        };
        if current.as_deref() != Some(job.generation.as_str()) {
            store
                .finish_auto_job(
                    &job,
                    "missing",
                    Some("event_replaced"),
                    Some("入库事件已撤回或更新。"),
                    0,
                    false,
                )
                .await
                .map_err(database_error)?;
            return Ok(());
        }
        let event = Event::parse(path).ok_or("自动刮削事件名无效。")?;
        let mut processing = std::pin::pin!(self.process(collector, &event));
        let result = loop {
            tokio::select! {
                result=&mut processing=>break result,
                _=tokio::time::sleep(Duration::from_secs(20))=>{
                    if !store.renew_auto_job(&job).await.map_err(database_error)? {
                        return Err("自动刮削租约已转移，当前执行停止。".into());
                    }
                }
            }
        };
        let mut count = 0;
        let (status, kind, reason, delay, failed) = match result {
            Ok(Outcome::Written(written)) => {
                count = written;
                ("completed", None, None, 0, false)
            }
            Ok(Outcome::Enriched) => ("completed", None, None, 0, false),
            Ok(Outcome::Skipped) => (
                "skipped",
                Some("out_of_scope"),
                Some("条目已锁定或不在配置的媒体库内。".to_owned()),
                0,
                false,
            ),
            Ok(Outcome::AwaitingTitle) => (
                "waiting_source",
                Some("awaiting_title"),
                Some("来源尚未提供可核对的分集标题。".into()),
                if job.attempts <= 1 { 1800 } else { 21600 },
                false,
            ),
            Ok(Outcome::AwaitingAirdateReview) => (
                "review",
                Some("airdate_review"),
                Some("放送日期需要核对。".into()),
                0,
                false,
            ),
            Ok(Outcome::NoVerifiedSource) => (
                "review",
                Some("unverified_source"),
                Some("没有可核对的来源。".into()),
                0,
                false,
            ),
            Ok(Outcome::MissingItem) if job.age_seconds >= MISSING_ITEM_GRACE.as_secs() as i64 => (
                "missing",
                Some("missing_item"),
                Some("Emby 在入库宽限期后仍未返回此条目。".into()),
                0,
                false,
            ),
            Ok(Outcome::MissingItem) => (
                "retry",
                Some("missing_item"),
                Some("等待 Emby 完成入库。".into()),
                30,
                false,
            ),
            Err(error) => {
                if emby_unavailable(&error) {
                    store
                        .set_auto_dependency(queue, Some(&error))
                        .await
                        .map_err(database_error)?;
                }
                if error.contains("来源尚未提供可核对的分集标题") {
                    (
                        "waiting_source",
                        Some("awaiting_title"),
                        Some(error),
                        if job.attempts <= 1 { 1800 } else { 21600 },
                        false,
                    )
                } else if let Some(kind) = review_error(&error) {
                    ("review", Some(kind), Some(error), 0, true)
                } else if job.expired {
                    (
                        "failed",
                        Some("retry_window_exhausted"),
                        Some(error),
                        0,
                        true,
                    )
                } else {
                    let delay = retry_delay(&job);
                    (
                        "retry",
                        Some(if emby_unavailable(&error) {
                            "emby_unavailable"
                        } else {
                            "transient_error"
                        }),
                        Some(error),
                        delay,
                        true,
                    )
                }
            }
        };
        if !store
            .finish_auto_job(&job, status, kind, reason.as_deref(), delay, failed)
            .await
            .map_err(database_error)?
        {
            return Err("自动刮削结果未提交：租约已转移。".into());
        }
        // Log only committed states, including zero-sidecar metadata enrichment.
        // Existing notification consumers understand this single success format.
        if status == "completed" {
            eprintln!(
                "Automatic scrape {} {} completed with {count} new sidecar files.",
                job.kind, job.item_id
            );
        } else if matches!(status, "review" | "failed" | "missing") {
            let archive = if status == "review" {
                "review"
            } else if status == "missing" {
                "missing"
            } else {
                "failed"
            };
            eprintln!(
                "Automatic scrape {} {} archived as {archive}: {}",
                job.kind,
                job.item_id,
                reason.as_deref().unwrap_or_default()
            );
        } else if status == "waiting_source" {
            eprintln!(
                "Automatic scrape {} {}: {}; retry in {delay}s.",
                job.kind,
                job.item_id,
                reason.as_deref().unwrap_or_default()
            );
        } else if status == "retry" {
            eprintln!(
                "Automatic scrape retry scheduled for {} {}: {}; retry in {delay}s.",
                job.kind,
                job.item_id,
                reason.as_deref().unwrap_or_default()
            );
        }
        job.status = status.into();
        if matches!(
            status,
            "completed" | "skipped" | "review" | "failed" | "missing"
        ) {
            self.settle_file(&job).await?;
        }
        Ok(())
    }

    async fn settle_file(&self, job: &AutoJob) -> Result<(), String> {
        let path = self.spool.join(&job.name);
        match generation(&path).await {
            Ok(current) if current != job.generation => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err("任务结果已保存，事件文件暂时无法读取。".into()),
            _ => {}
        }
        let settled = if job.backfill {
            Some(path.with_extension(format!(
                "backfill-{}",
                match job.status.as_str() {
                    "completed" => "done",
                    "missing" => "skipped",
                    other => other,
                }
            )))
        } else if matches!(job.status.as_str(), "completed" | "skipped") {
            None
        } else {
            Some(path.with_extension(format!("{}-{}", job.status, job.id)))
        };
        let result = match settled {
            Some(target) => tokio::fs::rename(&path, target).await,
            None => tokio::fs::remove_file(&path).await,
        };
        match result {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("任务结果已保存在 PostgreSQL，事件文件稍后重试归档。".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_are_classified_without_guessing_season_mappings() {
        assert!(emby_unavailable("Emby 请求失败（HTTP 503）。"));
        assert!(!emby_unavailable("TMDB HTTP 503"));
        assert_eq!(
            review_error("主来源：分季映射冲突；备用来源：未猜测匹配。"),
            Some("mapping_review")
        );
        assert_eq!(
            review_error("Emby 没有保存全部可写的补充字段。"),
            Some("writeback_incomplete")
        );
        assert_eq!(review_error("Emby 请求失败（HTTP 503）。"), None);
    }

    #[tokio::test]
    async fn terminal_cleanup_is_idempotent_and_preserves_a_new_event_generation() {
        let temp = tempfile::tempdir().unwrap();
        let service = AutoScrape {
            spool: temp.path().into(),
            url: None,
            roots: vec![],
            private_roots: vec![],
            central_metadata: false,
            season_map: super::super::SeasonMap::default(),
            database_season_map: false,
            emby_timezone: jiff::tz::TimeZone::UTC,
        };
        let path = temp.path().join("Episode-1908.backfill");
        std::fs::write(&path, b"first").unwrap();
        let mut job = AutoJob {
            id: 1,
            name: "Episode-1908.backfill".into(),
            generation: generation(&path).await.unwrap(),
            kind: "Episode".into(),
            item_id: "1908".into(),
            backfill: true,
            attempts: 1,
            failures: 0,
            lease: "fixture".into(),
            status: "completed".into(),
            expired: false,
            age_seconds: 0,
        };
        std::fs::write(&path, b"a newer event").unwrap();
        service.settle_file(&job).await.unwrap();
        assert!(
            path.exists(),
            "old committed results must not remove a replacement event"
        );
        job.generation = generation(&path).await.unwrap();
        service.settle_file(&job).await.unwrap();
        assert!(!path.exists());
        assert!(path.with_extension("backfill-done").exists());
        service.settle_file(&job).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "需要隔离的 COLLECT_METADATA_TEST_POSTGRES"]
    async fn auto_startup_gate_survives_collector_restart_without_consuming_attempts() {
        use axum::{
            Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get,
        };
        use serde_json::json;
        use sqlx::{AssertSqlSafe, PgPool};
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let database = std::env::var("COLLECT_METADATA_TEST_POSTGRES").unwrap();
        let admin = PgPool::connect(&database).await.unwrap();
        let schema = format!("auto_startup_test_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
            .execute(&admin)
            .await
            .unwrap();
        let ready = Arc::new(AtomicBool::new(false));
        async fn mock(
            State(ready): State<Arc<AtomicBool>>,
            uri: axum::http::Uri,
        ) -> axum::response::Response {
            if !ready.load(Ordering::SeqCst) {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            if uri.path().ends_with("System/Info/Public") {
                // Emby 4.10 PublicInfo has no StartupWizardCompleted field.
                return Json(json!({"Id":"abc123","Version":"4.10.2.0"})).into_response();
            }
            Json(json!({"Items":[{"Id":"91","Type":"Movie","Path":"/media/fixture.mkv","LocationType":"Virtual"}]})).into_response()
        }
        let app = Router::new()
            .fallback(get(mock))
            .with_state(Arc::clone(&ready));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_url = format!("http://{}/emby", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut url = reqwest::Url::parse(&database).unwrap();
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("emby.json");
        std::fs::write(
            &config,
            json!({"enabled":true,"server":server_url,"api_key":"fixture"}).to_string(),
        )
        .unwrap();
        let make_collector = || {
            Collector::from_database_url(url.as_str())
                .unwrap()
                .with_emby_settings(Arc::new(crate::EmbySettings::load(config.clone()).unwrap()))
        };
        let service = AutoScrape {
            spool: temp.path().to_path_buf(),
            url: Some(server_url),
            roots: vec!["/media".into()],
            private_roots: vec![],
            central_metadata: false,
            season_map: super::super::SeasonMap::default(),
            database_season_map: false,
            emby_timezone: jiff::tz::TimeZone::UTC,
        };
        let event = temp.path().join("Movie-91.event");
        std::fs::write(&event, []).unwrap();
        let collector = make_collector();
        service.durable_cycle(&collector).await.unwrap();
        let state = collector.auto_scrape_state().await.unwrap();
        assert_eq!(state["jobs"][0]["status"], "queued");
        assert_eq!(state["jobs"][0]["attempts"], 0);
        assert_eq!(state["dependencies"][0]["ready"], false);
        assert_eq!(state["dependencies"][0]["failures"], 1);
        drop(collector);
        ready.store(true, Ordering::SeqCst);
        let collector = make_collector();
        service.durable_cycle(&collector).await.unwrap();
        assert_eq!(
            collector.auto_scrape_state().await.unwrap()["jobs"][0]["attempts"],
            0,
            "restart must retain readiness cooldown"
        );
        sqlx::query(AssertSqlSafe(format!("UPDATE {schema}.collect_metadata_auto_dependencies SET next_check_at=now()-interval '1 second'")))
            .execute(&admin).await.unwrap();
        service.durable_cycle(&collector).await.unwrap();
        let state = collector.auto_scrape_state().await.unwrap();
        assert_eq!(state["jobs"][0]["status"], "skipped");
        assert_eq!(state["jobs"][0]["attempts"], 1);
        assert!(!event.exists());
        service.durable_cycle(&collector).await.unwrap();
        assert_eq!(
            collector.auto_scrape_state().await.unwrap()["jobs"][0]["attempts"],
            1
        );
        drop(collector);
        server.abort();
        sqlx::query(AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
            .execute(&admin)
            .await
            .unwrap();
    }
}
