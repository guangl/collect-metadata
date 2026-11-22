import json
import os
import subprocess
import sys
import time
from pathlib import Path
from threading import Event
from urllib.parse import urlencode
from urllib.request import Request, urlopen

from watchdog.events import FileSystemEventHandler
from watchdog.observers.inotify import InotifyObserver

from javlibrarian.catalog import VIDEO_EXT, normalize
from javlibrarian.intake import prepare
from portraits import update_portraits

ROOT = Path('/private')
EMBY_ROOT = '/media/.private-video/'
API = 'http://media-emby:8096/emby/'
RETRY_INTERVAL = 900
SETTLE_SECONDS = 5


class NewVideos(FileSystemEventHandler):
    def __init__(self, wake):
        self.wake = wake

    def on_any_event(self, event):
        path = Path(getattr(event, 'dest_path', '') or event.src_path)
        if event.is_directory:
            if event.event_type in {'created', 'moved'} and path.parent == ROOT:
                self.wake.set()
        elif event.event_type in {'closed', 'moved'} and path.suffix.lower() in VIDEO_EXT:
            if ROOT in (path.parent, path.parent.parent) and not path.name.startswith('.'):
                self.wake.set()

def emby(path, post=False):
    token = Path('/run/secrets/emby-key').read_text().strip()
    request = Request(API + path, data=b'{}' if post else None,
                      headers={'X-Emby-Token': token, 'Content-Type': 'application/json'})
    with urlopen(request, timeout=20) as response:
        data = response.read()
        return json.loads(data) if data else None

def candidates():
    for folder in sorted(ROOT.iterdir()):
        if folder.name.startswith('.') or folder.is_symlink() or not folder.is_dir():
            continue
        if not normalize(folder.name)[0]:
            continue
        entries = list(folder.iterdir())
        if any(p.is_symlink() or p.suffix.lower() == '.nfo' or p.name in
               {'poster.jpg', 'fanart.jpg', 'Samples'} for p in entries):
            continue
        videos = [p for p in entries if p.is_file() and not p.name.startswith('.')
                  and p.suffix.lower() in VIDEO_EXT]
        snapshot = {}
        for video in videos:
            try:
                info = video.stat()
            except FileNotFoundError:
                break
            if not info.st_size:
                break
            snapshot[video.name] = (info.st_size, info.st_mtime_ns)
        else:
            if snapshot:
                yield folder, snapshot

def update_emby(pending):
    folders = emby('Library/VirtualFolders')
    library = next(f for f in folders if f['Locations'] == [EMBY_ROOT.rstrip('/')])
    query = urlencode({'ParentId':library['ItemId'], 'Recursive':'true',
                       'IncludeItemTypes':'Movie', 'Fields':'Path,People'})
    items = emby('Items?' + query)['Items']
    remaining = set(pending)
    for folder in pending:
        prefix = EMBY_ROOT + folder + '/'
        matching = [item for item in items if (item.get('Path') or '').startswith(prefix)]
        for item in matching:
            emby('Items/' + item['Id'] + '/Refresh?MetadataRefreshMode=FullRefresh&ImageRefreshMode=FullRefresh&ReplaceAllMetadata=true&ReplaceAllImages=true', post=True)
        if matching:
            remaining.remove(folder)
    if remaining:
        emby('Items/' + library['ItemId'] + '/Refresh?Recursive=true', post=True)
    return remaining

def fill_cast(pending):
    library = next(f for f in emby('Library/VirtualFolders')
                   if f['Locations'] == [EMBY_ROOT.rstrip('/')])
    query = urlencode({'ParentId': library['ItemId'], 'Recursive': 'true',
                       'IncludeItemTypes': 'Movie', 'Fields': 'Path,People'})
    for item in emby('Items?' + query)['Items']:
        media = item.get('Path') or ''
        if not media.startswith(EMBY_ROOT):
            continue
        folder = Path(media).parent.name
        if folder not in pending:
            continue
        try:
            if update_portraits(ROOT / folder, item, emby, API):
                pending.remove(folder)
        except (OSError, ValueError) as error:
            print(f'Portrait lookup deferred: {folder}: {error}', flush=True)

def loose_videos():
    entries = list(ROOT.iterdir())
    for video in entries:
        if (video.name.startswith('.') or video.suffix.lower() not in VIDEO_EXT
                or video.is_symlink() or not video.is_file() or not normalize(video.stem)[0]):
            continue
        # A folder or companion file with the same stem needs manual review.
        if any(other != video and (other.name == video.stem
                                   or other.name.startswith(video.stem + '.'))
               for other in entries):
            continue
        try:
            stat = video.stat()
        except FileNotFoundError:
            continue
        if stat.st_size:
            yield video, (stat.st_ino, stat.st_size, stat.st_mtime_ns)


def scrape(wake, reason):
    before = dict(candidates())
    loose_before = dict(loose_videos())
    if not before and not loose_before:
        return set()

    time.sleep(SETTLE_SECONDS)
    after = dict(candidates())
    loose_after = dict(loose_videos())
    stable_folders = [(folder, snapshot) for folder, snapshot in after.items()
                      if snapshot == before.get(folder)]
    if len(stable_folders) != len(after) or loose_after != loose_before:
        wake.set()
    stable_videos = [(video, ()) for video, snapshot in loose_after.items()
                     if snapshot == loose_before.get(video)]
    folders = []
    for source, files in [*stable_folders, *stable_videos]:
        try:
            folder = prepare(source, files)
        except (OSError, ValueError) as error:
            print(f'Intake deferred for {source.name}: {error}', flush=True)
            continue
        if folder != source:
            print(f'Organized {source.name} into {folder.name}.', flush=True)
        folders.append(folder)
    if not folders:
        return set()

    print(f'{reason}: starting metadata lookup for {len(folders)} title folder(s).')
    command = [sys.executable, '-m', 'javlibrarian', 'scrape', '--dir', str(ROOT)]
    for folder in folders:
        command.extend(('--only', folder.name))
    subprocess.run(command, check=True)
    return {folder.name for folder in folders if (folder / 'movie.nfo').is_file()}


def main():
    os.umask(0o077)
    wake = Event()
    observer = InotifyObserver()
    observer.schedule(NewVideos(wake), str(ROOT), recursive=True)
    observer.start()
    pending = set()
    cast_pending = set()
    next_scan = time.monotonic()
    print('Watching private library for new videos. Fallback interval: 15 minutes.')
    try:
        while True:
            delay = max(0, next_scan - time.monotonic())
            changed = wake.wait(min(delay, 15) if pending or cast_pending else delay)
            try:
                if changed or time.monotonic() >= next_scan:
                    wake.clear()
                    if not changed:
                        next_scan = time.monotonic() + RETRY_INTERVAL
                    added = scrape(wake, 'File event' if changed else 'Fallback scan')
                    pending.update(added)
                    cast_pending.update(added)
                if pending:
                    pending = update_emby(pending)
                if cast_pending:
                    fill_cast(cast_pending)
            except Exception as error:
                print(f'Private library cycle failed: {type(error).__name__}: {error}')
    finally:
        observer.stop()
        observer.join()


if __name__ == '__main__':
    main()
