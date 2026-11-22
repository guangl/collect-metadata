use sqlx::PgConnection;

pub(in crate::storage) async fn migrate(connection: &mut PgConnection) -> Result<(), sqlx::Error> {
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
