"""Simplify Chinese metadata and translate Japanese display text locally."""

import re
from functools import lru_cache
from pathlib import Path
from xml.etree import ElementTree as ET

from opencc import OpenCC

MODEL = Path('/opt/translation-model')
KANA = re.compile(r'[\u3040-\u30ff\uff66-\uff9d]')
HAN = re.compile(r'[\u3400-\u9fff]')
CONVERTER = OpenCC('t2s')
BROKEN_TEXT = re.compile(r'[�⁇]|[?？]{2,}')
KANJI_PAIRS = re.compile(r'(?=([\u3400-\u9fff]{2}))')


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
        translator, tokenizer = load_model()
        tokens = ['jpn_Jpan', *tokenizer.encode(text, out_type=str), '</s>']
        result = translator.translate_batch([tokens], target_prefix=[['zho_Hans']],
                                            beam_size=4, max_decoding_length=512)
        translated = tokenizer.decode(result[0].hypotheses[0][1:]).strip()
        if not HAN.search(translated) or KANA.search(translated) or BROKEN_TEXT.search(translated):
            return text
        text = translated
    return CONVERTER.convert(text)


def synopsis_chinese(text):
    if not KANA.search(text):
        return chinese(text)
    sentences = re.split(r'(?<=[。！？])(?![。！？])\s*|\n+', text)
    translated = [chinese(sentence.strip()) for sentence in sentences if sentence.strip()]
    if any(KANA.search(sentence) or BROKEN_TEXT.search(sentence) for sentence in translated):
        return text
    return ''.join(translated)


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


def localize(root):
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
        names = '|'.join(re.escape(name) for name in sorted(actors, key=lambda name: (-len(name), name)))
        parts = re.split(f'({names})', text) if names else [text]
        translated = ''.join(
            part if part in actors or not part.strip()
            else part.replace(part.strip(), title_chinese(part.strip()), 1)
            for part in parts
        )
        if BROKEN_TEXT.search(translated) or ('チアガール' in text and '啦啦' not in translated):
            translated = text
        title.text = ((prefix if code else '') + series_title + translated).strip()
    for field in ('plot', 'outline', 'tagline', 'genre', 'tag', 'set/name'):
        seen = set()
        for element in root.findall(field):
            if element.text:
                if field in ('plot', 'outline'):
                    element.text = synopsis_chinese(element.text)
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
