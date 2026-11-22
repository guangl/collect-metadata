import sys

from supervise import supervise


if __name__ == '__main__':
    if len(sys.argv) < 2:
        raise SystemExit('Pass the Sublarr server command')
    raise SystemExit(supervise((
        ('Sublarr', sys.argv[1:]),
        ('Metadata translation', [sys.executable, '-Bu', '/bridge/metadata_translation_bridge.py']),
    )))
