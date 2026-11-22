#!/usr/bin/env python3
"""HTTP adapter for Sublarr's existing DeepSeek backend."""
import datetime
import hmac
import html
import json
import os
import re
import sqlite3
import threading
import time
from contextlib import closing
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from ipaddress import IPv4Address, IPv4Network
from pathlib import Path
import socket


def plain_text(text):
    return ' '.join(html.unescape(re.sub(r'<[^>]+>', ' ', text)).split())


def is_chinese(text):
    # Emby descriptions may append a Japanese source below the existing Chinese text.
    text = plain_text(text).split('[简介原文]', 1)[0]
    han = len(re.findall(r'[\u3400-\u9fff\uf900-\ufaff]', text))
    kana_or_hangul = len(re.findall(r'[\u3040-\u30ff\u1100-\u11ff\uac00-\ud7af]', text))
    other_letters = sum(char.isalpha() for char in text) - han - kana_or_hangul
    return han > 0 and han * 2 >= other_letters and kana_or_hangul * 3 <= han


class Bridge:
    def __init__(self, backend, budget_path, daily_limit, shape_errors=()):
        self.backend = backend
        self.budget_path = budget_path
        self.daily_limit = daily_limit
        self.shape_errors = shape_errors
        self.lock = threading.Lock()
        with closing(sqlite3.connect(budget_path)) as db:
            db.execute('CREATE TABLE IF NOT EXISTS budget(day TEXT PRIMARY KEY, used INTEGER NOT NULL)')
            db.commit()

    def reserve(self, count):
        day = datetime.datetime.now(datetime.timezone.utc).date().isoformat()
        with closing(sqlite3.connect(self.budget_path)) as db, db:
            if self.daily_limit is not None:
                db.execute('BEGIN IMMEDIATE')
                row = db.execute('SELECT used FROM budget WHERE day = ?', (day,)).fetchone()
                if (row[0] if row else 0) + count > self.daily_limit:
                    return False
            db.execute('INSERT INTO budget VALUES (?, ?) ON CONFLICT(day) DO UPDATE SET used = used + excluded.used', (day, count))
            return True

    def translate_batch(self, texts, target, deadline):
        # Sublarr is line-oriented. A synopsis is one unit even when it has paragraphs.
        try:
            result = self.backend.translate_batch(texts, 'auto', target)
            if result.success and len(result.translated_lines) == len(texts):
                return [line.strip() if isinstance(line, str) and line.strip() else None
                        for line in result.translated_lines]
        except self.shape_errors:
            pass

        # An unaligned response cannot be attached to any item. Split it so one bad
        # synopsis does not discard the rest, and charge each retry to the daily quota.
        if len(texts) == 1:
            return [None]
        midpoint = len(texts) // 2
        translations = []
        for part in (texts[:midpoint], texts[midpoint:]):
            if time.monotonic() < deadline and self.reserve(len(part)):
                translations.extend(self.translate_batch(part, target, deadline))
            else:
                translations.extend([None] * len(part))
        return translations

    def translate(self, data):
        target = data.get('target_language') if isinstance(data, dict) else None
        texts = data.get('texts') if isinstance(data, dict) else None
        if (not isinstance(target, str) or not re.fullmatch(r'[a-z]{2,3}(?:-[A-Za-z0-9]{2,4}){0,2}', target)
                or not isinstance(texts, list) or not 1 <= len(texts) <= 24
                or not all(isinstance(text, str) and 0 < len(text.strip()) <= 4000 for text in texts)):
            return 400, {'error': 'Invalid translation request'}
        pending = [(index, plain_text(text)) for index, text in enumerate(texts)
                   if target != 'zh-CN' or not is_chinese(text)]
        pending = [(index, text) for index, text in pending if text]
        translations = [None] * len(texts)
        if not pending:
            return 200, {'translations': translations}
        if not self.lock.acquire(blocking=False):
            return 503, {'error': 'Translator busy'}
        try:
            if not self.reserve(len(pending)):
                return 429, {'error': 'Daily translation budget exhausted'}
            # The NAS workflow uses Sublarr's `zh` target for Simplified Chinese.
            result = self.translate_batch([text for _, text in pending], 'zh' if target == 'zh-CN' else target,
                                          time.monotonic() + 12)
            for (index, _), translated in zip(pending, result):
                translations[index] = translated
            return 200, {'translations': translations}
        except Exception:
            # Do not log the provider's response, media text, or credentials.
            return 502, {'error': 'Translation failed'}
        finally:
            self.lock.release()


