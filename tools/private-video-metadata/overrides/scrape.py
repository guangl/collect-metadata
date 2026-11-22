"""Metadata inspection and scraping workflows."""

import json
import math
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

from .artwork import (
    SAMPLES_DIR,
    download_with_alt,
    is_vr,
    make_poster,
    pick_sample_poster,
    ps_reference_url,
)
from .backdrops import publish_sample_backdrops
from .catalog import VIDEO_EXT, normalize
from .filesystem import atomic_write_bytes
from .intake import intake_tags
from .nfo import build_nfo, localized_synopsis, supplement_nfo
from .publisher import fetch_publisher, merge_publisher
from .sources import fetch_javbus, fetch_javdb, fetch_r18_text, merge_sources


def do_inspect(folders):
    """Print parsed catalog numbers and tags without network or filesystem writes."""
    print(f"\n{'Original folder':<44} {'Query ID':<16} Tags")
    print("─" * 92)
    recognized = unrecognized = 0
    for folder in folders:
        code, tags = normalize(folder.name)
        if code:
            recognized += 1
        else:
            unrecognized += 1
        mark = "✅" if code else "❌"
        print(
            f"{folder.name[:43]:<44} {str(code):<16} "
            f"{'/'.join(tags) or '—'}  {mark}"
        )
    print("─" * 92)
    print(
        f"Recognized {recognized} / Unrecognized {unrecognized} / Total {len(folders)}"
    )
    print("\n[inspect] No network requests or file writes were performed.")


def collect_metadata(fetcher, code, *, include_digital=True):
    # Collect the catalog fields before generating any localized display text.
    bus = fetch_javbus(fetcher, code)
    db = fetch_javdb(fetcher, code)
    data = merge_sources(bus, db)
    if data:
        data["source"] = "+".join(
            source for source, found in (("javbus", bus), ("javdb", db)) if found
        )
    publisher = fetch_publisher(fetcher, code, include_digital=include_digital)
    data = merge_publisher(data, publisher) or {}
    if data and not data.get("plot"):
        fallback = fetch_r18_text(fetcher, code) or {}
        missing = {key: value for key, value in fallback.items() if not data.get(key)}
        if missing:
            data.update(missing)
            data["source"] = "+".join(filter(None, [data.get("source"), "r18"]))
    return data or None


