"""Read the publisher and retailer pages for a verified catalog number."""

from datetime import date
import re
from urllib.parse import urlsplit

from .sources import _clean, _same_code

PUBLISHERS = {'IDPC': 'ideapocket.com', 'IPZZ': 'ideapocket.com',
              'SNOS': 's1s1s1.com', 'SSIS': 's1s1s1.com',
              'SONE': 's1s1s1.com'}
STUDIOS = {'ideapocket.com': 'アイデアポケット', 's1s1s1.com': 'エスワン ナンバーワンスタイル'}


def official_url(code):
    match = re.fullmatch(r"([A-Z]+)-(\d{3,})", code, re.I)
    host = PUBLISHERS.get(match[1].upper()) if match else None
    if host:
        return f"https://{host}/works/detail/{(match[1] + match[2]).lower()}"
    return None


def _date(text):
    match = re.search(r'(?<!\d)(\d{4})[/年-](\d{1,2})[/月-](\d{1,2})(?!\d)', text or '')
    if match:
        try:
            return date(*(int(value) for value in match.groups())).isoformat()
        except ValueError:
            pass
    return None


def _digital_url(page, code):
    for href in re.findall(r'<a\b[^>]*\bhref="([^"]+)"', page, re.I):
        url = urlsplit(href)
        if url.scheme != 'https' or url.netloc != 'www.dmm.co.jp':
            continue
        match = re.match(r'^/digital/videoa/-/detail/=/cid=([a-z0-9]+)(?:/|$)', url.path, re.I)
        if match and _same_code(match[1], code):
            return f'https://www.dmm.co.jp/digital/videoa/-/detail/=/cid={match[1]}/'
    return None


def _digital_release(fetcher, url):
    status, page = fetcher.get(url, max_retry=1)
    if status != 200 or not page:
        return None
    for row in re.findall(r'<tr\b[^>]*>(.*?)</tr>', page, re.S):
        cells = re.match(r'\s*<(?:th|td)\b[^>]*>(.*?)</(?:th|td)>\s*<td\b[^>]*>(.*?)</td>', row, re.S)
        if cells and _clean(cells[1]).rstrip('：:').rstrip() in ('配信開始日', '配信日'):
            return _date(_clean(cells[2]))
    return None


def fetch_publisher(fetcher, code, *, include_digital=True):
    url = official_url(code)
    if not url:
        return None
    host = urlsplit(url).hostname
    status, page = fetcher.get(url, max_retry=1)
    if status != 200 or not page:
        return None

    detail = page.partition('class="p-workPage__table"')[2].partition('class="p-workPage__side"')[0]
    rows = dict(re.findall(
        r'<div class="th">\s*([^<]+?)\s*</div>(.*?)(?=<div class="th">|$)', detail, re.S
    ))
    catalog_numbers = re.findall(r'</span>\s*([A-Za-z0-9-]+)\s*</p>', rows.get('品番', ''), re.I)
    if not any(_same_code(number, code) for number in catalog_numbers):
        return None

    def names(row):
        return [_clean(value) for value in re.findall(r'<a\b[^>]*>(.*?)</a>', rows.get(row, ''), re.S)]

    data = {'website': url, 'studio': STUDIOS[host], 'source': 'publisher', 'code': code}
    data['disc_release'] = _date(_clean(rows.get('発売日', '')))
    data['digital_release'] = _date(_clean(rows.get('配信開始日') or rows.get('配信日', '')))
    retailer = _digital_url(page, code)
    if include_digital and retailer and not data['digital_release']:
        data['digital_release'] = _digital_release(fetcher, retailer)
    disc_runtime = re.search(r'</span>\s*(\d+)\s*分', rows.get('収録時間', ''))
    if disc_runtime and int(disc_runtime[1]) > 0:
        data['runtime'] = int(disc_runtime[1])
    data['series'] = next(iter(names('シリーズ')), None)
    data['label'] = next(iter(names('レーベル')), None)
    data['director'] = next(iter(names('監督')), None)
    data['genres'] = names('ジャンル')

    work = page.split('class="p-workPage__side"', 1)[0]
    title = re.search(r'<h2 class="p-workPage__title"[^>]*>(.*?)</h2>', work, re.S)
    if not title or not _clean(title[1]):
        return None
    data['title'] = _clean(title[1])
    synopsis = re.search(r'<p class="p-workPage__text"[^>]*>(.*?)</p>', work, re.S)
    if synopsis and _clean(synopsis[1]):
        data['plot'] = _clean(re.sub(r'<br\s*/?>', '\n', synopsis[1], flags=re.I))
    slider = work.partition('<div class="p-slider -work">')[2].partition('</section>')[0]
    images = list(dict.fromkeys(re.findall(
        r'data-src="(https://cdn\.up-timely\.com/image/\d+/content/\d+/[\w-]+\.jpe?g)"', slider, re.I
    )))
    if images:
        data['cover'] = images[0]
        data['samples'] = images[1:]

    cast = re.findall(
        rf'href="https://{re.escape(host)}/(?:index\.php/)?actress/detail/(\d+)"[^>]*>(.*?)</a>',
        rows.get('女優', ''), re.S,
    )
    actors = {}
    for profile_id, raw_name in cast:
        name = _clean(raw_name)
        if not name or name in actors:
            continue
        actor = actors[name] = {'name': name}
        status, profile = fetcher.get(f'https://{host}/actress/detail/{profile_id}', max_retry=1)
        if status != 200 or not profile:
            continue
        heading = re.search(r'<h2\b[^>]*>(.*?)</h2>', profile, re.S)
        if not heading or _clean(heading[1]) != name:
            continue
        image = re.search(rf'data-src="(https://cdn\.up-timely\.com/image/\d+/actress_main/{profile_id}/[\w-]+\.jpe?g)"',
                          profile, re.I)
        if image:
            actor['thumb'] = image[1]
    data['actors'] = list(actors.values())
    return data


def merge_publisher(data, publisher):
    if not publisher:
        return data
    if not data:
        return publisher

    for field in ('website', 'title', 'plot', 'studio', 'series', 'label', 'director',
                  'disc_release', 'digital_release'):
        if publisher.get(field):
            data[field] = publisher[field]
    if not data.get('runtime') and publisher.get('runtime'):
        data['runtime'] = publisher['runtime']
    data['genres'] = list(dict.fromkeys(publisher.get('genres', []) + data.get('genres', [])))
    actors = {actor['name']: dict(actor) for actor in data.get('actors', []) if actor.get('name')}
    for actor in publisher.get('actors', []):
        actors.setdefault(actor['name'], {}).update(actor)
    data['actors'] = list(actors.values())
    if publisher.get('cover'):
        if not data.get('cover'):
            data['cover'] = publisher['cover']
        elif not data.get('cover_alt'):
            data['cover_alt'] = publisher['cover']
    if not data.get('samples') and publisher.get('samples'):
        data['samples'] = publisher['samples']
    data['source'] += '+publisher'
    return data