@dataclass
class Client:
    token: bytes
    bridge: Bridge
    app: object


def handler(clients):
    class Handler(BaseHTTPRequestHandler):
        def setup(self):
            super().setup()
            self.connection.settimeout(5)

        def reply(self, status, data):
            body = json.dumps(data, ensure_ascii=False).encode('utf-8')
            self.send_response(status)
            self.send_header('Content-Type', 'application/json; charset=utf-8')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            try:
                self.wfile.write(body)
            except OSError:
                pass  # Collect Metadata may have timed out and preserved the original.

        def do_POST(self):
            if self.path != '/translate':
                return self.reply(404, {'error': 'Not found'})
            supplied = self.headers.get('Authorization', '').encode('utf-8')
            client = next((client for client in clients if hmac.compare_digest(supplied, client.token)), None)
            if client is None:
                return self.reply(401, {'error': 'Unauthorized'})
            try:
                size = int(self.headers.get('Content-Length', '0'))
                if not 0 < size <= 100_000:
                    return self.reply(413, {'error': 'Invalid body size'})
                data = json.loads(self.rfile.read(size))
            except (ValueError, OSError):
                return self.reply(400, {'error': 'Invalid JSON'})
            with client.app.app_context():
                status, result = client.bridge.translate(data)
            return self.reply(status, result)

    return Handler


def translation_bind():
    subnet = os.getenv('METADATA_TRANSLATION_DOCKER_SUBNET')
    if not subnet:
        return os.getenv('METADATA_TRANSLATION_BIND', '127.0.0.1')
    network = IPv4Network(subnet)
    matches = {address for address in socket.gethostbyname_ex(socket.gethostname())[2]
               if IPv4Address(address) in network}
    if len(matches) != 1:
        raise SystemExit('Translation needs exactly one address on its Docker network')
    return matches.pop()


def collect_token():
    path = os.getenv('METADATA_TRANSLATION_COLLECT_ENV_FILE')
    if not path:
        return os.environ['METADATA_TRANSLATION_TOKEN'].strip()
    for line in Path(path).read_text(encoding='utf-8').splitlines():
        name, separator, value = line.partition('=')
        if separator and name.strip() == 'METADATA_TRANSLATION_TOKEN':
            return value.strip().strip("'\x22")
    return ''


def deepseek_key():
    if os.getenv('METADATA_TRANSLATION_USE_SUBLARR_DB') == '1':
        from config import get_settings
        from sqlalchemy import create_engine, text

        engine = create_engine(get_settings().get_database_url(), hide_parameters=True)
        try:
            with engine.connect() as conn:
                return conn.execute(
                    text("SELECT value FROM config_entries WHERE key = :key"),
                    {"key": "backend.deepseek.api_key"},
                ).scalar_one_or_none()
        finally:
            engine.dispose()
    db_path = os.getenv('SUBLARR_DB', '/config/sublarr.db')
    with closing(sqlite3.connect(f'file:{db_path}?mode=ro', uri=True)) as config_db:
        row = config_db.execute("SELECT value FROM config_entries WHERE key='backend.deepseek.api_key'").fetchone()
    return row[0] if row else None


