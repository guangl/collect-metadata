"""Prepare incoming single-video titles for the NAS watcher."""

import os

from .catalog import TAG_RULES, VIDEO_EXT, normalize
from .filesystem import rename_folder, rename_path, target_conflicts
from .rename import plan_sidecar_moves
from .sources import _same_code


TAG_ATTRIBUTE = "user.javlibrarian.catalog-tags"
KNOWN_TAGS = {tag for _, tags in TAG_RULES for tag in tags}


def intake_tags(video):
    """Read filename tags retained on the video through renames and retries."""
    try:
        value = os.getxattr(video, TAG_ATTRIBUTE).decode("utf-8")
    except (AttributeError, OSError, UnicodeError):
        return []
    return [tag for tag in value.splitlines() if tag in KNOWN_TAGS]


def prepare(source, files=()):
    """Use the catalog number for an incoming video and its title folder."""
    loose = source.is_file()
    directory = source.parent
    name = source.stem if loose else source.name
    code, tags = normalize(name)
    if not code:
        raise ValueError("Catalog number missing")
    if not loose and len(files) != 1:
        raise ValueError("Multiple videos would share the same name")
    video = source if loose else source / next(iter(files))
    if not video.is_file() or video.is_symlink():
        raise ValueError("Incoming video is missing or is a symlink")
    video_code, video_tags = normalize(video.stem)
    if video_code and not _same_code(video_code, code):
        raise ValueError(f"Video catalog number {video_code} differs from {code}")

    target = directory / code
    if target_conflicts(source, target):
        raise FileExistsError(f"Catalog folder already exists: {code}")
    for sibling in directory.iterdir():
        if sibling == source or sibling.name.startswith("."):
            continue
        if sibling.is_dir():
            other, _ = normalize(sibling.name)
        elif sibling.suffix.lower() in VIDEO_EXT:
            other, _ = normalize(sibling.stem)
        else:
            continue
        if _same_code(other, code):
            raise FileExistsError(f"Another input uses catalog number {code}: {sibling.name}")

    new_name = f"{code}{video.suffix.lower()}"
    if loose:
        if plan_sidecar_moves(video, new_name):
            raise ValueError("Move the video and its sidecars into a title folder first")
        _remember_tags(video, [*tags, *video_tags])
        target.mkdir(mode=0o700)
        try:
            video.rename(target / new_name)
        except OSError:
            target.rmdir()
            raise
        return target

    moves = plan_sidecar_moves(video, new_name)
    moves.insert(0, (video, source / new_name))
    for origin, destination in moves:
        if origin.is_symlink() or target_conflicts(origin, destination):
            raise FileExistsError(f"Cannot safely rename to {destination.name}")
    _remember_tags(video, [*tags, *video_tags])
    renamed = []
    try:
        for origin, destination in moves:
            if origin.name != destination.name:
                rename_path(origin, destination.name)
                renamed.append((origin, destination))
        ok, message = rename_folder(source, code)
        if not ok:
            raise OSError(message)
    except OSError:
        for origin, destination in reversed(renamed):
            rename_path(destination, origin.name)
        raise
    return target


def _remember_tags(video, tags):
    if tags:
        stored = dict.fromkeys([*intake_tags(video), *tags])
        os.setxattr(video, TAG_ATTRIBUTE, "\n".join(stored).encode("utf-8"))
