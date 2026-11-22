"""Simplify Chinese metadata and translate Japanese display text."""

import json
import logging
import os
import re
from functools import lru_cache
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import Request, urlopen
from xml.etree import ElementTree as ET

from opencc import OpenCC

MODEL = Path('/opt/translation-model')
LOGGER = logging.getLogger(__name__)
KANA = re.compile(r'[\u3040-\u30ff\uff66-\uff9d]')
HAN = re.compile(r'[\u3400-\u9fff]')
CONVERTER = OpenCC('t2s')
BROKEN_TEXT = re.compile(r'[�⁇]|[?？]{2,}')
SYNOPSIS_SENTENCES = re.compile(r'(?<=[。！？!?])(?![。！？!?])|(\n+)')
CLAUSE_PUNCTUATION = re.compile(r'[、，,；;：:]\s*')
CLAUSE_END = re.compile(
    r'てきた|したあと|した後|していた|ていた|ている|けれども|けれど|ながら|として|'
    r'ので|から|[くぐるた]と|して|した|が'
)
CLAUSE_START = re.compile(r'(?=ある日|その日|その後|それから|次は)')
SCRIPT_BOUNDARY = re.compile(r'(?<=[ぁ-ゖ])(?=[\u3400-\u9fffァ-ヺ])|(?<=[\u3400-\u9fffァ-ヺ])(?=[A-Za-z])')
CLAUSE_JOIN = re.compile(r'[、，,；;：:。.!！?？]$')
REPEATED_PHRASE = re.compile(r'([\u3400-\u9fff]{2,12})(?:[、，,]\s*\1){3}')
DECLINED_TRANSLATION = re.compile(r'^(?:很?抱歉|对不起|我(?:无法|不能)(?:翻译|提供)|由于内容)')
MAX_BRIDGE_RESPONSE = 32_000
MIN_CLAUSE_LENGTH = 6
MAX_CLAUSE_SPLITS = 3
KANJI_PAIRS = re.compile(r'(?=([\u3400-\u9fff]{2}))')
GENRE_NAMES = {
    'アクメ・オーガズム': '高潮',
    'おもちゃ': '玩具',
    'スレンダー': '苗条',
    'パイズリ': '乳交',
    'ブルーレイ': '蓝光',
    'blu-ray（ブルーレイ）': '蓝光',
    '単体作品': '单体作品',
}
TITLE_TERMS = {
    'チアガール': ('啦啦',),
    'パイズリ': ('乳交',),
    '手コキ': ('手淫', '用手'),
}


@lru_cache(maxsize=1)
def load_model():
    import ctranslate2
    import sentencepiece

    translator = ctranslate2.Translator(str(MODEL), device='cpu',
                                        compute_type='int8', intra_threads=2)
    tokenizer = sentencepiece.SentencePieceProcessor(model_file=str(MODEL / 'sentencepiece.bpe.model'))
    return translator, tokenizer


@lru_cache(maxsize=256)
def chinese(text):
    if KANA.search(text):
        try:
            translator, tokenizer = load_model()
            tokens = ['jpn_Jpan', *tokenizer.encode(text, out_type=str), '</s>']
            result = translator.translate_batch([tokens], target_prefix=[['zho_Hans']],
                                                beam_size=4, max_decoding_length=512)
            translated = tokenizer.decode(result[0].hypotheses[0][1:]).strip()
        except (ImportError, OSError, RuntimeError, ValueError):
            return text
        if not HAN.search(translated) or KANA.search(translated) or BROKEN_TEXT.search(translated):
            return text
        text = translated
    return CONVERTER.convert(text)


def split_clause(text):
    for patterns in ((CLAUSE_PUNCTUATION,), (CLAUSE_END, CLAUSE_START), (SCRIPT_BOUNDARY,)):
        cuts = [match.end() for pattern in patterns for match in pattern.finditer(text)
                if MIN_CLAUSE_LENGTH <= match.end() <= len(text) - MIN_CLAUSE_LENGTH]
        if cuts:
            cut = min(cuts, key=lambda index: abs(len(text) - 2 * index))
            return text[:cut].rstrip(), text[cut:].lstrip()
    return None


