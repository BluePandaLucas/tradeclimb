#!/usr/bin/env python3
"""Exercise the actual start script through a terminal, keeping SSE open."""
import fcntl
import os
import pathlib
import pty
import select
import signal
import socket
import subprocess
import tempfile
import termios
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
for command in (b'q\n', b'exit\n', b'\x03'):
    with tempfile.TemporaryDirectory(prefix='tc-shutdown-') as state:
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        master, slave = pty.openpty()
        def terminal():
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
        env = os.environ.copy()
        env['TC_DATA_DIR'] = state
        process = subprocess.Popen([str(ROOT / 'scripts/start'), '--bind', f'127.0.0.1:{port}'], cwd=ROOT, env=env, stdin=slave, stdout=slave, stderr=slave, preexec_fn=terminal)
        os.close(slave)
        stream = None
        transcript = bytearray()
        try:
            deadline = time.monotonic() + 90
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise AssertionError(f'start script exited early: {transcript.decode(errors="replace")}')
                if select.select([master], [], [], .1)[0]:
                    transcript.extend(os.read(master, 65536))
                if b'service ready' in transcript:
                    break
            else:
                raise AssertionError(f'server did not become ready: {transcript.decode(errors="replace")}')
            # Reproduces the browser connection that previously prevented exit.
            stream = urllib.request.urlopen(urllib.request.Request(f'http://127.0.0.1:{port}/api/events',headers={'Authorization':'Bearer '+(pathlib.Path(state)/'auth/owner.key').read_text().strip()}), timeout=10)
            assert stream.readline().startswith(b'event:')
            started = time.monotonic()
            os.write(master, command)
            process.wait(timeout=8)
            assert process.returncode == 0, transcript.decode(errors='replace')
            print(f'PASS: {command!r} exits the start script with SSE open ({time.monotonic()-started:.2f}s)')
        finally:
            if stream:
                stream.close()
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            os.close(master)
