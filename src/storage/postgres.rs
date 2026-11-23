use std::time::Duration;

use crate::{MediaKind, diagnostics};

use serde::Serialize;
use serde_json::Value;
use sqlx::{
    AssertSqlSafe, PgConnection, PgPool, Postgres, Row,
    postgres::{PgConnectOptions, PgPoolOptions, PgRow},
};

pub(crate) struct Store {
    pub(super) pool: PgPool,
}

#[derive(Serialize)]
pub struct HistoryItem {
    pub(crate) id: i64,
    code: String,
    title: Option<String>,
    media_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    original_languages: Vec<String>,
    cover_url: Option<String>,
    year: Option<i16>,
    collected_at: String,
}

// Only this fixed projection is interpolated into SQL. Values use bind parameters.
const HISTORY_FIELDS: &str = "id, code, title,
    coalesce(document->>'media_type', 'av') as media_type,
    document->>'scope' as scope,
    coalesce(document->'fields'->'original_languages', '[]'::jsonb) as original_languages,
    coalesce(nullif(document->'fields'->>'poster_url',''), nullif(document->'fields'->>'cover_url','')) as cover_url,
    CASE WHEN document->'fields'->>'release_date' ~ '^[0-9]{4}(-|$)'
        THEN left(document->'fields'->>'release_date',4)::smallint END as year,
    to_char(collected_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS collected_at";

impl Store {
    pub(crate) async fn connect(options: &PgConnectOptions) -> Result<Self, String> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(6))
            .connect_with(options.clone())
            .await
            .map_err(|error| {
                diagnostics::postgres_error("postgres_connect_failed", &error);
                "无法连接 PostgreSQL。请检查主机、端口、数据库名、账号和 TLS 设置。"
            })?;

        let migrate = async {
            let mut transaction = pool.begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock(414290617)")
                .execute(&mut *transaction)
                .await?;
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS collect_metadata_snapshots (
                    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                    code text NOT NULL,
                    title text,
                    collected_at timestamptz NOT NULL DEFAULT now(),
                    document jsonb NOT NULL
                )",
            )
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS collect_metadata_emby_items (
                    server_id text NOT NULL, item_id text NOT NULL, document jsonb NOT NULL,
                    updated_at timestamptz NOT NULL DEFAULT now(), PRIMARY KEY(server_id, item_id)
                )",
            )
            .execute(&mut *transaction)
            .await?;
            #[cfg(feature = "web")]
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS collect_metadata_anime_verified_episode_airdates (
                    server_id text NOT NULL,
                    emby_episode_id text NOT NULL,
                    emby_series_id text NOT NULL,
                    season_number integer NOT NULL CHECK (season_number > 0),
                    episode_number integer NOT NULL CHECK (episode_number > 0),
                    bangumi_episode_id integer NOT NULL CHECK (bangumi_episode_id > 0),
                    emby_title text NOT NULL CHECK (length(btrim(emby_title)) > 0),
                    tvdb_id text CHECK (tvdb_id ~ '^[0-9]+$'),
                    emby_local_airdate date NOT NULL,
                    official_airdate date NOT NULL,
                    evidence_url text NOT NULL CHECK (left(evidence_url, 8) = 'https://' AND length(evidence_url) > 8),
                    verified_at timestamptz NOT NULL DEFAULT now(),
                    PRIMARY KEY (server_id, emby_episode_id)
                )",
            )
            .execute(&mut *transaction)
            .await?;
            super::batch::migrate(&mut transaction).await?;
            #[cfg(feature = "web")]
            super::auto_scrape::migrate(&mut transaction).await?;
            sqlx::query(
                "ALTER TABLE collect_metadata_snapshots
                 ADD COLUMN IF NOT EXISTS history_visible boolean NOT NULL DEFAULT true",
            )
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await
        };
        migrate.await.map_err(|error| {
            diagnostics::postgres_error("postgres_migration_failed", &error);
            "无法初始化 PostgreSQL 的采集表。请检查账号是否能创建和修改应用表。"
        })?;
        Ok(Self { pool })
    }

    #[cfg(feature = "web")]
    pub(crate) async fn verified_episode_airdate(
        &self,
        server: &str,
        item: &Value,
        bangumi_episode_id: u32,
        local: jiff::civil::Date,
        official: jiff::civil::Date,
    ) -> Result<bool, String> {
        use crate::providers::media::emby::provider_id;

        let bangumi_episode_id = i32::try_from(bangumi_episode_id)
            .map_err(|_| "Bangumi 分集 ID 超出 PostgreSQL 的可用范围。")?;
        sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM collect_metadata_anime_verified_episode_airdates
                WHERE server_id=$1 AND emby_episode_id=$2 AND emby_series_id=$3
                  AND season_number=$4 AND episode_number=$5 AND bangumi_episode_id=$6
                  AND emby_local_airdate=$7::text::date AND official_airdate=$8::text::date
                  AND emby_title=$9 AND tvdb_id IS NOT DISTINCT FROM $10
            )",
        )
        .bind(server)
        .bind(item["Id"].as_str())
        .bind(item["SeriesId"].as_str())
        .bind(item["ParentIndexNumber"].as_i64())
        .bind(item["IndexNumber"].as_i64())
        .bind(bangumi_episode_id)
        .bind(local.to_string())
        .bind(official.to_string())
        .bind(item["Name"].as_str())
        .bind(provider_id(item, "Tvdb"))
        .fetch_one(&self.pool)
        .await
        .map_err(|error| {
            diagnostics::postgres_error("postgres_anime_episode_airdate_read_failed", &error);
            "读取 PostgreSQL 的已核对分集放送日失败。".to_owned()
        })
    }

    pub(crate) async fn emby_item(&self, server: &str, id: &str) -> Result<Option<Value>, String> {
        sqlx::query_scalar(
            "SELECT document FROM collect_metadata_emby_items WHERE server_id=$1 AND item_id=$2",
        )
        .bind(server)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            diagnostics::postgres_error("postgres_emby_read_failed", &error);
            "读取已同步的 Emby 条目失败。请检查 PostgreSQL。".into()
        })
    }

    pub(crate) async fn upsert_emby_item(
        &self,
        server: &str,
        id: &str,
        doc: &Value,
    ) -> Result<(), String> {
        sqlx::query("INSERT INTO collect_metadata_emby_items(server_id,item_id,document) VALUES($1,$2,$3)
            ON CONFLICT(server_id,item_id) DO UPDATE SET document=excluded.document, updated_at=now()
            WHERE collect_metadata_emby_items.document IS DISTINCT FROM excluded.document")
            .bind(server).bind(id).bind(doc).execute(&self.pool).await
            .map(|_| ()).map_err(|error| {
                diagnostics::postgres_error("postgres_emby_write_failed", &error);
                "写入 Emby 条目失败。请检查 PostgreSQL。".into()
            })
    }

    #[cfg(feature = "web")]
    pub(crate) async fn verified_bangumi_season(
        &self,
        server_id: &str,
        series_id: &str,
        season: i32,
    ) -> Result<Option<Vec<u32>>, String> {
        let ids: Option<Vec<i32>> = sqlx::query_scalar(
            "SELECT bangumi_subject_ids FROM collect_metadata_anime_verified_seasons
             WHERE server_id=$1 AND emby_series_id=$2 AND season_number=$3",
        )
        .bind(server_id)
        .bind(series_id)
        .bind(season)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            diagnostics::postgres_error("postgres_anime_mapping_read_failed", &error);
            "读取 PostgreSQL 的已核对分季映射失败。".to_owned()
        })?;
        ids.map(|ids| {
            let distinct: std::collections::HashSet<_> = ids.iter().collect();
            if ids.is_empty() || distinct.len() != ids.len() || ids.iter().any(|id| *id <= 0) {
                return Err("PostgreSQL 的分季映射包含空值、重复或无效的 Bangumi ID。".to_owned());
            }
            Ok(ids.into_iter().map(|id| id as u32).collect())
        })
        .transpose()
    }

    #[cfg(feature = "web")]
    pub(crate) async fn archived_bangumi_episodes(
        &self,
        subjects: &[u32],
    ) -> Result<Vec<Value>, String> {
        let ids = subjects
            .iter()
            .copied()
            .map(i32::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "Bangumi 季条目 ID 超出 PostgreSQL 的可用范围。".to_owned())?;
        // An incomplete part could shift the offsets of subsequent parts.
        // Trust the archive only if it lists every declared regular episode
        // for every verified part, using the same snapshot for both tables.
        sqlx::query_scalar(
            "WITH complete_parts AS (
                SELECT requested.id, requested.part_order, subject.archive_version
                FROM unnest($1::int[]) WITH ORDINALITY AS requested(id, part_order)
                JOIN collect_metadata_anime_subjects AS subject ON subject.bangumi_id=requested.id
                JOIN collect_metadata_anime_episodes AS episode ON episode.bangumi_id=requested.id
                    AND episode.kind=0 AND episode.archive_version=subject.archive_version
                GROUP BY requested.id, requested.part_order, subject.expected_episodes, subject.archive_version
                HAVING count(*)=subject.expected_episodes
                    AND count(*) FILTER (WHERE episode.source_number > 0
                        AND episode.source_number=trunc(episode.source_number)
                        AND episode.source_number<=9223372036854775807)=count(*)
            )
            SELECT jsonb_build_object(
                'id', episode.bangumi_episode_id, 'subject_id', episode.bangumi_id,
                'sort', episode.source_number::bigint, 'airdate', episode.airdate, 'type', episode.kind)
            FROM complete_parts AS part
            JOIN collect_metadata_anime_subjects AS subject ON subject.bangumi_id=part.id
            JOIN collect_metadata_anime_episodes AS episode ON episode.bangumi_id=part.id
                AND episode.kind=0 AND episode.archive_version=subject.archive_version
            WHERE (SELECT count(*)=cardinality($1::int[]) AND count(DISTINCT archive_version)=1 FROM complete_parts)
            ORDER BY part.part_order, episode.source_number",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            diagnostics::postgres_error("postgres_anime_archive_read_failed", &error);
            "读取 PostgreSQL 的 Bangumi 分集目录失败。".to_owned()
        })
    }

    pub(crate) async fn check_connection(&self) -> Result<(), String> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(|error| {
                diagnostics::postgres_error("postgres_check_failed", &error);
                "PostgreSQL 连接检查失败。请检查数据库是否在线，再测试连接。".into()
            })
    }

    pub(crate) async fn save(&self, code: &str, document: &Value) -> Result<HistoryItem, String> {
        let mut connection = self.pool.acquire().await.map_err(|error| {
            diagnostics::postgres_error("postgres_snapshot_save_failed", &error);
            "采集完成，但 PostgreSQL 保存失败。请检查数据库连接。"
        })?;
        Self::insert_snapshot(&mut connection, code, document)
            .await
            .map_err(|error| {
                diagnostics::postgres_error("postgres_snapshot_save_failed", &error);
                "采集完成，但 PostgreSQL 保存失败。请检查数据库连接。".into()
            })
    }

    pub(super) async fn insert_snapshot(
        connection: &mut PgConnection,
        code: &str,
        document: &Value,
    ) -> Result<HistoryItem, sqlx::Error> {
        let fields = &document["fields"];
        let title = [
            "title",
            "title_ja",
            "title_original",
            "title_en",
            "title_site",
        ]
        .iter()
        .find_map(|field| fields[field].as_str());
        let statement = format!(
            "INSERT INTO collect_metadata_snapshots (code, title, document) VALUES ($1, $2, $3)
             RETURNING {HISTORY_FIELDS}"
        );
        let row = sqlx::query::<Postgres>(AssertSqlSafe(statement))
            .bind(code)
            .bind(title)
            .bind(document)
            .fetch_one(connection)
            .await?;
        Ok(history_item(&row))
    }

    pub(crate) async fn history(
        &self,
        before: Option<i64>,
        kind: Option<MediaKind>,
        language: Option<&str>,
    ) -> Result<Vec<HistoryItem>, String> {
        let statement = format!(
            "SELECT {HISTORY_FIELDS} FROM collect_metadata_snapshots
             WHERE history_visible AND id < $1
             AND ($2::text IS NULL OR coalesce(document->>'media_type', 'av') = $2)
             AND ($3::text IS NULL
                  OR ($3 = 'und' AND coalesce(document->'fields'->'original_languages', '[]'::jsonb) = '[]'::jsonb)
                  OR (document->'fields'->'original_languages') ? $3)
             ORDER BY id DESC LIMIT 50"
        );
        let kind = kind.map(|value| match value {
            MediaKind::Av => "av",
            MediaKind::Movie => "movie",
            MediaKind::Series => "series",
        });
        let rows = sqlx::query(AssertSqlSafe(statement))
            .bind(before.unwrap_or(i64::MAX))
            .bind(kind)
            .bind(language)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| {
                diagnostics::postgres_error("postgres_history_failed", &error);
                "无法读取 PostgreSQL 中的采集历史。"
            })?;
        Ok(rows.iter().map(history_item).collect())
    }

    pub(crate) async fn history_languages(&self) -> Result<Vec<String>, String> {
        sqlx::query_scalar(
            "SELECT DISTINCT language
             FROM collect_metadata_snapshots
             CROSS JOIN LATERAL jsonb_array_elements_text(document->'fields'->'original_languages') as languages(language)
             WHERE history_visible
             ORDER BY language"
        ).fetch_all(&self.pool).await.map_err(|error| { diagnostics::postgres_error("postgres_history_languages_failed", &error); "无法读取历史记录中的语言。".into() })
    }

    pub(crate) async fn clear_history(&self) -> Result<i64, String> {
        // A batch publishes its snapshot and reference in the same transaction.
        // Keep those documents available for batch results and file writeback.
        sqlx::query_scalar(
            "WITH batch_snapshots AS (
                SELECT snapshot_id FROM collect_metadata_batch_rows WHERE snapshot_id IS NOT NULL
             ), retained AS (
                UPDATE collect_metadata_snapshots s SET history_visible = false
                WHERE history_visible AND EXISTS (SELECT 1 FROM batch_snapshots b WHERE b.snapshot_id = s.id)
                RETURNING 1
             ), removed AS (
                DELETE FROM collect_metadata_snapshots s
                WHERE NOT EXISTS (SELECT 1 FROM batch_snapshots b WHERE b.snapshot_id = s.id)
                RETURNING history_visible
             )
             SELECT (SELECT count(*) FROM retained) + (SELECT count(*) FROM removed WHERE history_visible)",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|error| {
            diagnostics::postgres_error("postgres_history_clear_failed", &error);
            "无法清空 PostgreSQL 中的采集历史。请检查数据库连接和写入权限。".into()
        })
    }

    pub(crate) async fn load(
        &self,
        id: i64,
        include_cleared: bool,
    ) -> Result<(HistoryItem, Value), String> {
        let statement = format!(
            "SELECT {HISTORY_FIELDS}, document FROM collect_metadata_snapshots
             WHERE id = $1 AND ($2 OR history_visible)"
        );
        let row = sqlx::query(AssertSqlSafe(statement))
            .bind(id)
            .bind(include_cleared)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| {
                diagnostics::postgres_error("postgres_snapshot_read_failed", &error);
                "无法读取 PostgreSQL 中的采集快照。"
            })?
            .ok_or("没有找到这条采集记录。")?;
        Ok((history_item(&row), row.get("document")))
    }
}

fn history_item(row: &PgRow) -> HistoryItem {
    HistoryItem {
        id: row.get("id"),
        code: row.get("code"),
        title: row.get("title"),
        media_type: row.get("media_type"),
        scope: row.get("scope"),
        original_languages: row
            .get::<sqlx::types::Json<Vec<String>>, _>("original_languages")
            .0,
        cover_url: row.get("cover_url"),
        year: row.get("year"),
        collected_at: row.get("collected_at"),
    }
}