def main():
    from db.models.translation import TranslationEvent
    from extensions import db
    from flask import Flask
    from sqlalchemy import URL
    from translation.concurrency import get_concurrency
    from translation.deepseek import DeepSeekBackend
    from translation.llm_base import LineCountMismatchError, LineMisalignmentError

    metadata_token = collect_token()
    if not metadata_token:
        raise SystemExit('Set METADATA_TRANSLATION_TOKEN')
    collect_db = Path(os.getenv('METADATA_TRANSLATION_BUDGET_DB', '/config/collect-metadata-translation.db')).resolve()
    private_token_file = os.getenv('METADATA_TRANSLATION_PRIVATE_TOKEN_FILE')
    if private_token_file:
        private_token = Path(private_token_file).read_text(encoding='utf-8').strip()
        private_db = Path(os.environ['METADATA_TRANSLATION_PRIVATE_USAGE_DB']).resolve()
        if not private_token or hmac.compare_digest(metadata_token.encode(), private_token.encode()):
            raise SystemExit('Set a distinct token for private synopsis translation')
        if collect_db == private_db or (collect_db.exists() and private_db.exists() and collect_db.samefile(private_db)):
            raise SystemExit('Use separate translation usage databases')
        profiles = [(metadata_token, collect_db, None, DeepSeekBackend),
                    (private_token, private_db, None, DeepSeekBackend)]
    else:
        daily_limit = max(0, int(os.getenv('METADATA_TRANSLATION_DAILY_LIMIT', '100')))
        profiles = [(metadata_token, collect_db, daily_limit, DeepSeekBackend)]

    episode_token_file = os.getenv('METADATA_TRANSLATION_EPISODE_TOKEN_FILE')
    if episode_token_file:
        class EpisodeBackend(DeepSeekBackend):
            def _assemble_messages(self, *args, **kwargs):
                messages = super()._assemble_messages(*args, **kwargs)
                messages[0]['content'] += (
                    ' Translate complete TV episode metadata into Simplified Chinese as one single physical line.'
                    ' Translate every hiragana and katakana word, including phonetic names, into Chinese transliterations.'
                    ' Retain all narrative meaning and credits. No commentary or numbering.'
                )
                return messages

            def _estimate_max_tokens(self, lines):
                return min(4096, max(600, sum(len(line) for line in lines) * 3 + 256))

        episode_token = Path(episode_token_file).read_text(encoding='utf-8').strip()
        episode_db = Path(os.environ['METADATA_TRANSLATION_EPISODE_USAGE_DB']).resolve()
        if (not episode_token
                or any(hmac.compare_digest(token.encode(), episode_token.encode()) for token, *_ in profiles)
                or any(episode_db == path or (episode_db.exists() and path.exists() and episode_db.samefile(path))
                       for _, path, *_ in profiles)):
            raise SystemExit('Use a distinct episode translation token and usage database')
        profiles.append((episode_token, episode_db, None, EpisodeBackend))

    api_key = deepseek_key()
    if not api_key:
        raise SystemExit('Configure the DeepSeek backend in Sublarr')
    # Each client has one active request slot. Match the provider's process-wide
    # concurrency to those slots so a long batch cannot block the other client.
    get_concurrency().register('deepseek', len(profiles))
    clients = []
    for token, usage_db, daily_limit, backend in profiles:
        bridge = Bridge(backend(api_key=api_key, model='deepseek-chat'), usage_db,
                        daily_limit, (LineCountMismatchError, LineMisalignmentError))
        # Sublarr writes usage events through the Flask application bound to this client.
        app = Flask(__name__)
        app.config['SQLALCHEMY_DATABASE_URI'] = URL.create('sqlite', database=str(usage_db))
        db.init_app(app)
        with app.app_context():
            TranslationEvent.__table__.create(db.engine, checkfirst=True)
        clients.append(Client(f'Bearer {token}'.encode('utf-8'), bridge, app))

    port = int(os.getenv('METADATA_TRANSLATION_PORT', '7100'))
    with ThreadingHTTPServer((translation_bind(), port), handler(clients)) as server:
        server.serve_forever()


if __name__ == '__main__':
    main()
