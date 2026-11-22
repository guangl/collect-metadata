CREATE TABLE IF NOT EXISTS collect_metadata_anime_imports (
    source text NOT NULL,
    version text NOT NULL,
    source_url text NOT NULL,
    sha256 text NOT NULL,
    imported_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (source, version)
);

CREATE TABLE IF NOT EXISTS collect_metadata_anime_subjects (
    bangumi_id integer PRIMARY KEY,
    title text NOT NULL,
    title_zh text,
    format text NOT NULL,
    premiere date,
    premiere_precision text,
    expected_episodes integer,
    is_adult boolean NOT NULL,
    archive_version text NOT NULL
);

CREATE TABLE IF NOT EXISTS collect_metadata_anime_episodes (
    bangumi_episode_id integer PRIMARY KEY,
    bangumi_id integer NOT NULL REFERENCES collect_metadata_anime_subjects (bangumi_id),
    kind smallint NOT NULL,
    source_number numeric,
    title text,
    title_zh text,
    airdate date,
    archive_version text NOT NULL
);

CREATE INDEX IF NOT EXISTS collect_metadata_anime_episodes_subject_idx
    ON collect_metadata_anime_episodes (bangumi_id, kind, source_number);

CREATE TABLE IF NOT EXISTS collect_metadata_anime_relations (
    bangumi_id integer NOT NULL REFERENCES collect_metadata_anime_subjects (bangumi_id),
    related_bangumi_id integer NOT NULL REFERENCES collect_metadata_anime_subjects (bangumi_id),
    relation_type integer NOT NULL,
    archive_version text NOT NULL,
    PRIMARY KEY (bangumi_id, related_bangumi_id, relation_type)
);

CREATE TABLE IF NOT EXISTS collect_metadata_anime_tmdb_subjects (
    bangumi_id integer PRIMARY KEY REFERENCES collect_metadata_anime_subjects (bangumi_id),
    mapping jsonb,
    coverage jsonb,
    source_version text NOT NULL
);

CREATE TABLE IF NOT EXISTS collect_metadata_anime_tmdb_episodes (
    bangumi_episode_id integer PRIMARY KEY REFERENCES collect_metadata_anime_episodes (bangumi_episode_id),
    targets jsonb NOT NULL CHECK (jsonb_typeof(targets) = 'array'),
    source_version text NOT NULL
);

CREATE TABLE IF NOT EXISTS collect_metadata_anime_verified_seasons (
    server_id text NOT NULL,
    emby_series_id text NOT NULL,
    season_number integer NOT NULL CHECK (season_number > 0),
    series_title text NOT NULL,
    first_aired date,
    expected_episodes integer CHECK (expected_episodes > 0),
    bangumi_subject_ids integer[] NOT NULL CHECK (cardinality(bangumi_subject_ids) > 0),
    evidence jsonb NOT NULL CHECK (jsonb_typeof(evidence) = 'array'),
    verified_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (server_id, emby_series_id, season_number)
);

CREATE OR REPLACE VIEW collect_metadata_anime_broadcasts AS
WITH episode_counts AS (
    SELECT bangumi_id,
        count(*) FILTER (WHERE kind = 0) AS regular_episodes_listed,
        count(*) FILTER (WHERE kind = 1) AS specials_listed,
        count(*) FILTER (WHERE kind = 0 AND airdate <= (now() AT TIME ZONE 'Asia/Shanghai')::date)
            AS regular_episodes_dated_through_today,
        min(airdate) FILTER (WHERE kind = 0) AS earliest_episode_airdate
    FROM collect_metadata_anime_episodes
    GROUP BY bangumi_id
)
SELECT subject.bangumi_id, subject.title, subject.title_zh, subject.format,
    subject.premiere AS catalog_premiere, subject.premiere_precision,
    episode_counts.earliest_episode_airdate,
    CASE
        WHEN subject.premiere_precision IN ('day', 'month') THEN subject.premiere
        ELSE episode_counts.earliest_episode_airdate
    END AS quarter_date,
    subject.expected_episodes,
    coalesce(episode_counts.regular_episodes_listed, 0) AS regular_episodes_listed,
    coalesce(episode_counts.specials_listed, 0) AS specials_listed,
    coalesce(episode_counts.regular_episodes_dated_through_today, 0) AS regular_episodes_dated_through_today,
    tmdb.mapping, tmdb.coverage
FROM collect_metadata_anime_subjects subject
LEFT JOIN episode_counts USING (bangumi_id)
LEFT JOIN collect_metadata_anime_tmdb_subjects tmdb USING (bangumi_id);

CREATE OR REPLACE VIEW collect_metadata_anime_quarters AS
SELECT date_trunc('quarter', quarter_date)::date AS quarter_start, broadcast.*
FROM collect_metadata_anime_broadcasts broadcast
WHERE quarter_date >= ((now() AT TIME ZONE 'Asia/Shanghai')::date - interval '20 years')::date;

CREATE OR REPLACE VIEW collect_metadata_anime_sequels AS
WITH links AS (
    SELECT bangumi_id AS previous_bangumi_id, related_bangumi_id AS next_bangumi_id
    FROM collect_metadata_anime_relations WHERE relation_type = 3
    UNION
    SELECT related_bangumi_id AS previous_bangumi_id, bangumi_id AS next_bangumi_id
    FROM collect_metadata_anime_relations WHERE relation_type = 2
)
SELECT links.previous_bangumi_id,
    coalesce(previous.title_zh, previous.title) AS previous_title,
    links.next_bangumi_id,
    coalesce(following.title_zh, following.title) AS next_title,
    following.catalog_premiere AS next_catalog_premiere,
    following.premiere_precision AS next_premiere_precision,
    following.quarter_date AS next_quarter_date,
    following.expected_episodes AS next_expected_episodes,
    following.regular_episodes_listed AS next_regular_episodes_listed
FROM links
JOIN collect_metadata_anime_broadcasts previous ON previous.bangumi_id = links.previous_bangumi_id
JOIN collect_metadata_anime_broadcasts following ON following.bangumi_id = links.next_bangumi_id;
