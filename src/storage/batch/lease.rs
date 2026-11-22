use serde_json::json;
use sqlx::{Row, types::Json};

use crate::{
    CandidateChoice, OutputReport,
    batch::{Claim, Outcome},
    diagnostics,
    output::{ExportPlan, WritebackJob, WritebackState, emby_source},
};

use super::{CLAIM_LOCK, Store, batch_kind};

impl Store {
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
            let writeback = r
                .try_get::<Option<Json<WritebackJob>>, _>("writeback_job")?
                .map(|job| {
                    r.try_get::<Json<WritebackState>, _>("writeback_state")
                        .map(|state| (job.0, state.0))
                })
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
        let output_issue = report.as_ref().is_some_and(OutputReport::has_issue);
        let output_counts = report
            .as_ref()
            .map(|report| json!({"written": report.written, "failed": report.failed}));
        let stage = if outcome.stage == "success" && output_issue {
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
        let details =
            json!({"stage": stage, "output_issue": output_issue, "output": output_counts});
        if stage == "failed" || stage == "partial" {
            diagnostics::warn("batch_row_finished", details);
        } else {
            diagnostics::info("batch_row_finished", details);
        }
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
