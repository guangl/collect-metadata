#!/usr/bin/env python3
"""Emit a single PostgreSQL transaction from verified source snapshots."""

import argparse
from datetime import date
from hashlib import sha256
import json
from pathlib import Path, PurePosixPath
import re
import sys
import zipfile


RELEASE_FORMATS = {1: "TV", 2: "OVA", 5: "WEB", 0: "UNKNOWN"}
TABLE = "collect_metadata_anime_"


def digest(path):
    result = sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def checked(path, expected):
    if digest(path) != expected.split(":")[-1]:
        raise ValueError(f"SHA-256 mismatch for {path.name}")


def snapshot_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


def rows(archive, name):
    matches = [member for member in archive.namelist() if PurePosixPath(member).name == name]
    if len(matches) != 1:
        raise ValueError(f"Expected one {name} in Bangumi Archive")
    with archive.open(matches[0]) as stream:
        for line in stream:
            yield json.loads(line)


def calendar_date(value):
    if not isinstance(value, str):
        return None, None
    for pattern, suffix, precision in (
        (r"\d{4}-\d{2}-\d{2}", "", "day"),
        (r"\d{4}-\d{2}", "-01", "month"),
        (r"\d{4}", "-01-01", "year"),
    ):
        if re.fullmatch(pattern, value):
            try:
                return date.fromisoformat(value + suffix), precision
            except ValueError:
                break
    return None, None


def expected_count(row):
    match = re.search(r"(?m)^\|\s*(?:话数|話數|集数|集數)\s*=\s*([0-9]+)\s*$", row.get("infobox") or "")
    return int(match[1]) or None if match else None


def encode(value):
    if value is None:
        return r"\N"
    if isinstance(value, bool):
        return "t" if value else "f"
    return str(value).replace("\x00", "").replace("\\", "\\\\").replace(
        "\t", "\\t"
    ).replace("\r", "\\r").replace("\n", "\\n")


def load(name, columns, identity, records, keep_existing=False):
    table = TABLE + name
    stage = "stage_" + name
    keys = ", ".join(identity)
    fields = ", ".join(columns)
    print(f"CREATE TEMP TABLE {stage} (LIKE {table} INCLUDING DEFAULTS) ON COMMIT DROP;")
    print(f"COPY {stage} ({fields}) FROM STDIN;")
    for row in records:
        print("\t".join(encode(value) for value in row))
    print(r"\.")
    changes = ", ".join(f"{column} = excluded.{column}" for column in columns if column not in identity)
    update = "NOTHING" if keep_existing else f"UPDATE SET {changes}"
    if not keep_existing:
        update += f" WHERE {table} IS DISTINCT FROM excluded"
    print(f"INSERT INTO {table} ({fields}) SELECT {fields} FROM {stage} ON CONFLICT ({keys}) DO {update};")


