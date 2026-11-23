use serde_json::{Value, json};
use sqlx::{PgConnection, Row};

use super::Store;

pub(crate) struct AutoEvent {
    pub name: String,
    pub generation: String,
    pub kind: String,
    pub item_id: String,
    pub backfill: bool,
}

#[derive(Debug)]
pub(crate) struct AutoJob {
    pub id: i64,
    pub name: String,
    pub generation: String,
    pub kind: String,
    pub item_id: String,
    pub backfill: bool,
    pub attempts: i32,
    pub failures: i32,
    pub lease: String,
    pub status: String,
    pub expired: bool,
    pub age_seconds: i64,
}

pub(super) async fn migrate(connection: &mut PgConnection) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(include_str!("auto_scrape/schema.sql"))
        .execute(connection)
        .await?;
    Ok(())
}

fn job(row: sqlx::postgres::PgRow) -> AutoJob {
    AutoJob {
        id: row.get("id"),
        name: row.get("event_name"),
        generation: row.get("generation"),
        kind: row.get("item_type"),
        item_id: row.get("item_id"),
        backfill: row.get("backfill"),
        attempts: row.get("attempts"),
        failures: row.get("failures"),
        lease: row
            .get::<Option<String>, _>("lease_token")
            .unwrap_or_default(),
        status: row.get("status"),
        expired: row.get("expired"),
        age_seconds: row.get("age_seconds"),
    }
}

const JOB_FIELDS: &str = "*, created_at < now()-interval '7 days' AS expired, floor(extract(epoch FROM now()-created_at))::bigint AS age_seconds";

