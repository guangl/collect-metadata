mod lease;
mod schema;

#[cfg(test)]
mod tests;

pub(super) use schema::migrate;

use serde_json::Value;
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction, types::Json};

use crate::{
    LookupResponse, OutputReport,
    batch::{
        BatchAction, BatchItemInput, BatchRow, BatchStatus, BatchSummary, BatchView,
        CandidateChoice, MatchMode,
    },
    output::ExportPlan,
};

use super::Store;

const CLAIM_LOCK: i64 = 414290618;

impl Store {
    pub(crate) async fn create_batch(
        &self,
        items: Vec<BatchItemInput>,
        use_tmdb: bool,
        folder_name: Option<&str>,
        export: Option<&ExportPlan>,
        owner: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        self.create_batch_mode(
            items,
            use_tmdb,
            folder_name,
            export,
            owner,
            MatchMode::Existing,
        )
        .await
    }

    pub(crate) async fn create_batch_mode(
        &self,
        items: Vec<BatchItemInput>,
        use_tmdb: bool,
        folder_name: Option<&str>,
        export: Option<&ExportPlan>,
        owner: Option<&str>,
        mode: MatchMode,
    ) -> Result<i64, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let id = sqlx::query_scalar(
            "INSERT INTO collect_metadata_batches (folder_name, export_plan, execution_owner, match_mode)
             VALUES ($1,$2,$3,$4) RETURNING id",
        )
        .bind(folder_name)
        .bind(export.map(Json))
        .bind(owner)
        .bind(mode.code())
        .fetch_one(&mut *tx)
        .await?;
        for chunk in items.chunks(500) {
            let mut insert = QueryBuilder::<Postgres>::new(
                "INSERT INTO collect_metadata_batch_rows (batch_id, kind, query, requires_tmdb, source_files) ",
            );
            insert.push_values(chunk, |mut row, item| {
                let explicit_tmdb = matches!(mode, MatchMode::Ai)
                    && crate::providers::media::tmdb_id_query(&item.query).is_some();
                row.push_bind(id)
                    .push_bind(item.kind.code())
                    .push_bind(item.query.trim())
                    .push_bind((use_tmdb || explicit_tmdb) && item.kind != crate::MediaKind::Av)
                    .push_bind(Json(&item.source_files));
            });
            insert.build().persistent(false).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(id)
    }

