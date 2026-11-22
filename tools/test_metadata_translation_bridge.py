import datetime
import json
import sqlite3
import tempfile
import threading
import unittest
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing, nullcontext
from http.server import ThreadingHTTPServer
from pathlib import Path
from types import SimpleNamespace
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from metadata_translation_bridge import Bridge, Client, handler


class SharedTranslationTest(unittest.TestCase):
    def test_authentication_concurrency_and_usage(self):
        allow_completion = threading.Event()
        both_active = threading.Event()
        ready = threading.Barrier(2, action=both_active.set, timeout=5)

        class Backend:
            def translate_batch(self, texts, source, target):
                ready.wait()
                if not allow_completion.wait(5):
                    raise TimeoutError('Translation test did not release the provider')
                return SimpleNamespace(success=True, translated_lines=texts)

        with tempfile.TemporaryDirectory() as directory:
            clients = []
            for name in ('collect', 'private'):
                bridge = Bridge(Backend(), Path(directory) / f'{name}.db', None)
                self.assertTrue(bridge.reserve(100))
                app = SimpleNamespace(app_context=nullcontext)
                clients.append(Client(f'Bearer {name}'.encode(), bridge, app))

            with ThreadingHTTPServer(('127.0.0.1', 0), handler(clients)) as server:
                worker = threading.Thread(target=server.serve_forever, daemon=True)
                worker.start()
                url = f'http://127.0.0.1:{server.server_port}/translate'

                def translate(token, *texts):
                    request = Request(url, json.dumps({'target_language': 'zh', 'texts': texts}).encode(),
                                      {'Authorization': f'Bearer {token}', 'Content-Type': 'application/json'})
                    try:
                        with urlopen(request, timeout=6) as response:
                            return response.status, json.load(response)
                    except HTTPError as error:
                        with error:
                            return error.code, json.load(error)

                try:
                    self.assertEqual(translate('invalid', 'Denied')[0], 401)
                    with ThreadPoolExecutor(max_workers=2) as executor:
                        collect = executor.submit(translate, 'collect', 'Collect')
                        private = executor.submit(translate, 'private', 'Private', 'Synopsis')
                        try:
                            self.assertTrue(both_active.wait(5), 'Both clients must reach the provider concurrently')
                            self.assertEqual(translate('collect', 'Busy')[0], 503)
                        finally:
                            allow_completion.set()
                        self.assertEqual(collect.result(), (200, {'translations': ['Collect']}))
                        self.assertEqual(private.result(), (200, {'translations': ['Private', 'Synopsis']}))
                finally:
                    server.shutdown()
                    worker.join()

            for client, expected in zip(clients, (101, 102)):
                with closing(sqlite3.connect(client.bridge.budget_path)) as db:
                    self.assertEqual(db.execute('SELECT SUM(used) FROM budget').fetchone()[0], expected)


if __name__ == '__main__':
    unittest.main()
