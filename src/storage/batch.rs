use serde_json::Value;
use sqlx::{PgConnection, PgPool, Postgres, QueryBuilder, Row, Transaction, types::Json};

use crate::{
    LookupResponse, OutputReport,
    batch::{
        BatchAction, BatchItemInput, BatchRow, BatchStatus, BatchSummary, BatchView,
        CandidateChoice, Claim, MatchMode, Outcome,
    },
    output::{ExportPlan, WritebackJob, WritebackState, emby_source},
};

use super::Store;

const CLAIM_LOCK: i64 = 414290618;

pub(super) async fn migrate(connection: &mut PgConnection) -> Result<(), sqlx::Error> {
    for statement in [
        "CREATE TABLE IF NOT EXISTS collect_metadata_batches (
            id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
            created_at timestamptz NOT NULL DEFAULT now(),
            folder_name text,
            export_plan jsonb,
            execution_owner text,
            status text NOT NULL DEFAULT 'running' CHECK (status IN ('running','paused','cancelled'))
        )",
        "CREATE TABLE IF NOT EXISTS collect_metadata_batch_rows (
            id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
            batch_id bigint NOT NULL REFERENCES collect_metadata_batches(id),
            kind text NOT NULL CHECK (kind IN ('movie','series','av')),
            query text NOT NULL,
            source_files jsonb NOT NULL DEFAULT '[]'::jsonb,
            requires_tmdb boolean NOT NULL DEFAULT false,
            stage text NOT NULL DEFAULT 'queued'
                CONSTRAINT collect_metadata_batch_rows_stage_v2_check
                CHECK (stage IN ('queued','searching','collecting','choice','success','partial','not_found','failed','paused','cancelled')),
            attempt bigint NOT NULL DEFAULT 0,
            lease_until timestamptz,
            updated_at timestamptz NOT NULL DEFAULT now(),
            candidate jsonb,
            candidates jsonb,
            message text,
            snapshot_id bigint REFERENCES collect_metadata_snapshots(id),
            result jsonb,
            output_report jsonb,
            writeback_state jsonb,
            writeback_job jsonb
        )",
        "ALTER TABLE collect_metadata_batches ADD COLUMN IF NOT EXISTS status
            text NOT NULL DEFAULT 'running' CHECK (status IN ('running','paused','cancelled'))",
        "ALTER TABLE collect_metadata_batches ADD COLUMN IF NOT EXISTS folder_name text",
        "ALTER TABLE collect_metadata_batches ADD COLUMN IF NOT EXISTS export_plan jsonb",
        "ALTER TABLE collect_metadata_batches ADD COLUMN IF NOT EXISTS execution_owner text",
        "ALTER TABLE collect_metadata_batch_rows ADD COLUMN IF NOT EXISTS output_report jsonb",
        "ALTER TABLE collect_metadata_batch_rows ADD COLUMN IF NOT EXISTS writeback_state jsonb",
        "ALTER TABLE collect_metadata_batch_rows ADD COLUMN IF NOT EXISTS writeback_job jsonb",
        "ALTER TABLE collect_metadata_batch_rows ADD COLUMN IF NOT EXISTS requires_tmdb
            boolean NOT NULL DEFAULT false",
        "ALTER TABLE collect_metadata_batch_rows ADD COLUMN IF NOT EXISTS source_files
            jsonb NOT NULL DEFAULT '[]'::jsonb",
        "ALTER TABLE collect_metadata_batches ADD COLUMN IF NOT EXISTS match_mode
            text NOT NULL DEFAULT 'existing' CHECK (match_mode IN ('existing','fill_missing','ai'))",
        "ALTER TABLE collect_metadata_batch_rows ADD COLUMN IF NOT EXISTS remember_match
            boolean NOT NULL DEFAULT false",
        "ALTER TABLE collect_metadata_batch_rows ADD COLUMN IF NOT EXISTS export_revision
            bigint NOT NULL DEFAULT 0",
        "CREATE TABLE IF NOT EXISTS collect_metadata_matches (
            kind text NOT NULL,
            query_key text NOT NULL,
            candidate jsonb NOT NULL,
            PRIMARY KEY (kind, query_key)
        )",
        "CREATE INDEX IF NOT EXISTS collect_metadata_batch_rows_batch_idx
            ON collect_metadata_batch_rows(batch_id, id)",
        "CREATE INDEX IF NOT EXISTS collect_metadata_batch_rows_claim_idx
            ON collect_metadata_batch_rows(id) WHERE stage IN ('queued','searching','collecting')",
    ] {
        sqlx::query(statement).execute(&mut *connection).await?;
    }
    let current_check: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_constraint
         WHERE conrelid='collect_metadata_batch_rows'::regclass
           AND conname='collect_metadata_batch_rows_stage_v2_check')",
    )
    .fetch_one(&mut *connection)
    .await?;
    if !current_check {
        sqlx::query(
            "ALTER TABLE collect_metadata_batch_rows
               DROP CONSTRAINT collect_metadata_batch_rows_stage_check,
               ADD CONSTRAINT collect_metadata_batch_rows_stage_v2_check
                 CHECK (stage IN ('queued','searching','collecting','choice','success','partial','not_found','failed','paused','cancelled'))"
        ).execute(&mut *connection).await?;
    }
    Ok(())
}

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
            let (saved, record) = self.load(id).await?;
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

    #[cfg(test)]
    pub(crate) async fn claim(
        &self,
        has_tmdb: bool,
        owner: Option<&str>,
    ) -> Result<Option<Claim>, sqlx::Error> {
        self.claim_with_ai(has_tmdb, owner, false).await
    }

    pub(crate) async fn claim_with_ai(
        &self,
        has_tmdb: bool,
        owner: Option<&str>,
        has_ai: bool,
    ) -> Result<Option<Claim>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // The shared database has three slots even if desktop and web workers are both running.
        let claim_lock: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
            .bind(CLAIM_LOCK)
            .fetch_one(&mut *tx)
            .await?;
        if !claim_lock {
            return Ok(None);
        }
        let row = sqlx::query(
            "WITH next AS (
                SELECT r.id, b.export_plan, b.match_mode FROM collect_metadata_batch_rows r
                JOIN collect_metadata_batches b ON b.id=r.batch_id
                WHERE b.status='running'
                  AND (r.writeback_job IS NOT NULL OR $1 OR NOT r.requires_tmdb)
                  AND (r.writeback_job IS NOT NULL OR $3 OR b.match_mode<>'ai' OR r.candidate IS NOT NULL)
                  AND (coalesce(r.writeback_job->>'owner', b.execution_owner, b.export_plan->>'owner') IS NULL
                       OR coalesce(r.writeback_job->>'owner', b.execution_owner, b.export_plan->>'owner')=$2)
                  AND r.stage IN ('queued','searching','collecting')
                  AND (r.lease_until IS NULL OR r.lease_until <= clock_timestamp())
                  AND (SELECT count(*) FROM collect_metadata_batch_rows
                       WHERE lease_until > clock_timestamp()) < 3
                ORDER BY r.id FOR UPDATE OF r SKIP LOCKED LIMIT 1
             )
             UPDATE collect_metadata_batch_rows r
             SET stage=CASE WHEN r.writeback_job IS NOT NULL OR r.kind='av' OR r.candidate IS NOT NULL THEN 'collecting' ELSE 'searching' END,
                 attempt=r.attempt+1, lease_until=clock_timestamp()+interval '15 seconds', updated_at=now()
             FROM next WHERE r.id=next.id
             RETURNING r.id,r.batch_id,r.attempt,r.kind,r.query,r.candidate,
                split_part(r.source_files->>0, '/', 1) AS folder_root, next.export_plan,
                (next.match_mode='ai') AS use_ai, (next.match_mode<>'existing') AS fill_missing,
                r.remember_match, r.export_revision,
                CASE WHEN next.export_plan IS NULL THEN '[]'::jsonb ELSE r.source_files END AS source_files,
                r.writeback_job, CASE WHEN r.writeback_job IS NOT NULL THEN r.writeback_state END AS writeback_state"
        ).bind(has_tmdb).bind(owner).bind(has_ai).fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        row.map(|r| {
            let writeback = r.try_get::<Option<Json<WritebackJob>>, _>("writeback_job")?
                .map(|job| r.try_get::<Json<WritebackState>, _>("writeback_state").map(|state| (job.0, state.0)))
                .transpose()?;
            Ok(Claim {
                id: r.get("id"),
                batch: r.get("batch_id"),
                attempt: r.get("attempt"),
                kind: batch_kind(r.get("kind"))?,
                query: r.get("query"),
                folder_root: r.get("folder_root"),
                remember_match: r.get("remember_match"),
                use_ai: r.get("use_ai"),
                fill_missing: r.get("fill_missing"),
                export_revision: r.get("export_revision"),
                export_plan: r
                    .try_get::<Option<Json<ExportPlan>>, _>("export_plan")?
                    .map(|plan| plan.0),
                source_files: r.try_get::<Json<Vec<String>>, _>("source_files")?.0,
                writeback,
                candidate: r
                    .try_get::<Option<Json<CandidateChoice>>, _>("candidate")?
                    .map(|value| value.0),
            })
        })
        .transpose()
    }

    pub(crate) async fn renew(&self, claim: &Claim) -> Result<bool, sqlx::Error> {
        Ok(sqlx::query(
            "UPDATE collect_metadata_batch_rows SET lease_until=clock_timestamp()+interval '15 seconds'
             WHERE id=$1 AND attempt=$2 AND stage IN ('searching','collecting') AND lease_until>clock_timestamp()"
        ).bind(claim.id).bind(claim.attempt).execute(&self.pool).await?.rows_affected() == 1)
    }

    pub(crate) async fn collecting(
        &self,
        claim: &Claim,
        candidate: &CandidateChoice,
    ) -> Result<bool, sqlx::Error> {
        Ok(sqlx::query(
            "UPDATE collect_metadata_batch_rows SET stage='collecting', candidate=$3, updated_at=now()
             WHERE id=$1 AND attempt=$2 AND stage IN ('searching','collecting') AND lease_until>clock_timestamp()"
        ).bind(claim.id).bind(claim.attempt).bind(Json(candidate)).execute(&self.pool).await?.rows_affected() == 1)
    }

    pub(crate) async fn finish(
        &self,
        claim: &Claim,
        outcome: Outcome,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let owned = sqlx::query(
            "SELECT 1 FROM collect_metadata_batch_rows
             WHERE id=$1 AND attempt=$2 AND stage IN ('searching','collecting') AND lease_until>clock_timestamp() FOR UPDATE",
        )
        .bind(claim.id)
        .bind(claim.attempt)
        .fetch_optional(&mut *tx)
        .await?;
        if owned.is_none() {
            return Ok(false);
        }
        let mut snapshot = None;
        let mut failure = None;
        let mut writeback = None;
        if let Some(result) = outcome.result {
            if result.has_fields() {
                if emby_source(&result.record).is_some() {
                    writeback = Some(Json(WritebackState::default()));
                }
                let saved = Self::insert_snapshot(&mut tx, result.code(), &result.record).await?;
                snapshot = Some(saved.id);
            } else {
                failure = Some(result.record);
            }
        }
        if let Some((key, candidate)) = outcome.remembered_match {
            sqlx::query(
                "INSERT INTO collect_metadata_matches (kind, query_key, candidate) VALUES ($1,$2,$3)
                 ON CONFLICT (kind, query_key) DO UPDATE SET candidate=EXCLUDED.candidate",
            )
            .bind(claim.kind.code())
            .bind(key)
            .bind(Json(candidate))
            .execute(&mut *tx).await?;
        }
        let report = outcome.output.map(|output| match output {
            Ok(prepared) => prepared.publish(),
            Err(report) => report,
        });
        let stage =
            if outcome.stage == "success" && report.as_ref().is_some_and(OutputReport::has_issue) {
                "partial"
            } else {
                outcome.stage
            };
        sqlx::query(
            "UPDATE collect_metadata_batch_rows SET stage=$2, message=$3, candidates=$4,
                 snapshot_id=coalesce($5, snapshot_id), result=$6, output_report=$7,
                 writeback_state=$8, lease_until=NULL, remember_match=remember_match AND $5 IS NULL,
                 updated_at=now() WHERE id=$1",
        )
        .bind(claim.id)
        .bind(stage)
        .bind(outcome.message)
        .bind(outcome.candidates)
        .bind(snapshot)
        .bind(failure)
        .bind(report.map(Json))
        .bind(writeback)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub(crate) async fn release_revoked(&self, claim: &Claim) -> Result<(), sqlx::Error> {
        // Keep the slot until the old worker acknowledges the stop or its lease expires.
        sqlx::query(
            "UPDATE collect_metadata_batch_rows SET lease_until=NULL
             WHERE id=$1 AND attempt=$2+1 AND stage IN ('paused','queued','cancelled','success','partial')",
        )
        .bind(claim.id)
        .bind(claim.attempt)
        .execute(&self.pool)
        .await?;
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
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
        sqlx::query("UPDATE collect_metadata_batch_rows SET lease_until=now()-interval '1 second' WHERE id=$1")
            .bind(expired.id).execute(&first.pool).await.unwrap();
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
}