    pub(crate) async fn list_batches(
        &self,
        before: Option<i64>,
    ) -> Result<Vec<BatchSummary>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT b.id, b.status, b.folder_name, to_char(b.created_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_at,
                count(r.id) AS total,
                count(*) FILTER (WHERE r.stage IN ('success','partial','failed','not_found','cancelled')) AS finished,
                count(*) FILTER (WHERE r.stage = 'choice') AS waiting
             FROM (SELECT id, created_at, status, folder_name FROM collect_metadata_batches WHERE id < $1 ORDER BY id DESC LIMIT 50) AS b
             JOIN collect_metadata_batch_rows r ON r.batch_id=b.id
             GROUP BY b.id,b.created_at,b.status,b.folder_name ORDER BY b.id DESC"
        ).bind(before.unwrap_or(i64::MAX)).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|r| {
                Ok(BatchSummary {
                    id: r.get("id"),
                    created_at: r.get("created_at"),
                    folder_name: r.get("folder_name"),
                    status: batch_status(r.get("status"))?,
                    total: r.get("total"),
                    finished: r.get("finished"),
                    waiting: r.get("waiting"),
                })
            })
            .collect()
    }

    pub(crate) async fn batch_view(&self, batch: i64) -> Result<Option<BatchView>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT r.id,r.kind,r.query,r.stage,r.message,r.candidates,b.status,
                jsonb_array_length(r.source_files) AS file_count, r.output_report,
                CASE WHEN r.writeback_job IS NOT NULL THEN r.stage
                     WHEN r.writeback_state->>'remaining' = '0' THEN 'done'
                     WHEN r.writeback_state->>'remaining' IS NOT NULL THEN 'failed'
                     WHEN r.writeback_state IS NOT NULL THEN 'ready'
                END AS writeback,
                (r.snapshot_id IS NOT NULL OR r.result IS NOT NULL) AS has_result,
                s.title, coalesce(s.document->'fields'->'original_languages','[]'::jsonb) AS original_languages,
                coalesce(nullif(s.document->'fields'->>'poster_url',''), nullif(s.document->'fields'->>'cover_url','')) AS cover_url
             FROM collect_metadata_batch_rows r
             JOIN collect_metadata_batches b ON b.id=r.batch_id
             LEFT JOIN collect_metadata_snapshots s ON s.id=r.snapshot_id
             WHERE r.batch_id=$1 ORDER BY r.id"
        ).bind(batch).fetch_all(&self.pool).await?;
        let Some(first) = rows.first() else {
            return Ok(None);
        };
        let status = batch_status(first.get("status"))?;
        let rows = rows
            .into_iter()
            .map(|r| {
                Ok(BatchRow {
                    id: r.get("id"),
                    kind: batch_kind(r.get("kind"))?,
                    query: r.get("query"),
                    stage: r.get("stage"),
                    message: r.get("message"),
                    candidates: r.get("candidates"),
                    has_result: r.get("has_result"),
                    title: r.get("title"),
                    cover_url: r.get("cover_url"),
                    original_languages: r.get::<Json<Vec<String>>, _>("original_languages").0,
                    file_count: r.get("file_count"),
                    writeback: r.get("writeback"),
                    output: r
                        .try_get::<Option<Json<OutputReport>>, _>("output_report")?
                        .map(|report| report.0),
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        Ok(Some(BatchView { status, rows }))
    }

    pub(crate) async fn batch_files(
        &self,
        batch: i64,
        row: i64,
    ) -> Result<Option<Vec<String>>, sqlx::Error> {
        let files = sqlx::query_scalar::<_, Json<Vec<String>>>(
            "SELECT source_files FROM collect_metadata_batch_rows WHERE batch_id=$1 AND id=$2",
        )
        .bind(batch)
        .bind(row)
        .fetch_optional(&self.pool)
        .await?;
        Ok(files.map(|files| files.0))
    }

    pub(crate) async fn control_batch(
        &self,
        batch: i64,
        action: BatchAction,
    ) -> Result<Option<u64>, sqlx::Error> {
        let mut tx = lock_queue(&self.pool).await?;
        let (state_change, row_change) = match action {
            BatchAction::Pause => (
                Some("UPDATE collect_metadata_batches SET status='paused' WHERE id=$1 AND status='running'
                      AND EXISTS (SELECT 1 FROM collect_metadata_batch_rows WHERE batch_id=$1
                                  AND stage IN ('queued','searching','collecting','choice'))"),
                "UPDATE collect_metadata_batch_rows SET stage='paused', updated_at=now(),
                     attempt=attempt+(stage IN ('searching','collecting'))::integer
                 WHERE batch_id=$1 AND stage IN ('queued','searching','collecting','choice')",
            ),
            BatchAction::Resume => (
                Some("UPDATE collect_metadata_batches SET status='running' WHERE id=$1 AND status='paused'"),
                "UPDATE collect_metadata_batch_rows SET
                    stage=CASE WHEN candidates IS NULL THEN 'queued' ELSE 'choice' END, updated_at=now()
                 WHERE batch_id=$1 AND stage='paused'",
            ),
            BatchAction::Cancel => (
                Some("UPDATE collect_metadata_batches SET status='cancelled' WHERE id=$1 AND status IN ('running','paused')
                      AND EXISTS (SELECT 1 FROM collect_metadata_batch_rows WHERE batch_id=$1
                                  AND stage IN ('queued','searching','collecting','choice','paused'))"),
                "UPDATE collect_metadata_batch_rows SET stage=coalesce(writeback_job->>'stage','cancelled'),
                     writeback_job=NULL, updated_at=now(),
                     attempt=attempt+(stage IN ('searching','collecting'))::integer
                 WHERE batch_id=$1 AND stage IN ('queued','searching','collecting','choice','paused')",
            ),
            BatchAction::RetryFailed => (
                None,
                "UPDATE collect_metadata_batch_rows r SET
                    stage=CASE WHEN b.status='paused' THEN 'paused' ELSE 'queued' END,
                    updated_at=now(), message=NULL, candidates=NULL, result=NULL
                 FROM collect_metadata_batches b
                 WHERE r.batch_id=$1 AND b.id=r.batch_id AND b.status IN ('running','paused')
                   AND r.stage='failed'",
            ),
        };
        if let Some(statement) = state_change
            && sqlx::query(statement)
                .bind(batch)
                .execute(&mut *tx)
                .await?
                .rows_affected()
                == 0
        {
            return Ok(None);
        }
        let affected = sqlx::query(row_change)
            .bind(batch)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if affected == 0 {
            return Ok(None);
        }
        tx.commit().await?;
        Ok(Some(affected))
    }

    pub(crate) async fn retry_batch_row(&self, batch: i64, row: i64) -> Result<bool, sqlx::Error> {
        let mut tx = lock_queue(&self.pool).await?;
        let affected = sqlx::query(
            "UPDATE collect_metadata_batch_rows r SET
                 stage=CASE WHEN b.status='paused' THEN 'paused' ELSE 'queued' END,
                 updated_at=now(), message=NULL, candidates=NULL, result=NULL
             FROM collect_metadata_batches b
             WHERE r.batch_id=$1 AND r.id=$2 AND b.id=r.batch_id AND b.status IN ('running','paused')
               AND r.stage IN ('failed','not_found') AND r.query<>''",
        ).bind(batch).bind(row).execute(&mut *tx).await?.rows_affected();
        tx.commit().await?;
        Ok(affected == 1)
    }

    pub(crate) async fn select_batch_candidate(
        &self,
        batch: i64,
        row: i64,
        candidate: CandidateChoice,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = lock_queue(&self.pool).await?;
        let provider = candidate.provider.code();
        let affected = sqlx::query(
            "UPDATE collect_metadata_batch_rows r SET stage='queued', updated_at=now(), candidate=$3,
                 candidates=NULL, message=NULL, requires_tmdb=($4='tmdb'), remember_match=true
             FROM collect_metadata_batches b
             WHERE r.batch_id=$1 AND r.id=$2 AND r.stage='choice' AND b.id=r.batch_id AND b.status='running'
               AND EXISTS (SELECT 1 FROM jsonb_array_elements(r.candidates) AS c
                           WHERE c->>'provider'=$4 AND c->>'id'=$5)",
        ).bind(batch).bind(row).bind(Json(&candidate)).bind(provider).bind(&candidate.id)
          .execute(&mut *tx).await?.rows_affected();
        tx.commit().await?;
        Ok(affected == 1)
    }

    pub(crate) async fn correct_batch_match(
        &self,
        batch: i64,
        row: i64,
        candidate: CandidateChoice,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = lock_queue(&self.pool).await?;
        let changed = sqlx::query(
            "UPDATE collect_metadata_batch_rows r SET
                stage=CASE WHEN b.status='paused' THEN 'paused' ELSE 'queued' END,
                candidate=$3, candidates=NULL, message=NULL, output_report=NULL,
                requires_tmdb=($4='tmdb'), remember_match=true, updated_at=now(),
                export_revision=r.export_revision+1
             FROM collect_metadata_batches b
             WHERE r.batch_id=$1 AND r.id=$2 AND b.id=r.batch_id AND b.status IN ('running','paused')
               AND r.stage IN ('choice','failed','not_found','success','partial','paused')
               AND r.writeback_job IS NULL
               AND r.kind IN ('movie','series') AND ($4<>'tvmaze' OR r.kind='series')
               AND (r.query NOT LIKE 'emby-episode:%' OR $4='tmdb')",
        )
        .bind(batch)
        .bind(row)
        .bind(Json(&candidate))
        .bind(candidate.provider.code())
        .execute(&mut *tx).await?.rows_affected() == 1;
        tx.commit().await?;
        Ok(changed)
    }

    pub(crate) async fn remembered_match(
        &self,
        kind: crate::MediaKind,
        key: &str,
    ) -> Result<Option<CandidateChoice>, sqlx::Error> {
        sqlx::query_scalar::<_, Json<CandidateChoice>>(
            "SELECT candidate FROM collect_metadata_matches WHERE kind=$1 AND query_key=$2",
        )
        .bind(kind.code())
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map(|value| value.map(|candidate| candidate.0))
    }

    pub(crate) async fn batch_row_result(
        &self,
        batch: i64,
        row: i64,
    ) -> Result<Option<LookupResponse>, String> {
        let item = sqlx::query("SELECT snapshot_id, result FROM collect_metadata_batch_rows WHERE batch_id=$1 AND id=$2")
            .bind(batch).bind(row).fetch_optional(&self.pool).await.map_err(|_| "无法读取批量采集结果。")?;
        let Some(item) = item else {
            return Ok(None);
        };
        if let Some(id) = item.get::<Option<i64>, _>("snapshot_id") {
            let (saved, record) = self.load(id, true).await?;
            return Ok(Some(LookupResponse {
                record,
                saved: Some(saved),
                storage_error: None,
            }));
        }
        Ok(item
            .get::<Option<Value>, _>("result")
            .map(|record| LookupResponse {
                record,
                saved: None,
                storage_error: None,
            }))
    }
}

pub(super) async fn lock_queue(pool: &PgPool) -> Result<Transaction<'_, Postgres>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(CLAIM_LOCK)
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}

fn batch_status(code: &str) -> Result<BatchStatus, sqlx::Error> {
    match code {
        "running" => Ok(BatchStatus::Running),
        "paused" => Ok(BatchStatus::Paused),
        "cancelled" => Ok(BatchStatus::Cancelled),
        _ => Err(sqlx::Error::Protocol("invalid batch status".into())),
    }
}

fn batch_kind(code: &str) -> Result<crate::MediaKind, sqlx::Error> {
    match code {
        "av" => Ok(crate::MediaKind::Av),
        "movie" => Ok(crate::MediaKind::Movie),
        "series" => Ok(crate::MediaKind::Series),
        _ => Err(sqlx::Error::Protocol("invalid batch media kind".into())),
    }
}