def valid_synopsis_text(text):
    return bool(text and not KANA.search(text) and not BROKEN_TEXT.search(text)
                and not REPEATED_PHRASE.search(text))


def translate_synopsis_sentence(text, splits=MAX_CLAUSE_SPLITS):
    translated = chinese(text)
    if valid_synopsis_text(translated):
        return translated
    parts = split_clause(text) if splits else None
    if not parts:
        return None
    left = translate_synopsis_sentence(parts[0], splits - 1)
    if left is None:
        return None
    right = translate_synopsis_sentence(parts[1], splits - 1)
    if right is None:
        return None
    separator = '' if CLAUSE_JOIN.search(left) else '，'
    return left + separator + right


def translate_synopsis_remote(text):
    endpoint = os.getenv('PRIVATE_METADATA_TRANSLATION_URL')
    token_file = os.getenv('PRIVATE_METADATA_TRANSLATION_TOKEN_FILE')
    if not endpoint or not token_file or len(text.strip()) > 4000:
        return None
    try:
        token = Path(token_file).read_text(encoding='utf-8').strip()
        if not token:
            raise ValueError('Empty translation token')
        # The source is known to be Japanese. The bridge's "zh" target bypasses its
        # Chinese language heuristic, which can misclassify Japanese with many kanji.
        payload = json.dumps({'target_language': 'zh', 'texts': [text]}, ensure_ascii=False)
        request = Request(endpoint, data=payload.encode('utf-8'), headers={
            'Authorization': f'Bearer {token}',
            'Content-Type': 'application/json',
        })
        with urlopen(request, timeout=25) as response:
            body = response.read(MAX_BRIDGE_RESPONSE + 1)
        if len(body) > MAX_BRIDGE_RESPONSE:
            raise ValueError('Oversized translation response')
        data = json.loads(body)
    except HTTPError as error:
        LOGGER.warning('Synopsis translation fallback returned HTTP %s.', error.code)
        return None
    except (OSError, ValueError):
        LOGGER.warning('Synopsis translation fallback unavailable.')
        return None

    values = data.get('translations') if isinstance(data, dict) else None
    if isinstance(values, list) and len(values) == 1 and isinstance(values[0], str):
        translated = CONVERTER.convert(values[0].strip())
        if (valid_synopsis_text(translated) and HAN.search(translated)
                and not DECLINED_TRANSLATION.match(translated) and len(translated) * 3 >= len(text.strip())):
            return translated
    LOGGER.warning('Synopsis translation fallback returned unusable text.')
    return None


def synopsis_chinese(text, *, allow_partial=True):
    if not KANA.search(text):
        return chinese(text)
    translated = []
    needs_retry = False
    for part in filter(None, SYNOPSIS_SENTENCES.split(text)):
        if part.isspace():
            translated.append(part)
            continue
        sentence = part.strip()
        direct = translate_synopsis_sentence(sentence, splits=0)
        if direct is None:
            needs_retry = True
            direct = translate_synopsis_sentence(sentence)
        translated.append(direct)
    if needs_retry and allow_partial:
        # Re-translate the source as a whole so DeepSeek can fix errors in even the
        # locally accepted sentences and use the surrounding sentences for context.
        remote = translate_synopsis_remote(text)
        if remote:
            return remote
    if not allow_partial and None in translated:
        return text
    # The NFO keeps the complete source in originalplot, including failed sentences.
    return ''.join(filter(None, translated)).strip() or text


def numbered_series(title, series):
    if series and series.isascii():
        pattern = r'\s+'.join(re.escape(word) for word in series.split())
        if pattern:
            return re.match(rf'{pattern}\s+(?:(?:vol\.?|#)\s*)?\d+\b', title, re.I | re.ASCII)
    return None


