#!/usr/bin/env python3
"""Exercise the real CLI, worker processes, HTTP API, and persistence.
No exchange orders or account keys. --live additionally observes public quotes.
"""
import argparse
import json
import pathlib
import signal
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser()
parser.add_argument('--live', action='store_true')
args = parser.parse_args()
with tempfile.TemporaryDirectory(prefix='tradeclimb-smoke-') as folder:
    state = pathlib.Path(folder)
    binary = ROOT / 'target/release/tc'
    def cli(*arguments):
        result = subprocess.run([str(binary), '--data-dir', str(state / 'data'), *arguments], cwd=ROOT, text=True, capture_output=True, check=True)
        return json.loads(result.stdout)
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    base = f'http://127.0.0.1:{port}'
    log = open(state / 'service.log', 'w+')
    service = None
    def start():
        return subprocess.Popen([str(binary), '--data-dir', str(state / 'data'), 'serve', '--bind', f'127.0.0.1:{port}', '--assets', str(ROOT / 'dist')], cwd=ROOT, stdout=log, stderr=log)
    def request(path, body=None):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(base + path, data=data, headers={'Content-Type': 'application/json', 'Authorization':'Bearer '+(state/'data/auth/owner.key').read_text().strip()})
        with urllib.request.urlopen(req, timeout=20) as response:
            return json.load(response)
    def until(check, seconds=120):
        deadline = time.monotonic() + seconds
        last = None
        while time.monotonic() < deadline:
            try:
                last = check()
                if last:
                    return last
            except (OSError, urllib.error.URLError):
                pass
            time.sleep(.5)
        raise AssertionError(f'timed out; last={last}')
    def done(job):
        value = request('/api/jobs/' + job)
        assert value['state'] not in ('failed', 'interrupted'), value
        return value if value['state'] == 'completed' else None
    try:
        demo = cli('demo')
        service = start()
        until(lambda: request('/api/overview'))
        for record in demo['strategies']:
            result = until(lambda: done(record['job']))
            assert len(result['result']['equity']) <= 1500
            assert len(result['result']['fills']) <= 200
            assert len(result['result']['benchmarks']) == 2
        before = request('/api/overview')
        assert before['campaigns'][0]['objective'] == 'sharpe_daily_v1'
        assert all(j['objective'] == 'sharpe_daily_v1' for j in before['jobs'])
        assert all(g['objective'] == 'sharpe_daily_v1' for g in before['leaderboards'])
        # Zero-variance cash stays unranked rather than receiving an infinite score.
        ranked = {entry['strategy'] for g in before['leaderboards'] for entry in g['entries']}
        assert demo['strategies'][0]['strategy'] not in ranked
        assert all('sharpe' in g['entries'][0] for g in before['leaderboards'])
        assert all('equity' not in (j['result'] or {}) for j in before['jobs'])
        source = demo['strategies'][-1]['strategy']
        submission = dict(strategy=source, campaign=demo['campaign'], period='validation', idempotency_key='smoke-validation')
        validation = request('/api/jobs', submission)
        result = until(lambda: done(validation['id']))
        assert set(result['result']) == {'sharpe'}, result
        submission['idempotency_key'] = 'smoke-validation-repeat'
        repeated = request('/api/jobs', submission)
        assert repeated['id'] == validation['id']
        assert request('/api/overview')['validation']['used'] == 1
        legacy = cli('campaign', demo['dataset'], '--objective', 'net_cagr')
        legacy_job = request('/api/jobs', dict(submission, campaign=legacy['id'], idempotency_key='legacy-validation'))
        legacy_result = until(lambda: done(legacy_job['id']))
        assert set(legacy_result['result']) == {'net_cagr'}, legacy_result
        assert legacy_job['id'] != validation['id']
        assert request('/api/overview')['validation']['used'] == 2
        try:
            request('/api/jobs/' + validation['id'] + '/trades')
            raise AssertionError('validation trade details exposed')
        except urllib.error.HTTPError as error:
            assert error.code == 400
        external = cli('register', '--name', 'Half BTC external', '--description', 'External SDK smoke fixture', '--project', str(ROOT / 'examples/strategy'), '--executable', str(ROOT / 'examples/strategy/target/release/tc-example-strategy'))
        run = request('/api/jobs', dict(strategy=external['id'], campaign=demo['campaign'], period='development', idempotency_key='external'))
        report = until(lambda: done(run['id']))
        assert report['result']['metrics']['trades'] > 0
        trades = request('/api/jobs/' + run['id'] + '/trades?limit=1')
        assert len(trades['fills']) == 1
        assert trades['total'] >= 2
        cancel_job = request('/api/jobs', dict(strategy=source, campaign=demo['campaign'], period='development', idempotency_key='cancel'))
        cancelled = request('/api/jobs/' + cancel_job['id'] + '/cancel', {})
        assert cancelled['state'] == 'cancelled'
        # Kill the control service during a blocked external strategy, then
        # verify restart cleans up the recorded Linux process group.
        stalled = state / 'stalled-strategy'
        stalled.write_text('#!/bin/sh\nsleep 30\n')
        stalled.chmod(0o755)
        slow = cli('register', '--name', 'Stalled fixture', '--description', 'Crash recovery check', '--project', str(ROOT / 'examples/strategy'), '--executable', str(stalled))
        stalled_job = request('/api/jobs', dict(strategy=slow['id'], campaign=demo['campaign'], period='development', idempotency_key='hard-crash'))
        def running_group():
            with sqlite3.connect(state / 'data/tradeclimb.db') as db:
                row = db.execute("SELECT data FROM objects WHERE kind='worker_lease' AND id=?", (stalled_job['id'],)).fetchone()
            if not row:
                return None
            pid = json.loads(row[0])['pid']
            children = pathlib.Path(f'/proc/{pid}/task/{pid}/children')
            return pid if children.exists() and children.read_text().strip() else None
        worker_pid = until(running_group)
        service.kill()
        service.wait(timeout=10)
        service = start()
        until(lambda: request('/api/jobs/' + stalled_job['id'])['state'] == 'interrupted')
        def no_worker():
            stat = pathlib.Path(f'/proc/{worker_pid}/stat')
            return not stat.exists() or stat.read_text().rsplit(')', 1)[1].split()[0] == 'Z'
        until(no_worker, 10)
        if args.live:
            # The promotion path requires a native-data, eligible evaluation. This
            # isolated fixture substitutes provenance/metrics only to exercise
            # authorization and live recovery; it is not market evidence.
            native_manifest=state/'data/datasets'/demo['dataset']/'manifest.json'
            provenance=json.loads(native_manifest.read_text());provenance['synthetic']=False;provenance['venue']='bitvavo';native_manifest.write_text(json.dumps(provenance))
            with sqlite3.connect(state/'data/tradeclimb.db') as db:
                raw=db.execute('SELECT data FROM jobs WHERE id=?',(run['id'],)).fetchone()[0]
                fixture=json.loads(raw);fixture['result']['metrics'].update(eligible=True,max_drawdown=0.01,sharpe=1.0,net_cagr=0.10)
                db.execute('UPDATE jobs SET data=? WHERE id=?',(json.dumps(fixture),run['id']))
            paper = request('/api/paper', dict(strategy=external['id'],job=run['id'],approve=True))
            def paper_filled():
                p = next(p for p in request('/api/overview')['paper'] if p['id'] == paper['id'])
                assert p['state'] in ('starting','running'), p
                return p if p['simulator']['fills'] else None
            observed = until(paper_filled, 160)
            sequence = observed['sequence']
            service.send_signal(signal.SIGINT)
            service.wait(timeout=30)
            service = start()
            until(lambda: request('/api/overview'))
            until(lambda: next(p for p in request('/api/overview')['paper'] if p['id'] == paper['id'])['sequence'] > sequence, 60)
            stopped = request('/api/paper/' + paper['id'] + '/stop', {})
            assert stopped['state'] in ('stopping','stopped')
            until(lambda: next(p for p in request('/api/overview')['paper'] if p['id'] == paper['id'])['state'] == 'stopped', 8)
        backup = state / 'backup'
        subprocess.run([str(binary), '--data-dir', str(state / 'data'), 'backup', str(backup)], check=True, capture_output=True)
        restored = subprocess.run([str(binary), '--data-dir', str(backup), 'list'], check=True, capture_output=True, text=True)
        assert json.loads(restored.stdout)['validation']['used'] == 2
        # A restored run must not depend on the original artifact location.
        pathlib.Path(external['spec']['kind']['path']).unlink()
        new_run = subprocess.run([str(binary), '--data-dir', str(backup), 'run', '--strategy', external['id'], '--campaign', demo['campaign'], '--key', 'restored-external'], check=True, capture_output=True, text=True)
        restored_job = json.loads(new_run.stdout)['id']
        subprocess.run([str(binary), '--data-dir', str(backup), 'worker', restored_job], check=True, capture_output=True)
        assert (backup / 'jobs' / restored_job / 'report.json.gz').exists()
        print('PASS: real CLI → workers → development → scalar validation/cache → external SDK → cancellation → backup/restore' + (' → live paper fills/restart/stop' if args.live else ''))
    except Exception:
        log.flush()
        log.seek(0)
        print(log.read())
        raise
    finally:
        if service and service.poll() is None:
            service.send_signal(signal.SIGINT)
            try:
                service.wait(timeout=30)
            except subprocess.TimeoutExpired:
                service.kill()
                service.wait()
        log.close()
