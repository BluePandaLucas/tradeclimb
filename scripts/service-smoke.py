#!/usr/bin/env python3
"""Exercise the deployed service template with disposable synthetic data."""
import json
import pathlib
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.request
import uuid

ROOT = pathlib.Path(__file__).resolve().parent.parent
RELEASE = (pathlib.Path.home() / '.local/lib/tradeclimb/current').resolve()
unit = 'tradeclimb-acceptance-' + uuid.uuid4().hex + '.service'
unit_path = pathlib.Path.home() / '.config/systemd/user' / unit


def systemctl(*args):
    return subprocess.check_output(['systemctl', '--user', *args], text=True).strip()


def until(check, seconds=120):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            result = check()
            if result:
                return result
        except OSError:
            pass
        time.sleep(.2)
    raise AssertionError('Service acceptance check timed out')


with tempfile.TemporaryDirectory(prefix='tc-service-acceptance-') as directory:
    root = pathlib.Path(directory)
    data = root / 'data'
    backup = root / 'backup'
    binary = RELEASE / 'tc'
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]

    def cli(*args, state=data):
        result = subprocess.run([str(binary), '--data-dir', str(state), *args],
                                cwd=RELEASE / 'sdk', capture_output=True, text=True, check=True)
        return json.loads(result.stdout)

    def request(path, body=None, state=data):
        key = (state / 'auth/owner.key').read_text().strip()
        req = urllib.request.Request(f'http://127.0.0.1:{port}' + path,
                                     data=None if body is None else json.dumps(body).encode(),
                                     headers={'Content-Type': 'application/json', 'Authorization': 'Bearer ' + key})
        with urllib.request.urlopen(req, timeout=10) as response:
            return json.load(response)

    def write_unit(state):
        text = (ROOT / 'deploy/tradeclimb.service').read_text()
        for key, value in {'HOME': pathlib.Path.home(), 'RELEASE': RELEASE, 'DATA': state, 'HOSTS': 'localhost'}.items():
            text = text.replace('@' + key + '@', str(value))
        unit_path.write_text(text.replace('127.0.0.1:8080', f'127.0.0.1:{port}'))
        systemctl('daemon-reload')

    def done(job, state=data):
        record = request('/api/jobs/' + job, state=state)
        assert record['state'] not in ('failed', 'cancelled', 'interrupted'), record
        return record if record['state'] == 'completed' else None

    try:
        demo = cli('demo')
        write_unit(data)
        systemctl('start', unit)
        until(lambda: request('/api/session'))
        for record in demo['strategies']:
            until(lambda: done(record['job']))
        owner = (data / 'auth/owner.key').read_bytes()
        initial_pid = systemctl('show', unit, '-p', 'MainPID', '--value')
        systemctl('stop', unit)
        assert systemctl('show', unit, '-p', 'Result', '--value') == 'success'
        systemctl('start', unit)
        until(lambda: request('/api/session'))
        assert systemctl('show', unit, '-p', 'MainPID', '--value') != initial_pid
        assert (data / 'auth/owner.key').read_bytes() == owner
        # Hold a real isolated worker open so SIGKILL exercises interrupted work.
        stalled = root / 'stalled'
        stalled.write_text('#!/bin/sh\nsleep 120\n')
        stalled.chmod(0o700)
        strategy = cli('register', '--name', 'Service recovery fixture', '--description', 'Synthetic QA only',
                       '--executable', str(stalled), '--project', str(ROOT / 'examples/strategy'))
        job = request('/api/jobs', {'strategy': strategy['id'], 'campaign': demo['campaign'],
                                    'period': 'development', 'idempotency_key': 'service-crash'})

        def worker_pid():
            with sqlite3.connect(data / 'tradeclimb.db') as db:
                row = db.execute("SELECT data FROM objects WHERE kind='worker_lease' AND id=?", (job['id'],)).fetchone()
            if not row:
                return None
            pid = json.loads(row[0])['pid']
            children = pathlib.Path(f'/proc/{pid}/task/{pid}/children')
            return pid if children.exists() and children.read_text().strip() else None

        worker = until(worker_pid)
        pid = systemctl('show', unit, '-p', 'MainPID', '--value')
        systemctl('kill', '--kill-whom=main', '--signal=SIGKILL', unit)
        until(lambda: systemctl('show', unit, '-p', 'MainPID', '--value') not in ('0', pid))
        until(lambda: request('/api/jobs/' + job['id'])['state'] == 'interrupted')
        until(lambda: not pathlib.Path(f'/proc/{worker}').exists())
        # Owner/scientific ledgers survive crashes; this test never uses validation.
        assert request('/api/overview')['validation']['used'] == 0
        assert (data / 'auth/owner.key').read_bytes() == owner
        subprocess.run([str(binary), '--data-dir', str(data), 'backup', str(backup)], check=True)
        original_job = demo['strategies'][1]['job']
        original_report = request('/api/jobs/' + original_job)['result']
        systemctl('stop', unit)
        # Hide original state to prove the restored instance is self-contained.
        data.rename(root / 'original-hidden')
        write_unit(backup)
        systemctl('start', unit)
        until(lambda: request('/api/session', state=backup))
        assert request('/api/jobs/' + original_job, state=backup)['result'] == original_report
        assert request('/api/overview', state=backup)['validation']['used'] == 0
        # A new evaluation must execute from the restored dataset/artifact tree.
        rerun = request('/api/jobs', {'strategy': demo['strategies'][1]['strategy'], 'campaign': demo['campaign'],
                                      'period': 'development', 'idempotency_key': 'restore-check'}, state=backup)
        until(lambda: done(rerun['id'], state=backup))
        assert (backup / 'auth/owner.key').read_bytes() != owner
        print('PASS: service template cold start, graceful stop, SIGKILL auto-restart, interrupted worker cleanup, durable identity/ledger, backup restore and new evaluation')
    except Exception:
        print(systemctl('status', unit, '--no-pager') if systemctl('show', unit, '-p', 'ActiveState', '--value') == 'active' else
              subprocess.check_output(['journalctl', '--user', '-u', unit, '--no-pager', '-n', '60'], text=True))
        raise
    finally:
        subprocess.run(['systemctl', '--user', 'stop', unit], check=False)
        subprocess.run(['systemctl', '--user', 'reset-failed', unit], check=False, capture_output=True)
        unit_path.unlink(missing_ok=True)
        systemctl('daemon-reload')