def probe_video(path):
    """Read the duration and primary video stream when the file can be probed."""
    command = [
        'ffprobe', '-v', 'error', '-select_streams', 'V:0', '-show_entries',
        'format=duration:stream=duration,codec_name,width,height', '-of', 'json', str(path),
    ]
    try:
        result = subprocess.run(command, check=True, capture_output=True, timeout=20)
        data = json.loads(result.stdout)
    except (OSError, subprocess.SubprocessError, ValueError):
        return {}
    stream = next(iter(data.get('streams', [])), None)
    if not stream:
        return {}
    for value in (data.get('format', {}).get('duration'), stream.get('duration')):
        try:
            seconds = float(value)
        except (TypeError, ValueError):
            continue
        if math.isfinite(seconds) and seconds > 0:
            stream['duration_seconds'] = round(seconds)
            stream['minutes'] = max(1, int(seconds // 60))
            break
    return stream


def supplement_one(folder, fetcher):
    """Fill supported fields in existing NFOs without re-scraping their artwork."""
    code, _ = normalize(folder.name)
    if not code:
        raise ValueError("Catalog number not recognized")
    videos = [path for path in folder.iterdir() if path.is_file() and path.suffix.lower() in VIDEO_EXT]
    sidecars = sorted({folder / "movie.nfo", *(video.with_suffix(".nfo") for video in videos)})
    sidecars = [path for path in sidecars if path.is_file() and not path.is_symlink()]
    if not sidecars:
        raise ValueError("No existing NFO")
    if fetcher is None:
        data = {}
    else:
        fetcher.begin_movie()
        try:
            data = collect_metadata(fetcher, code, include_digital=False) or {}
        finally:
            fetcher.finish_movie()

    proposed = []
    for path in sidecars:
        content = path.read_bytes()
        updated, tags = supplement_nfo(content, code, data)
        if tags:
            proposed.append((path, updated, tags))
    for path, updated, _ in proposed:
        atomic_write_bytes(path, updated)
    return {path.name: tags for path, _, tags in proposed}


def _build_sidecars(nfo_path, videos, data, tags, *, force):
    groups = {nfo_path: videos}
    for video in videos:
        sidecar = video.with_suffix(".nfo")
        if sidecar != nfo_path and (force or not sidecar.exists()):
            groups.setdefault(sidecar, []).append(video)
    measured = {video: probe_video(video) for video in videos}
    if len(videos) == 1:
        video = videos[0]
        tags.extend(intake_tags(video))
        if (measured[video].get("width") or 0) >= 3840:
            tags.append("4K")
    # Translate once so every NFO for this title gets the same synopsis and shares one request.
    synopsis = localized_synopsis(data)
    # A sidecar shared by multiple files cannot carry any single file's runtime.
    return {
        path: build_nfo(
            data, tags, nfo_path.parent.name, measured[items[0]] if len(items) == 1 else {},
            synopsis=synopsis,
        )
        for path, items in groups.items()
    }


def _sample_urls(samples, alternates):
    for index, sample in enumerate(samples):
        yield sample, alternates[index] if index < len(alternates) else None


def _write_artwork(folder, fetcher, code, data):
    images = []
    samples = data.get("samples", [])
    alternate_samples = data.get("samples_alt", [])
    fanart = folder / "fanart.jpg"
    poster = folder / "poster.jpg"

    has_fanart = download_with_alt(
        fetcher, data.get("cover"), data.get("cover_alt"), fanart
    )
    if has_fanart:
        images.append("fanart")
        thumbnail = folder / "landscape.jpg"
        if not thumbnail.exists():
            atomic_write_bytes(thumbnail, fanart.read_bytes())

    if is_vr(data, code):
        temporary = Path(tempfile.mkdtemp(prefix="jav_"))
        try:
            downloaded = []
            for index, (sample, alternate) in enumerate(_sample_urls(samples, alternate_samples), 1):
                path = temporary / f"s{index:02d}.jpg"
                if download_with_alt(fetcher, sample, alternate, path):
                    downloaded.append(path)

            selected, note = pick_sample_poster(
                fetcher,
                downloaded,
                ps_reference_url(samples, code=code),
                temporary,
            )
            if selected:
                atomic_write_bytes(poster, selected.read_bytes())
                downloaded.remove(selected)
                images.append(f"poster(sample: {note})")
            elif has_fanart and make_poster(fanart, poster):
                images.append(f"poster(crop: {note})")

            if downloaded:
                sample_directory = folder / SAMPLES_DIR
                sample_directory.mkdir(exist_ok=True)
                for index, path in enumerate(downloaded, 1):
                    atomic_write_bytes(
                        sample_directory / f"sample{index}.jpg", path.read_bytes()
                    )
                images.append(f"samples×{len(downloaded)}")
        finally:
            shutil.rmtree(temporary, ignore_errors=True)
    else:
        if has_fanart and make_poster(fanart, poster):
            images.append("poster(crop)")
        if samples:
            sample_directory = folder / SAMPLES_DIR
            sample_directory.mkdir(exist_ok=True)
            downloaded_count = 0
            for index, (sample, alternate) in enumerate(_sample_urls(samples, alternate_samples), 1):
                if download_with_alt(
                    fetcher,
                    sample,
                    alternate,
                    sample_directory / f"sample{index}.jpg",
                ):
                    downloaded_count += 1
            if downloaded_count:
                images.append(f"samples×{downloaded_count}")

    try:
        backdrop_count = publish_sample_backdrops(folder)
        if backdrop_count:
            images.append(f"emby-backdrops×{backdrop_count}")
    except OSError as error:
        print(f"{code}: could not publish Emby backdrops: {error}", flush=True)

    return images


def scrape_one(folder, fetcher, args):
    """Collect metadata for one title and publish its artwork and sidecars."""
    code, tags = normalize(folder.name)
    if not code:
        return "SKIP", "Catalog number not recognized"

    nfo_path = folder / "movie.nfo"
    if nfo_path.exists() and not args.force:
        return "SKIP", "movie.nfo exists; skipped (--force to override)"

    fetcher.begin_movie()

    videos = sorted(
        [
            path
            for path in folder.iterdir()
            if path.is_file() and path.suffix.lower() in VIDEO_EXT
        ],
        key=lambda path: path.name,
    )

    data = collect_metadata(fetcher, code)
    if not data:
        return "MISS", f"{code} was not found in any metadata source"
    source_tag = data["source"]
    payloads = _build_sidecars(nfo_path, videos, data, tags, force=args.force)
    images = _write_artwork(folder, fetcher, code, data)

    # Publish movie.nfo last so a rerun can retry incomplete metadata.
    for path, payload in payloads.items():
        if path != nfo_path:
            atomic_write_bytes(path, payload)
    atomic_write_bytes(nfo_path, payloads[nfo_path])

    detail = f"{code} [{source_tag}] nfo×{len(payloads)}"
    if tags:
        detail += f" tags:{'/'.join(tags)}"
    if images:
        detail += f" images:{','.join(images)}"
    return "OK", detail


def do_scrape(folders, fetcher, args):
    """Run serial scraping and print an aggregate result summary."""
    print(
        f"Intervals: pages {args.page_delay}s / images {args.image_delay}s / "
        f"titles {args.delay}s   Force: {'yes' if args.force else 'no'}"
    )
    print("─" * 92)

    stats = {"OK": 0, "SKIP": 0, "MISS": 0}
    misses = []
    started_at = time.time()

    for index, folder in enumerate(folders, 1):
        try:
            status, message = scrape_one(folder, fetcher, args)
        except KeyboardInterrupt:
            print(
                "\nInterrupted. A rerun will skip completed titles that already have movie.nfo."
            )
            break
        except Exception as error:
            status, message = "MISS", f"Error {type(error).__name__}: {error}"
        finally:
            fetcher.finish_movie()
        stats[status] = stats.get(status, 0) + 1
        if status == "MISS":
            misses.append((folder.name, message))
        icon = {"OK": "✅", "SKIP": "⏭ ", "MISS": "❌"}[status]
        print(
            f"[{index:3}/{len(folders)}] {icon} {folder.name[:40]:<42} {message}",
            flush=True,
        )

    elapsed = time.time() - started_at
    print("─" * 92)
    print(
        f"Complete: succeeded {stats['OK']}  skipped {stats['SKIP']}  "
        f"failed {stats['MISS']}   elapsed {elapsed / 60:.1f} min"
    )
    if fetcher.throttled:
        slowed = {
            bucket: delay
            for bucket, delay in fetcher._delay.items()
            if delay
            > fetcher._base["img" if bucket.endswith(":img") else "page"]
        }
        print(
            f"Rate limiting: HTTP 429 occurred {fetcher.throttled} time(s)"
            + (
                "; slowed "
                + ", ".join(
                    f"{bucket} → {delay:.1f}s" for bucket, delay in slowed.items()
                )
                if slowed
                else ""
            )
        )
    if misses:
        print("\nUnmatched titles (manual review may be needed):")
        for name, message in misses:
            print(f"  · {name:<44} {message}")
