"""Enrich verified catalog matches with official summaries and portraits."""

import re

from .sources import _clean

PUBLISHERS = {'IDPC': 'ideapocket.com', 'IPZZ': 'ideapocket.com',
              'SNOS': 's1s1s1.com', 'SSIS': 's1s1s1.com',
              'SONE': 's1s1s1.com'}


def official_url(code):
    match = re.fullmatch(r"([A-Z]+)-(\d{3,})", code, re.I)
    host = PUBLISHERS.get(match[1].upper()) if match else None
    if host:
        return f"https://{host}/works/detail/{(match[1] + match[2]).lower()}"
    return None


def enrich(fetcher, code, data):
    url = official_url(code)
    if not url:
        return
    host, *_, catalog = url.removeprefix("https://").split("/")
    status, page = fetcher.get(url, max_retry=1)
    if status != 200 or not page or not re.search(rf'</span>\s*{catalog}\s*</p>', page, re.I):
        return
    data['website'] = url
    synopsis = re.search(r'<p class="p-workPage__text"[^>]*>(.*?)</p>', page, re.S)
    if synopsis and _clean(synopsis[1]):
        data['plot'] = _clean(re.sub(r'<br\s*/?>', '\n', synopsis[1], flags=re.I))

    cast = re.findall(rf'href="https://{re.escape(host)}/actress/detail/(\d+)"[^>]*>(.*?)</a>',
                      page.split('class="p-workPage__side"', 1)[0], re.S)
    actors = {actor['name']: actor for actor in data.get('actors', []) if actor.get('name')}
    for profile_id, raw_name in cast:
        name = _clean(raw_name)
        if not name:
            continue
        status, profile = fetcher.get(f'https://{host}/actress/detail/{profile_id}', max_retry=1)
        if status != 200 or not profile:
            continue
        heading = re.search(r'<h2\b[^>]*>(.*?)</h2>', profile, re.S)
        if not heading or _clean(heading[1]) != name:
            continue
        image = re.search(rf'data-src="(https://cdn\.up-timely\.com/image/\d+/actress_main/{profile_id}/[\w-]+\.jpe?g)"',
                          profile, re.I)
        if image:
            actors.setdefault(name, {'name': name})['thumb'] = image[1]
    if actors:
        data['actors'] = list(actors.values())
