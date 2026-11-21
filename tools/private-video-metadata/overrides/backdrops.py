"""Make downloaded samples discoverable by Emby's local backdrop scanner."""

import os
import re
from tempfile import NamedTemporaryFile

_SAMPLE = re.compile(r"sample([1-9][0-9]*)\.jpg")


def publish_sample_backdrops(folder):
    """Publish JPG samples under extrafanart, keeping existing artwork intact."""
    samples = folder / "Samples"
    if samples.is_symlink():
        raise OSError(f"Sample directory is a symlink: {samples}")
    numbered = []
    for path in samples.glob("sample*.jpg"):
        match = _SAMPLE.fullmatch(path.name)
        if match and path.is_file() and not path.is_symlink():
            numbered.append((int(match[1]), path))
    if not numbered:
        return 0

    gallery = folder / "extrafanart"
    if gallery.is_symlink():
        raise OSError(f"Backdrop directory is a symlink: {gallery}")
    gallery.mkdir(exist_ok=True)
    count = 0
    for index, sample in sorted(numbered):
        target = gallery / f"fanart{index}.jpg"
        contents = sample.read_bytes()
        if not os.path.lexists(target):
            with NamedTemporaryFile(dir=gallery, prefix=".backdrop-") as temp:
                temp.write(contents)
                temp.flush()
                try:
                    os.link(temp.name, target)
                except FileExistsError:
                    pass
        if target.is_symlink() or not target.is_file() or target.read_bytes() != contents:
            raise FileExistsError(f"Backdrop conflict; kept existing file: {target}")
        count += 1
    return count
