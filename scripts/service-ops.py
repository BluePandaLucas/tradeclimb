#!/usr/bin/env python3
"""Private local backups, health samples and a bounded operational soak."""
import argparse
import datetime
import fcntl
import json
import os
import pathlib
import shutil
import sqlite3
import ssl
import subprocess
import time
import urllib.request

HOME = pathlib.Path.home()
BASE = HOME / '.local/lib/tradeclimb/current'
DATA = HOME / '.local/share/tradeclimb'
STATE = HOME / '.local/state/tradeclimb'
CONFIG = HOME / '.config/tradeclimb'


def health():
    for unit in ('tradeclimb.service', 'tradeclimb-lan.service'):
        subprocess.run(['systemctl', '--user', 'is-active', '--quiet', unit], check=True)
    context = ssl.create_default_context(cafile=str(CONFIG / 'lan.crt'))
    key = (DATA / 'auth/owner.key').read_text().strip()
    # Bypass shell/proxy environment; never log the credential or session body.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=context))
    request = urllib.request.Request('https://localhost:8443/api/session', headers={'Authorization': 'Bearer ' + key})
    with opener.open(request, timeout=10) as response:
        assert json.load(response)['role'] == 'owner'
    with opener.open('https://localhost:8443/', timeout=10) as response:
        assert b'<html' in response.read(100_000).lower()
    free = shutil.disk_usage(DATA).free
    assert free >= 5 * 1024**3, 'Less than 5 GiB disk space free'
    warnings = []
    with sqlite3.connect(f'file:{DATA}/tradeclimb.db?mode=ro', uri=True) as db:
        expiring = db.execute('SELECT COUNT(*) FROM agent_credentials WHERE revoked=0 AND expires < ?',
                              (int(time.time() * 1000) + 7 * 86400_000,)).fetchone()[0]
    if expiring:
        warnings.append(f'{expiring} agent credential(s) expired or expire within seven days')
    certificate = ssl._ssl._test_decode_cert(str(CONFIG / 'lan.crt'))
    if ssl.cert_time_to_seconds(certificate['notAfter']) < time.time() + 30 * 86400:
        warnings.append('LAN certificate expires within thirty days')
    backups = sorted((HOME / '.local/share/tradeclimb-backups').glob('backup-*'))
    if not backups or time.time() - backups[-1].stat().st_mtime > 2 * 86400:
        warnings.append('No successful backup within two days')
    return {'time': datetime.datetime.now(datetime.UTC).isoformat(), 'ok': True,
            'free_bytes': free, 'warnings': warnings}


def backup():
    root = HOME / '.local/share/tradeclimb-backups'
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    stamp = datetime.datetime.now(datetime.UTC).strftime('%Y%m%dT%H%M%SZ')
    pending = root / f'.partial-{stamp}'
    destination = root / f'backup-{stamp}'
    # Reserve space for the complete copy plus a safety margin. Raw caches are
    # excluded by tc backup and must not inflate this estimate.
    names = ('tradeclimb.db', 'tradeclimb.db-wal', 'datasets', 'strategies', 'jobs',
             'evaluations', 'final-test', 'platforms', 'candidates')
    estimate = sum(path.stat().st_size for name in names
                   for path in ([DATA / name] if (DATA / name).is_file() else (DATA / name).rglob('*'))
                   if path.is_file())
    assert shutil.disk_usage(root).free >= estimate * 1.2 + 5 * 1024**3, 'Insufficient disk space for backup plus 5 GiB reserve'
    subprocess.run([str(BASE / 'tc'), '--data-dir', str(DATA), 'backup', str(pending)], check=True)
    with sqlite3.connect(f'file:{pending}/tradeclimb.db?mode=ro', uri=True) as db:
        assert db.execute('PRAGMA quick_check').fetchone()[0] == 'ok', 'Backup database integrity failure'
    pending.rename(destination)
    # Only prune complete backups after publishing a new successful backup.
    for old in sorted(root.glob('backup-*'))[:-7]:
        shutil.rmtree(old)
    print(json.dumps({'backup': str(destination), 'retained': 7}))


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['health', 'backup', 'soak'])
    parser.add_argument('--hours', type=float, default=12)
    args = parser.parse_args()
    STATE.mkdir(parents=True, exist_ok=True, mode=0o700)
    if args.action == 'backup':
        with (STATE / 'backup.lock').open('a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            backup()
    elif args.action == 'health':
        try:
            sample = health()
        except Exception as error:
            (STATE / 'health.json').write_text(json.dumps({
                'time': datetime.datetime.now(datetime.UTC).isoformat(),
                'ok': False, 'error': type(error).__name__}) + '\n')
            raise
        (STATE / 'health.json').write_text(json.dumps(sample) + '\n')
        print(json.dumps(sample))
    else:
        assert 0 < args.hours <= 24, 'Soak must be between zero and 24 hours'
        end = time.monotonic() + args.hours * 3600
        stamp = datetime.datetime.now(datetime.UTC).strftime('%Y%m%dT%H%M%SZ')
        report = STATE / f'soak-{stamp}.jsonl'
        samples = 0
        with report.open('x') as output:
            while True:
                sample = health()
                output.write(json.dumps(sample) + '\n')
                output.flush()
                samples += 1
                if time.monotonic() >= end:
                    break
                time.sleep(min(60, end - time.monotonic()))
            output.write(json.dumps({'complete': True, 'samples': samples}) + '\n')
        print(json.dumps({'complete': True, 'samples': samples, 'report': str(report)}))


if __name__ == '__main__':
    main()
