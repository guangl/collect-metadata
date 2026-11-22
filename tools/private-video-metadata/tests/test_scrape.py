from contextlib import nullcontext
import json
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
from types import SimpleNamespace
from unittest import TestCase, main
from unittest.mock import Mock, patch
from xml.etree import ElementTree as ET

from javlibrarian import localize, scrape
from javlibrarian.intake import intake_tags, prepare
from javlibrarian.nfo import build_nfo
from javlibrarian.publisher import fetch_publisher
from javlibrarian.sources import JAVBUS, JAVDB, R18


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
    def test_incoming_video_and_generated_sidecar_use_catalog_number(self):
        with TemporaryDirectory() as temp:
            source = Path(temp, 's169bbs.com@SNOS-384-UC_[4K].MKV')
            source.write_bytes(b'video')
            stat = source.stat()
            folder = prepare(source)
            video = folder / 'SNOS-384.mkv'
            self.assertEqual(folder.name, 'SNOS-384')
            self.assertEqual(video.stat().st_ino, stat.st_ino)
            self.assertEqual(intake_tags(video), ['无码破解', '中文字幕'])
            catalog = {'code': 'SNOS-384', 'title': 'Test', 'source': 'publisher'}
            with (
                patch.object(scrape, 'collect_metadata', return_value=catalog),
                patch.object(scrape, 'probe_video', return_value={'width': 3840, 'height': 2160}),
            ):
                result, _ = scrape.scrape_one(folder, Mock(), SimpleNamespace(force=False))
            self.assertEqual(result, 'OK')
            self.assertEqual({p.name for p in folder.iterdir()}, {
                'SNOS-384.mkv', 'SNOS-384.nfo', 'movie.nfo',
            })
            nfo = ET.parse(folder / 'SNOS-384.nfo')
            self.assertEqual(nfo.findtext('num'), 'SNOS-384')
            self.assertEqual({e.text for e in nfo.findall('tag')}, {'无码破解', '中文字幕', '4K', 'SNOS'})

    def test_incoming_folder_renames_video_and_matching_subtitles(self):
        with TemporaryDirectory() as temp:
            folder = Path(temp, 'site.com@IDPC-032_[4K]')
            folder.mkdir()
            video = folder / 'some label.mkv'
            video.write_bytes(b'video')
            video.with_suffix('.zh.srt').write_bytes(b'subtitles')
            target = prepare(folder, [video.name])
            self.assertEqual(target.name, 'IDPC-032')
            self.assertEqual((target / 'IDPC-032.mkv').read_bytes(), b'video')
            self.assertEqual((target / 'IDPC-032.zh.srt').read_bytes(), b'subtitles')

    def test_collisions_and_multi_video_folders_keep_the_incoming_paths(self):
        with TemporaryDirectory() as temp:
            root = Path(temp)
            left = root / 'site.com@SNOS-384_[4K].mkv'
            right = root / 'SNOS-384.mp4'
            left.touch()
            right.touch()
            with self.assertRaises(FileExistsError):
                prepare(left)
            self.assertEqual(set(root.iterdir()), {left, right})
            folder = root / 'IDPC-032_[4K]'
            folder.mkdir()
            names = ['part1.mkv', 'part2.mkv']
            for name in names:
                (folder / name).touch()
            with self.assertRaises(ValueError):
                prepare(folder, names)
            self.assertEqual(sorted(p.name for p in folder.iterdir()), names)

    def test_catalog_chinese_wins_per_field_and_short_tags_skip_the_model(self):
        search = f'{JAVDB}/search?q=IDPC-032&f=all'
        primary = f'{JAVDB}/v/matching?locale=zh'
        bus = f'{JAVBUS}/IDPC-032'
        fetcher = FakeFetcher({
            search: '<a href="/v/matching"><strong>IDPC-032</strong></a>',
            primary: '''
                <h2 class="title"><strong>IDPC-032</strong>
                  <strong class="current-title">青春的中文標題</strong>
                  <span class="origin-title">青春のタイトル</span>
                </h2>
                <div><strong>簡介:</strong><span class="value">這是中文簡介。</span></div>
            ''',
            bus: '''
                <a class="bigImage" href="/cover.jpg"></a><h3>青春のタイトル</h3>
                <div class="col-md-3 info">
                  <p><span class="header">識別碼:</span> IDPC-032</p>
                  <span class="genre"><a href="/genre/1">苗條</a></span>
                </div>
            ''',
            OFFICIAL: PAGE.replace("FIRST IMPRESSION 197 ", "")
                          .replace('<a>FIRST IMPRESSION</a>', '') + """
                <meta name="description" content="通用網站文案">
            """,
        })
        data = scrape.collect_metadata(fetcher, 'IDPC-032', include_digital=False)
        data["genres"] += ["スレンダー", "アクメ・オーガズム", "未登録の分類"]
        with patch.object(localize, 'load_model', side_effect=AssertionError('catalog text exists')):
            nfo = ET.fromstring(build_nfo(data, [], 'IDPC-032'))
        self.assertEqual(nfo.findtext('title'), 'IDPC-032 青春的中文标题')
        self.assertEqual(nfo.findtext('originaltitle'), '青春のタイトル')
        self.assertEqual(nfo.findtext('plot'), '这是中文简介。')
        self.assertEqual(nfo.findtext('originalplot'), '最初の文章。次の文章。')
        self.assertEqual([e.text for e in nfo.findall('genre')], ['苗条', '高潮', '未登録の分類'])

    def test_r18_text_fills_missing_original_plot_only_for_a_verified_id(self):
        bus = f'{JAVBUS}/SNOS-384'
        dvd = f'{R18}/dvd_id=SNOS-384/json'
        combined = f'{R18}/combined=snos00384/json'
        pages = {
            bus: '<a class="bigImage" href="/cover.jpg"></a><h3>カタログタイトル</h3>',
            dvd: '{"content_id":"snos00384"}',
            combined: '{"content_id":"snos00384","title_ja":"別表記","comment_ja":"元の説明です。"}',
        }
        data = scrape.collect_metadata(FakeFetcher(pages), 'SNOS-384')
        self.assertEqual((data['source'], data['title'], data['plot']),
                         ('javbus+r18', 'カタログタイトル', '元の説明です。'))
        pages[combined] = pages[combined].replace('snos00384', 'snos00385')
        self.assertNotIn('plot', scrape.collect_metadata(FakeFetcher(pages), 'SNOS-384'))

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

    def test_new_scrape_retries_a_failed_synopsis_sentence_and_keeps_the_original(self):
        first = '海辺の町で育った。'
        left = 'ある日の練習中に、'
        right = 'ひざにけがをした。'
        original = first + '\n' + left + right
        catalog = {'code': 'IDPC-032', 'title': 'Test', 'source': 'publisher', 'plot': original}
        translations = {
            first: '在海边小镇长大。',
            left + right: '受伤，受伤，受伤，受伤。',
            left: '有一天训练时，',
            right: '膝盖受了伤。',
        }

        with TemporaryDirectory() as temp:
            folder = Path(temp, 'IDPC-032')
            folder.mkdir()
            (folder / 'IDPC-032.mp4').touch()
            with (
                patch.object(scrape, 'collect_metadata', return_value=catalog),
                patch.object(scrape, 'probe_video', return_value={}),
                patch.object(localize, 'chinese', side_effect=lambda text: translations.get(text, text)),
            ):
                result, _ = scrape.scrape_one(folder, Mock(), SimpleNamespace(force=False))
            self.assertEqual(result, 'OK')
            for name in ('IDPC-032.nfo', 'movie.nfo'):
                nfo = ET.parse(folder / name)
                self.assertEqual(nfo.findtext('plot'), '在海边小镇长大。\n有一天训练时，膝盖受了伤。')
                self.assertEqual(nfo.findtext('originalplot'), original)

    def test_synopsis_keeps_complete_translated_sentences_when_a_retry_fails(self):
        sentence = '海辺の町で育った。'
        left = 'ある日の練習中に、'
        unresolved = left + 'ひざにけがをした。'
        translations = {sentence: '在海边小镇长大。', left: '有一天训练时，'}
        with patch.object(localize, 'chinese', side_effect=lambda text: translations.get(text, text)) as model:
            self.assertEqual(localize.synopsis_chinese(sentence + unresolved), '在海边小镇长大。')
            model.assert_any_call(left)
            self.assertEqual(localize.synopsis_chinese(sentence + unresolved, allow_partial=False),
                             sentence + unresolved)
            self.assertEqual(localize.synopsis_chinese(unresolved), unresolved)

    def test_deepseek_retranslates_the_whole_synopsis_once_for_all_sidecars(self):
        first = '海辺の町で育った。'
        left = 'ある日の練習中に、'
        right = 'ひざにけがをした。'
        original = first + left + right
        catalog = {'code': 'IDPC-032', 'title': 'Test', 'source': 'publisher', 'plot': original}
        local = {first: '在山村长大。', left: '有一天训练时，', right: '膝盖受了伤。'}
        response = Mock()
        response.read.return_value = json.dumps({
            'translations': ['在海邊小鎮長大。有一天訓練時，膝蓋受了傷。'],
        }).encode()

        with TemporaryDirectory() as temp:
            folder = Path(temp, 'IDPC-032')
            folder.mkdir()
            (folder / 'IDPC-032.mp4').touch()
            token = Path(temp, 'token')
            token.write_text('example', encoding='utf-8')
            with (
                patch.dict('os.environ', {'PRIVATE_METADATA_TRANSLATION_URL': 'http://bridge:7100/translate',
                                          'PRIVATE_METADATA_TRANSLATION_TOKEN_FILE': str(token)}),
                patch.object(scrape, 'collect_metadata', return_value=catalog),
                patch.object(scrape, 'probe_video', return_value={}),
                patch.object(localize, 'chinese', side_effect=lambda text: local.get(text, text)),
                patch.object(localize, 'urlopen', return_value=nullcontext(response)) as request,
            ):
                result, _ = scrape.scrape_one(folder, Mock(), SimpleNamespace(force=False))
            self.assertEqual(result, 'OK')
            request.assert_called_once()
            sent = request.call_args.args[0]
            self.assertEqual(json.loads(sent.data), {'target_language': 'zh', 'texts': [original]})
            self.assertEqual(sent.get_header('Authorization'), 'Bearer example')
            for name in ('IDPC-032.nfo', 'movie.nfo'):
                nfo = ET.parse(folder / name)
                self.assertEqual(nfo.findtext('plot'), '在海边小镇长大。有一天训练时，膝盖受了伤。')
                self.assertEqual(nfo.findtext('originalplot'), original)

    def test_unusable_deepseek_translation_keeps_the_local_result(self):
        good = '海辺の町で育った。'
        failed = 'ひざにけがをした。'
        response = Mock()
        response.read.return_value = json.dumps({
            'translations': ['很抱歉，我无法翻译这段内容。'],
        }).encode()
        with (
            patch.dict('os.environ', {'PRIVATE_METADATA_TRANSLATION_URL': 'http://bridge:7100/translate',
                                      'PRIVATE_METADATA_TRANSLATION_TOKEN_FILE': '/run/test-token'}),
            patch.object(Path, 'read_text', return_value='example'),
            patch.object(localize, 'chinese', side_effect=lambda text: '在海边小镇长大。' if text == good else text),
            patch.object(localize, 'urlopen', return_value=nullcontext(response)),
            self.assertLogs(localize.LOGGER, level='WARNING'),
        ):
            self.assertEqual(localize.synopsis_chinese(good + failed), '在海边小镇长大。')

    def test_title_translation_retries_and_preserves_the_source_on_failure(self):
        source = '青春を舞台に捧げた歌手'
        with patch.object(localize, 'chinese', side_effect=['完全不相关', '把青春献给舞台的歌手。']):
            self.assertEqual(localize.title_chinese(source), '把青春献给舞台的歌手')
        with patch.object(localize, 'chinese', return_value='完全不相关'):
            self.assertEqual(localize.title_chinese(source), source)
        with patch.object(localize, 'chinese', return_value='最强女主角的皮士利和手可乐'):
            title = '最強ヒロインのパイズリと手コキ'
            self.assertEqual(localize.title_chinese(title), title)

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
