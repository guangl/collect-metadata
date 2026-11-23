CREATE TABLE IF NOT EXISTS collect_metadata_auto_jobs (
            id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
            queue_key text NOT NULL, event_name text NOT NULL, generation text NOT NULL,
            item_type text NOT NULL CHECK(item_type IN ('Movie','Video','Series','Season','Episode')),
            item_id text NOT NULL, backfill boolean NOT NULL,
            status text NOT NULL DEFAULT 'queued'
                CHECK(status IN ('queued','running','retry','waiting_source','review','completed','skipped','failed','missing')),
            attempts integer NOT NULL DEFAULT 0 CHECK(attempts>=0),
            failures integer NOT NULL DEFAULT 0 CHECK(failures>=0),
            next_retry_at timestamptz NOT NULL DEFAULT now(),
            error_kind text, last_error text,
            lease_token text, lease_until timestamptz,
            created_at timestamptz NOT NULL DEFAULT now(),
            updated_at timestamptz NOT NULL DEFAULT now(), finished_at timestamptz,
            UNIQUE(queue_key,event_name,generation)
        );

CREATE INDEX IF NOT EXISTS collect_metadata_auto_jobs_due_idx
            ON collect_metadata_auto_jobs(queue_key,next_retry_at,backfill,id)
            WHERE status IN ('queued','retry','waiting_source','running');

CREATE INDEX IF NOT EXISTS collect_metadata_auto_jobs_updated_idx
            ON collect_metadata_auto_jobs(updated_at,id);

CREATE TABLE IF NOT EXISTS collect_metadata_auto_dependencies (
            queue_key text PRIMARY KEY, ready boolean NOT NULL DEFAULT false,
            failures integer NOT NULL DEFAULT 0, last_error text,
            next_check_at timestamptz NOT NULL DEFAULT now(),
            updated_at timestamptz NOT NULL DEFAULT now()
        );
