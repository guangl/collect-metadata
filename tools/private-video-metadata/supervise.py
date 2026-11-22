import os
import signal
import subprocess
import threading
import time


def signal_groups(processes, requested):
    for child in processes:
        try:
            os.killpg(child.pid, requested)
        except ProcessLookupError:
            pass


def supervise(commands):
    stopping = threading.Event()
    for signum in (signal.SIGTERM, signal.SIGINT):
        signal.signal(signum, lambda *_: stopping.set())

    processes = []
    try:
        for name, command in commands:
            if stopping.is_set():
                return 0
            try:
                processes.append(subprocess.Popen(command, start_new_session=True))
            except OSError as error:
                print(f'{name} could not start ({type(error).__name__})', flush=True)
                return 1
        while not stopping.wait(0.25):
            for (name, _), child in zip(commands, processes):
                status = child.poll()
                if status is not None:
                    print(f'{name} exited with {status}; restarting the service', flush=True)
                    return 1
        return 0
    finally:
        signal_groups(processes, signal.SIGTERM)
        deadline = time.monotonic() + 25
        for child in processes:
            try:
                child.wait(timeout=max(0, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                break
        # A worker may have started an ffmpeg or scraping subprocess of its own.
        signal_groups(processes, signal.SIGKILL)
        for child in processes:
            child.wait()


if __name__ == '__main__':
    raise SystemExit(supervise((
        ('Collect Metadata Web', ['/usr/local/bin/collect-metadata-web']),
        ('Private media worker', ['python', '-u', '-m', 'javlibrarian.worker']),
    )))