def translate_title_part(text):
    # Expand common media terms that the local model otherwise transliterates.
    expanded = text.replace('ライヴ', 'ライブ公演').replace('グラビア', '写真撮影')
    variants = [expanded]
    if not expanded.endswith(('。', '！', '？', '!', '?')):
        variants.append(expanded + '。')
    if expanded != text:
        variants.reverse()
    anchors = set(KANJI_PAIRS.findall(CONVERTER.convert(expanded)))
    numbers = re.findall(r'\d+', text)
    for variant in variants:
        localized = chinese(variant)
        if KANA.search(localized) or BROKEN_TEXT.search(localized):
            continue
        if re.findall(r'\d+', localized) != numbers:
            continue
        if len(anchors) >= 2 and not anchors.intersection(KANJI_PAIRS.findall(localized)):
            continue
        if any(term in text and not any(word in localized for word in words)
               for term, words in TITLE_TERMS.items()):
            continue
        return localized.rstrip('.。') if variant != expanded else localized
    return text


def title_chinese(text):
    if not KANA.search(text):
        return chinese(text)
    # Japanese titles use spaces between phrases, often with no sentence punctuation.
    parts = re.split(
        r'(?<=[\u3040-\u9fff])[ \u3000]+(?=[\u3040-\u9fff])|(?<=[。！？])(?![。！？])\s*',
        text,
    )
    translated = [translate_title_part(part.strip()) for part in parts if part.strip()]
    if any(KANA.search(part) or BROKEN_TEXT.search(part) for part in translated):
        return text
    return ' '.join(translated)


def localize(root, *, catalog_title=False, plot_localized=False):
    code = (root.findtext('num') or root.findtext('sorttitle') or '').strip()
    actors = {name.text for name in root.findall('actor/name') if name.text}
    title = root.find('title')
    if title is not None and title.text:
        prefix = code + ' '
        text = title.text.removeprefix(prefix) if code else title.text
        original = (root.findtext('originaltitle') or text).removeprefix(prefix)
        series = root.findtext('series') or root.findtext('set/name')
        original_number = numbered_series(original, series)
        series_title = ''
        if original_number:
            series_title = original_number[0] + ' '
            display_number = numbered_series(text, series)
            if display_number and display_number[0].casefold() == original_number[0].casefold():
                text = text[display_number.end():].lstrip()
            else:
                text = original[original_number.end():].lstrip()
                catalog_title = False
        if catalog_title:
            translated = CONVERTER.convert(text)
        else:
            names = '|'.join(re.escape(name) for name in sorted(actors, key=lambda name: (-len(name), name)))
            parts = re.split(f'({names})', text) if names else [text]
            localized = [
                part if part in actors or not part.strip()
                else part.replace(part.strip(), title_chinese(part.strip()), 1)
                for part in parts
            ]
            if any(KANA.search(part) for part in localized if part not in actors):
                translated = text
            else:
                translated = ''.join(localized)
        title.text = ((prefix if code else '') + series_title + translated).strip()
    for field in ('plot', 'outline', 'tagline', 'genre', 'tag', 'set/name'):
        if field == 'plot' and plot_localized:
            continue
        seen = set()
        for element in root.findall(field):
            if element.text:
                if field in ('plot', 'outline'):
                    allow_partial = field == 'plot' and root.findtext('originalplot') == element.text
                    element.text = synopsis_chinese(element.text, allow_partial=allow_partial)
                elif field == 'genre' or (field == 'tag' and element.text not in actors):
                    value = element.text.strip()
                    element.text = GENRE_NAMES.get(value.casefold()) or (
                        value if KANA.search(value) else CONVERTER.convert(value)
                    )
                elif field != 'tag' or element.text not in actors:
                    element.text = chinese(element.text)
                if field in ('genre', 'tag'):
                    key = element.text.casefold()
                    if key in seen:
                        root.remove(element)
                    seen.add(key)

if __name__ == '__main__':
    import sys

    from javlibrarian.filesystem import atomic_write_bytes

    paths = [Path(arg) for arg in sys.argv[1:]]
    documents = []
    for path in paths:
        root = ET.parse(path).getroot()
        localize(root)
        documents.append((path, ET.tostring(root, encoding='utf-8', xml_declaration=True)))
    for path, document in documents:
        atomic_write_bytes(path, document)
