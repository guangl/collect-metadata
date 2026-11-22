use serde_json::{Value, json};
use sqlx::{Row, types::Json};

use crate::{
    batch::Claim,
    diagnostics,
    output::{PreparedWriteback, WritebackJob, WritebackState},
};

use super::{Store, batch::lock_queue};

const WRITEBACK_LOCK: i64 = 414290619;

pub(crate) struct WritebackRow {
    pub id: i64,
    pub snapshot: i64,
    pub stage: String,
    pub record: Value,
    pub state: WritebackState,
}

impl Store {
    pub(crate) async fn writeback_rows(
        &self,
        batch: i64,
        row: Option<i64>,
        owner: &str,
    ) -> Result<Vec<WritebackRow>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT r.id, r.snapshot_id, r.stage, s.document, r.writeback_state
             FROM collect_metadata_batch_rows r
             JOIN collect_metadata_batches b ON b.id=r.batch_id
             JOIN collect_metadata_snapshots s ON s.id=r.snapshot_id
             WHERE r.batch_id=$1 AND ($2::bigint IS NULL OR r.id=$2)
               AND r.stage IN ('success','partial') AND r.writeback_job IS NULL
               AND r.writeback_state IS NOT NULL
               AND ($2 IS NOT NULL OR r.writeback_state->>'remaining' IS DISTINCT FROM '0')
               AND coalesce(b.execution_owner, b.export_plan->>'owner', $3)=$3
             ORDER BY r.id",
        )
        .bind(batch)
        .bind(row)
        .bind(owner)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(WritebackRow {
                    id: row.try_get("id")?,
                    snapshot: row.try_get("snapshot_id")?,
                    stage: row.try_get("stage")?,
                    record: row.try_get("document")?,
                    state: row.try_get::<Json<WritebackState>, _>("writeback_state")?.0,
                })
            })
            .collect()
    }

    pub(crate) async fn queue_writebacks(
        &self,
        batch: i64,
        plans: &[(WritebackRow, WritebackJob)],
    ) -> Result<Option<usize>, sqlx::Error> {
        let mut tx = lock_queue(&self.pool).await?;
        for (row, job) in plans {
            let updated = sqlx::query(
                "UPDATE collect_metadata_batch_rows r
                 SET writeback_job=$4, stage=CASE WHEN b.status='paused' THEN 'paused' ELSE 'queued' END,
                     updated_at=now()
                 FROM collect_metadata_batches b
                 WHERE r.batch_id=$1 AND r.id=$2 AND r.snapshot_id=$3 AND r.stage=$5
                   AND r.writeback_state=$6 AND r.writeback_job IS NULL AND b.id=r.batch_id
                   AND b.status IN ('running','paused')
                   AND coalesce(b.execution_owner, b.export_plan->>'owner', $7)=$7",
            ).bind(batch).bind(row.id).bind(row.snapshot).bind(Json(job)).bind(&row.stage)
                .bind(Json(&row.state)).bind(&job.owner).execute(&mut *tx).await?.rows_affected();
            if updated != 1 {
                return Ok(None);
            }
        }
        tx.commit().await?;
        Ok(Some(plans.len()))
    }

    pub(crate) async fn finish_writeback(
        &self,
        claim: &Claim,
        job: &WritebackJob,
        state: &WritebackState,
        output: PreparedWriteback,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Serialize publication across batches that may target the same media directory.
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(WRITEBACK_LOCK)
            .execute(&mut *tx)
            .await?;
        let owned = sqlx::query(
            "SELECT 1 FROM collect_metadata_batch_rows
             WHERE id=$1 AND attempt=$2 AND stage='collecting' AND lease_until>clock_timestamp()
               AND writeback_job->>'id'=$3 FOR UPDATE",
        )
        .bind(claim.id)
        .bind(claim.attempt)
        .bind(&job.id)
        .fetch_optional(&mut *tx)
        .await?;
        if owned.is_none() {
            return Ok(false);
        }
        let id = job.id.clone();
        let state = state.clone();
        let (state, report) = tokio::task::spawn_blocking(move || output.publish(&id, state))
            .await
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        let details = json!({"written": report.written, "failed": report.failed});
        let failed = report.failed > 0;
        sqlx::query(
            "UPDATE collect_metadata_batch_rows
             SET stage=$2, writeback_state=$3, output_report=$4, writeback_job=NULL, lease_until=NULL, updated_at=now()
             WHERE id=$1",
        ).bind(claim.id).bind(&job.stage).bind(Json(state)).bind(Json(report)).execute(&mut *tx).await?;
        tx.commit().await?;
        if failed {
            diagnostics::warn("batch_writeback_finished", details);
        } else {
            diagnostics::info("batch_writeback_finished", details);
        }
        Ok(true)
    }
}
