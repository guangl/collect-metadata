import json
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
from types import SimpleNamespace
from unittest import TestCase, main
from unittest.mock import Mock, patch
from xml.etree import ElementTree as ET

from javlibrarian import localize, scrape
from javlibrarian.nfo import build_nfo
from javlibrarian.publisher import fetch_publisher


OFFICIAL = 'https://ideapocket.com/works/detail/idpc032'
RETAILER = 'https://www.dmm.co.jp/digital/videoa/-/detail/=/cid=idpc00032/'
PAGE = '''
<h2 class="p-workPage__title">FIRST IMPRESSION 197 青春のタイトル</h2>
<p class="p-workPage__text">最初の文章。次の文章。</p>
<div class="p-workPage__table">
  <div class="th">女優</div><a href="https://ideapocket.com/actress/detail/42">演者</a>
  <div class="th">発売日</div><a>2026年11月10日</a>
  <div class="th">シリーズ</div><a>FIRST IMPRESSION</a>
  <div class="th">レーベル</div><a>ティッシュ</a>
  <div class="th">品番</div><p><span>DVD</span>IDPC032</p>
  <div class="th">収録時間</div><p><span>DVD</span>150分</p>
</div>
<div class="p-workPage__side">
  <a href="https://www.dmm.co.jp/digital/videoa/-/detail/=/cid=idpc00032/tracking">Digital</a>
</div>
'''


class FakeFetcher:
    def __init__(self, pages):
        self.pages = pages

    def get(self, url, **kwargs):
        return (200, self.pages[url]) if url in self.pages else (403, None)


class PrivateMetadataTest(TestCase):
    def test_publisher_and_retailer_supply_separate_dates(self):
        fetcher = FakeFetcher({
            OFFICIAL: PAGE,
            RETAILER: '<tr><th>配信開始日：</th><td><span>2026/11/06</span></td></tr>',
        })
        data = scrape.collect_metadata(fetcher, 'IDPC-032')
        self.assertEqual(data['source'], 'publisher')
        self.assertEqual(data['studio'], 'アイデアポケット')
        self.assertEqual(data['actors'], [{'name': '演者'}])
        self.assertEqual(data['label'], 'ティッシュ')

        def translate(text):
            self.assertNotIn('197', text)
            return '青春的中文标题' if text == '青春のタイトル' else text

        # A stale localized catalog title must not replace the official series number.
        data['title_cn'] = '第一次印象197年'
        with patch.object(localize, 'chinese', side_effect=translate):
            nfo = ET.fromstring(build_nfo(data, [], 'IDPC-032', {
                'minutes': 154, 'duration_seconds': 9289, 'codec_name': 'h264',
            }))
        self.assertEqual(nfo.findtext('title'), 'IDPC-032 FIRST IMPRESSION 197 青春的中文标题')
        self.assertEqual(nfo.findtext('premiered'), '2026-11-06')
        self.assertEqual(nfo.findtext('releasedate'), '2026-11-10')
        self.assertEqual(nfo.findtext('tagline'), '线上配信 2026-11-06 · 实体发行 2026-11-10')
        self.assertEqual(nfo.findtext('runtime'), '154')
        self.assertEqual(nfo.findtext('fileinfo/streamdetails/video/durationinseconds'), '9289')
        self.assertEqual(nfo.findtext('plot'), '最初の文章。次の文章。')
        self.assertEqual(nfo.findtext('originalplot'), '最初の文章。次の文章。')

    def test_catalog_dates_are_not_copied_into_physical_release(self):
        data = {'code': 'IDPC-032', 'title': 'Test', 'release': '2026-11-06'}
        nfo = ET.fromstring(build_nfo(data, [], 'IDPC-032'))
        self.assertEqual(nfo.findtext('premiered'), '2026-11-06')
        self.assertIsNone(nfo.find('releasedate'))
        self.assertEqual(nfo.findtext('tagline'), '目录日期 2026-11-06')

    def test_title_translation_retries_and_preserves_the_source_on_failure(self):
        source = '青春を舞台に捧げた歌手'
        with patch.object(localize, 'chinese', side_effect=['完全不相关', '把青春献给舞台的歌手。']):
            self.assertEqual(localize.title_chinese(source), '把青春献给舞台的歌手')
        with patch.object(localize, 'chinese', return_value='完全不相关'):
            self.assertEqual(localize.title_chinese(source), source)

    def test_publisher_rejects_a_different_catalog_number(self):
        page = PAGE.replace('IDPC032</p>', 'IDPC033</p>') + '<p><span>DVD</span>IDPC032</p>'
        self.assertIsNone(fetch_publisher(FakeFetcher({OFFICIAL: page}), 'IDPC-032'))

    def test_probe_runtime_and_unreadable_media_fallback(self):
        media = Path('IDPC-032.mp4')
        payload = {'format': {'duration': '9288.639833'}, 'streams': [
            {'codec_name': 'h264', 'width': 1920, 'height': 1080},
        ]}
        with patch.object(subprocess, 'run', return_value=SimpleNamespace(stdout=json.dumps(payload))):
            self.assertEqual(scrape.probe_video(media), {
                'codec_name': 'h264', 'width': 1920, 'height': 1080, 'minutes': 154,
                'duration_seconds': 9289,
            })
        with patch.object(subprocess, 'run', side_effect=FileNotFoundError):
            self.assertEqual(scrape.probe_video(media), {})

    def test_multiple_videos_get_their_own_runtimes(self):
        with TemporaryDirectory() as temp:
            folder = Path(temp, 'IDPC-032')
            folder.mkdir()
            for name in ('part-1.mp4', 'part-2.mp4'):
                (folder / name).touch()
            catalog = {'code': 'IDPC-032', 'title': 'Test', 'runtime': 150, 'source': 'publisher'}
            with (
                patch.object(scrape, 'collect_metadata', return_value=catalog),
                patch.object(scrape, 'probe_video', side_effect=[{'minutes': 70}, {'minutes': 84}]),
            ):
                result, _ = scrape.scrape_one(folder, Mock(), SimpleNamespace(force=False))
            self.assertEqual(result, 'OK')
            for name, minutes in [('part-1.nfo', '70'), ('part-2.nfo', '84'), ('movie.nfo', '150')]:
                self.assertEqual(ET.parse(folder / name).findtext('runtime'), minutes)


if __name__ == '__main__':
    main()