def catalog_subjects(archive, version):
    subjects = {}
    for row in rows(archive, "subject.jsonlines"):
        if row.get("type") != 2:
            continue
        platform = RELEASE_FORMATS.get(row.get("platform"))
        if platform is None:
            continue
        premiere, precision = calendar_date(row.get("date"))
        subject = row["id"]
        subjects[subject] = (subject, row["name"], row.get("name_cn") or None,
                             platform, premiere, precision, expected_count(row), bool(row.get("nsfw")), version)
    if not subjects:
        raise ValueError("Bangumi Archive contains no serial animation subjects")
    return subjects


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("snapshot", type=Path, help="Downloaded and pinned Bangumi/TMDB snapshots")
    parser.add_argument("--verified-seasons", type=Path, help="Optional review records. Existing verified rows win.")
    args = parser.parse_args()
    root = args.snapshot
    bgm = snapshot_json(root / "bangumi-latest.json")
    manifest = snapshot_json(root / "manifest.json")
    if manifest.get("schemaVersion") != 1 or manifest.get("sourceDirty") is True:
        parser.error("Unsupported bangumi-tmdb release manifest")
    release = "data-" + manifest["sourceCommit"]
    if not re.fullmatch(r"data-[a-f0-9]{40}", release):
        parser.error("Invalid bangumi-tmdb source revision")
    pinned = "https://github.com/open-ani/bangumi-tmdb/releases/download/" + release
    archive_file = root / "bangumi.zip"
    checked(archive_file, bgm["digest"])
    for name in ("subjects.json", "episodes.json", "coverage.json"):
        checked(root / name, manifest["files"][name]["sha256"])
    mappings = snapshot_json(root / "subjects.json")
    episode_targets = snapshot_json(root / "episodes.json")
    coverage = snapshot_json(root / "coverage.json")
    for snapshot in (mappings, episode_targets, coverage):
        if snapshot.get("schemaVersion") != 1:
            parser.error("Unsupported bangumi-tmdb data version")
    archive_version = bgm["name"]
    with zipfile.ZipFile(archive_file) as archive:
        subjects = catalog_subjects(archive, archive_version)
        print("BEGIN;")
        print("SELECT pg_advisory_xact_lock(414290618);")
        print(Path(__file__).with_name("schema.sql").read_text(encoding="utf-8"))
        load("subjects", ("bangumi_id", "title", "title_zh", "format", "premiere", "premiere_precision",
                          "expected_episodes", "is_adult", "archive_version"), ("bangumi_id",), subjects.values())
        episodes = set()

        def episode_rows():
            for row in rows(archive, "episode.jsonlines"):
                if row["subject_id"] not in subjects:
                    continue
                episode = row["id"]
                episodes.add(episode)
                airdate, precision = calendar_date(row.get("airdate"))
                yield (episode, row["subject_id"], row["type"], row.get("sort"), row.get("name") or None,
                       row.get("name_cn") or None, airdate if precision == "day" else None, archive_version)
            if not episodes:
                raise ValueError("Bangumi Archive contains no serial animation episodes")

        load("episodes", ("bangumi_episode_id", "bangumi_id", "kind", "source_number", "title", "title_zh",
                          "airdate", "archive_version"), ("bangumi_episode_id",), episode_rows())
        relations = {
            (row["subject_id"], row["related_subject_id"], row["relation_type"], archive_version)
            for row in rows(archive, "subject-relations.jsonlines")
            if row["subject_id"] in subjects and row["related_subject_id"] in subjects
        }
        load("relations", ("bangumi_id", "related_bangumi_id", "relation_type", "archive_version"),
             ("bangumi_id", "related_bangumi_id", "relation_type"), sorted(relations))
    source_maps = {row["bangumiId"]: row["targets"] for row in mappings["subjects"]}
    coverage_by_id = {row["bangumiId"]: row for row in coverage["subjects"]}
    load("tmdb_subjects", ("bangumi_id", "mapping", "coverage", "source_version"), ("bangumi_id",),
         ((subject, json.dumps(source_maps[subject]) if subject in source_maps else None,
           json.dumps(coverage_by_id[subject]) if subject in coverage_by_id else None, release)
          for subject in subjects if subject in coverage_by_id or subject in source_maps))
    load("tmdb_episodes", ("bangumi_episode_id", "targets", "source_version"), ("bangumi_episode_id",),
         ((row["bangumiEpisodeId"], json.dumps(row["targets"]), release) for row in episode_targets["episodes"]
          if row["bangumiEpisodeId"] in episodes and row["bangumiId"] in subjects))
    if args.verified_seasons:
        fields = ("server_id", "emby_series_id", "season_number", "series_title", "first_aired",
                  "expected_episodes", "bangumi_subject_ids", "evidence")

        def verified_rows():
            for row in snapshot_json(args.verified_seasons):
                ids = row["bangumi_subject_ids"]
                if not ids or len(ids) != len(set(ids)) or any(id not in subjects for id in ids):
                    raise ValueError("Verified mapping refers to absent or duplicate Bangumi entries")
                yield tuple(row[field] for field in fields[:-2]) + (
                    "{" + ",".join(map(str, ids)) + "}", json.dumps(row["evidence"]),
                )

        load("verified_seasons", fields, fields[:3], verified_rows(), keep_existing=True)
    load("imports", ("source", "version", "source_url", "sha256"), ("source", "version"),
         [("bangumi-archive", archive_version, bgm["browser_download_url"], bgm["digest"].split(":")[-1]),
          ("open-ani-bangumi-tmdb", release, pinned + "/manifest.json", digest(root / "manifest.json"))],
         keep_existing=True)
    print("COMMIT;")
    print("SELECT count(*) AS catalog_in_20_years, count(mapping) AS with_tmdb_targets "
          "FROM collect_metadata_anime_quarters;")
    print("SELECT quarter_start, count(*) AS releases FROM collect_metadata_anime_quarters "
          "WHERE quarter_start >= (date_trunc('quarter', now() AT TIME ZONE 'Asia/Shanghai') - interval '3 months')::date "
          "GROUP BY quarter_start ORDER BY quarter_start LIMIT 8;")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, TypeError, OSError, zipfile.BadZipFile) as error:
        print(f"Anime catalog import aborted: {error}", file=sys.stderr)
        raise SystemExit(1) from error