impl Store {
    // The event generation is its original file mtime/length, not a process ID.
    // Rediscovering a pending file never resets persisted attempts or deadlines.
    pub(crate) async fn sync_auto_events(
        &self,
        queue: &str,
        events: &[AutoEvent],
    ) -> Result<Vec<AutoJob>, sqlx::Error> {
        let names: Vec<_> = events.iter().map(|e| e.name.clone()).collect();
        let generations: Vec<_> = events.iter().map(|e| e.generation.clone()).collect();
        let kinds: Vec<_> = events.iter().map(|e| e.kind.clone()).collect();
        let ids: Vec<_> = events.iter().map(|e| e.item_id.clone()).collect();
        let backfills: Vec<_> = events.iter().map(|e| e.backfill).collect();
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO collect_metadata_auto_jobs(queue_key,event_name,generation,item_type,item_id,backfill)
            SELECT $1,e.* FROM unnest($2::text[],$3::text[],$4::text[],$5::text[],$6::boolean[])
                AS e(event_name,generation,item_type,item_id,backfill)
            ON CONFLICT(queue_key,event_name,generation) DO NOTHING")
            .bind(queue).bind(&names).bind(&generations).bind(kinds).bind(ids).bind(backfills)
            .execute(&mut *tx).await?;
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {JOB_FIELDS} FROM collect_metadata_auto_jobs j
            WHERE queue_key=$1 AND status IN ('completed','review','skipped','failed','missing')
            AND EXISTS(SELECT 1 FROM unnest($2::text[],$3::text[]) AS e(name,generation)
                WHERE e.name=j.event_name AND e.generation=j.generation)"
        )))
        .bind(queue)
        .bind(names)
        .bind(generations)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows.into_iter().map(job).collect())
    }

    pub(crate) async fn claim_auto_jobs(&self, queue: &str) -> Result<Vec<AutoJob>, sqlx::Error> {
        let lease = uuid::Uuid::new_v4().to_string();
        let rows=sqlx::query("WITH due AS (
            SELECT id FROM collect_metadata_auto_jobs WHERE queue_key=$1
                AND ((status IN ('queued','retry','waiting_source') AND next_retry_at<=now())
                    OR (status='running' AND lease_until<=now()))
            ORDER BY backfill,id FOR UPDATE SKIP LOCKED LIMIT 4
        ) UPDATE collect_metadata_auto_jobs j SET status='running',attempts=attempts+1,
            lease_token=$2,lease_until=now()+interval '90 seconds',updated_at=clock_timestamp()
            FROM due WHERE j.id=due.id RETURNING j.*,j.created_at<now()-interval '7 days' AS expired,
                floor(extract(epoch FROM now()-j.created_at))::bigint AS age_seconds")
            .bind(queue).bind(lease).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(job).collect())
    }

    pub(crate) async fn renew_auto_job(&self, job: &AutoJob) -> Result<bool, sqlx::Error> {
        Ok(sqlx::query(
            "UPDATE collect_metadata_auto_jobs SET lease_until=now()+interval '90 seconds'
            WHERE id=$1 AND status='running' AND lease_token=$2 AND lease_until>now()",
        )
        .bind(job.id)
        .bind(&job.lease)
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1)
    }

    pub(crate) async fn finish_auto_job(
        &self,
        job: &AutoJob,
        status: &str,
        error_kind: Option<&str>,
        error: Option<&str>,
        delay: i64,
        failed: bool,
    ) -> Result<bool, sqlx::Error> {
        Ok(sqlx::query("UPDATE collect_metadata_auto_jobs SET status=$3,error_kind=$4,last_error=$5,
            failures=failures+$7::integer,next_retry_at=now()+$6::bigint*interval '1 second',
            lease_token=NULL,lease_until=NULL,updated_at=clock_timestamp(),
            finished_at=CASE WHEN $3 IN ('completed','review','skipped','failed','missing') THEN clock_timestamp() END
            WHERE id=$1 AND status='running' AND lease_token=$2 AND lease_until>now()")
            .bind(job.id).bind(&job.lease).bind(status).bind(error_kind).bind(error)
            .bind(delay).bind(i32::from(failed)).execute(&self.pool).await?.rows_affected()==1)
    }

    pub(crate) async fn auto_dependency_due(
        &self,
        queue: &str,
    ) -> Result<Option<bool>, sqlx::Error> {
        sqlx::query("INSERT INTO collect_metadata_auto_dependencies(queue_key) VALUES($1) ON CONFLICT DO NOTHING")
            .bind(queue).execute(&self.pool).await?;
        // None requests a fresh probe, Some caches the bounded readiness result.
        sqlx::query_scalar(
            "SELECT CASE WHEN next_check_at<=now() THEN NULL ELSE ready END
            FROM collect_metadata_auto_dependencies WHERE queue_key=$1",
        )
        .bind(queue)
        .fetch_one(&self.pool)
        .await
    }

    pub(crate) async fn set_auto_dependency(
        &self,
        queue: &str,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE collect_metadata_auto_dependencies SET ready=$2,
            failures=CASE WHEN $2 THEN 0 ELSE failures+1 END,last_error=$3,
            next_check_at=now()+interval '10 seconds',updated_at=clock_timestamp() WHERE queue_key=$1")
            .bind(queue).bind(error.is_none()).bind(error).execute(&self.pool).await?;
        Ok(())
    }

    pub(crate) async fn auto_scrape_state(&self) -> Result<Value, sqlx::Error> {
        let counts: Value = sqlx::query_scalar(
            "SELECT coalesce(jsonb_object_agg(status,n),'{}'::jsonb)
            FROM (SELECT status,count(*) n FROM collect_metadata_auto_jobs GROUP BY status) q",
        )
        .fetch_one(&self.pool)
        .await?;
        let dependencies: Value = sqlx::query_scalar(
            "SELECT coalesce(jsonb_agg(jsonb_build_object(
            'ready',ready,'failures',failures,'last_error',last_error,'next_check_at',next_check_at,
            'updated_at',updated_at)),'[]'::jsonb) FROM collect_metadata_auto_dependencies",
        )
        .fetch_one(&self.pool)
        .await?;
        let jobs:Value=sqlx::query_scalar("SELECT coalesce(jsonb_agg(to_jsonb(q)),'[]'::jsonb) FROM (
            SELECT id,item_type,item_id,backfill,status,attempts,failures,next_retry_at,error_kind,last_error,
                lease_until,created_at,updated_at,finished_at
            FROM collect_metadata_auto_jobs ORDER BY updated_at DESC,id DESC LIMIT 200) q")
            .fetch_one(&self.pool).await?;
        Ok(json!({"counts":counts,"dependencies":dependencies,"jobs":jobs}))
    }
}

#[cfg(test)]
mod tests;
