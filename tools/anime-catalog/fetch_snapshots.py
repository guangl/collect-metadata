#!/usr/bin/env python3
"""Fetch pinned, checksum-verified Bangumi Archive and open-ani release assets."""

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
from import_catalog import digest, snapshot_json
from pathlib import Path
import re
import shutil
import subprocess


ARCHIVE_INDEX = "https://raw.githubusercontent.com/bangumi/Archive/master/aux/latest.json"
MAPPING_RELEASE = "https://api.github.com/repos/open-ani/bangumi-tmdb/releases/latest"
CHUNK_SIZE = 4 * 1024 * 1024
ASSETS = ("manifest.json", "subjects.json", "episodes.json", "coverage.json")


def download_json(url, target):
    temporary = target.with_suffix(".download")
    subprocess.run(["curl", "-fLsS", "--retry", "2", "--connect-timeout", "12",
                    "--max-time", "40", url, "-o", str(temporary)], check=True)
    data = snapshot_json(temporary)
    temporary.replace(target)
    return data


def download(root, name, url, size, expected):
    expected = expected.split(":")[-1]
    if not re.fullmatch(r"[0-9a-f]{64}", expected) or not isinstance(size, int) or size <= 0:
        raise ValueError(f"Invalid release asset metadata for {name}")
    target = root / name
    if target.is_file() and target.stat().st_size == size and digest(target) == expected:
        print(f"Cached {name}", flush=True)
        return
    parts = root / f".{name}-{expected[:16]}"
    parts.mkdir(exist_ok=True)
    starts = range(0, size, CHUNK_SIZE)

    def fetch(start):
        length = min(CHUNK_SIZE, size - start)
        output = parts / str(start)
        if output.exists() and output.stat().st_size == length:
            return
        temporary = output.with_suffix(".download")
        request = subprocess.run(
            ["curl", "-fLsS", "--retry", "2", "--connect-timeout", "12", "--max-time", "120",
             "--max-filesize", str(length), "--range", f"{start}-{start + length - 1}",
             url, "-o", str(temporary), "-w", "%{http_code}"],
            check=True, stdout=subprocess.PIPE,
        )
        if temporary.stat().st_size != length or (
            request.stdout != b"206" and not (request.stdout == b"200" and length == size)
        ):
            raise ValueError(f"Invalid HTTP range for {name} at {start}")
        temporary.replace(output)

    with ThreadPoolExecutor(max_workers=16) as threads:
        for pending in as_completed([threads.submit(fetch, start) for start in starts]):
            pending.result()
    assembled = target.with_suffix(".assembling")
    with assembled.open("wb") as output:
        for start in starts:
            with (parts / str(start)).open("rb") as stream:
                shutil.copyfileobj(stream, output)
    if digest(assembled) != expected:
        assembled.unlink()
        shutil.rmtree(parts)
        raise ValueError(f"Checksum mismatch for {name}")
    assembled.replace(target)
    shutil.rmtree(parts)
    print(f"Verified {name} ({size} bytes)", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cache", type=Path)
    args = parser.parse_args()
    root = args.cache
    root.mkdir(parents=True, exist_ok=True)
    archive = download_json(ARCHIVE_INDEX, root / "bangumi-latest.json")
    if not re.fullmatch(r"dump-[0-9TZ.\-]+\.zip", archive["name"]):
        raise ValueError("Invalid Bangumi Archive name")
    archive_url = "https://github.com/bangumi/Archive/releases/download/archive/" + archive["name"]
    if archive["browser_download_url"] != archive_url:
        raise ValueError("Invalid Bangumi Archive source URL")
    release = download_json(MAPPING_RELEASE, root / "mapping-release.json")
    tag = release["tag_name"]
    if not re.fullmatch(r"data-[a-f0-9]{40}", tag):
        raise ValueError("Invalid open-ani release tag")
    base = f"https://github.com/open-ani/bangumi-tmdb/releases/download/{tag}"
    assets = {asset["name"]: asset for asset in release["assets"]}
    download(root, "bangumi.zip", archive_url, archive["size"], archive["digest"])
    for name in ASSETS:
        asset = assets[name]
        url = f"{base}/{name}"
        if asset["browser_download_url"] != url:
            raise ValueError(f"Invalid open-ani source URL for {name}")
        download(root, name, url, asset["size"], asset["digest"])


if __name__ == "__main__":
    main()
