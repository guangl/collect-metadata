"""Enforce the line coverage target for the automatic Emby metadata workflow."""

import argparse
import json
from pathlib import Path

MINIMUM_PERCENT = 95
CORE_FILES = (
    "src/batch/auto_scrape.rs",
    "src/batch/auto_scrape/durable.rs",
    "src/collect/emby/automatic.rs",
    "src/providers/media/bangumi.rs",
    "src/providers/media/tmdb/episode_groups.rs",
    "src/providers/media/tmdb/episodes.rs",
    "src/providers/media/emby/automatic.rs",
    "src/output/emby/publish.rs",
    "src/output/nfo.rs",
    "src/storage/auto_scrape.rs",
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path, help="JSON report from cargo llvm-cov")
    args = parser.parse_args()
    report = json.loads(args.report.read_text(encoding="utf-8"))
    root = Path(__file__).resolve().parent.parent
    files = {str(root / path): path for path in CORE_FILES}
    results = {}
    for object_report in report["data"]:
        for entry in object_report["files"]:
            filename = str(Path(entry["filename"]).resolve())
            if filename in files:
                path = files[filename]
                if path in results:
                    parser.error(f"duplicate coverage entry for {path}")
                results[path] = entry["summary"]["lines"]

    covered = total = 0
    for path in CORE_FILES:
        lines = results.get(path)
        if lines is None or lines["count"] == 0:
            parser.error(f"no executable lines reported for {path}")
        print(f'{lines["covered"]:5}/{lines["count"]:<5} {lines["percent"]:6.2f}%  {path}')
        covered += lines["covered"]
        total += lines["count"]

    print(f"{covered:5}/{total:<5} {100 * covered / total:6.2f}%  core line coverage")
    if covered * 100 < MINIMUM_PERCENT * total:
        parser.exit(1, f"Core line coverage must be at least {MINIMUM_PERCENT}%.\n")


if __name__ == "__main__":
    main()
