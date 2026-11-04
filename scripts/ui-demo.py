#!/usr/bin/env python3
"""Create an isolated, explicitly synthetic UI backlog. Never use a real workspace."""
import argparse
import copy
import hashlib
import json
import pathlib
import sqlite3
import subprocess
import uuid

root = pathlib.Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--data-dir', type=pathlib.Path, default=root / 'var/ui-backlog')
parser.add_argument('--families', type=int, default=120)
parser.add_argument('--versions', type=int, default=40)
parser.add_argument('--runs', type=int, default=2000)
args = parser.parse_args()
state = args.data_dir.resolve()
if (state / 'tradeclimb.db').exists():
    parser.error('Use a fresh directory; existing workspaces are never modified.')
subprocess.run([str(root / 'target/release/tc'), '--data-dir', str(state), 'demo'], check=True, stdout=subprocess.DEVNULL)
db = sqlite3.connect(state / 'tradeclimb.db')
strategy = json.loads(db.execute("SELECT data FROM objects WHERE kind='strategy' LIMIT 1").fetchone()[0])
job = json.loads(db.execute('SELECT data FROM jobs LIMIT 1').fetchone()[0])
# CLI demo submits queued evaluations; the service will execute these three real fixtures.
# Backlog rows deliberately have no results; they are navigation fixtures, never market evidence.
names = ['Adaptive trend', 'Breakout channel', 'Cross-market momentum', 'Defensive rotation', 'Mean reversion', 'Volatility filter']
ids = []
for f in range(args.families):
    parent = None
    for v in range(args.versions):
        item = copy.deepcopy(strategy)
        item['id'] = hashlib.sha256(f'ui-fixture-{f}-{v}'.encode()).hexdigest()
        item['parent'] = parent
        item['spec']['name'] = f'{names[f % len(names)]} {f + 1:03d}' + (f' · revision {v + 1}' if v else '')
        item['spec']['description'] = 'Synthetic UI backlog fixture. For navigation and layout testing only.'
        item['spec']['parameters'] = {'lookback': 20 + v, 'fixture': True}
        item['created_at'] = strategy['created_at'] - (args.versions - v) * 60000
        item['archived'] = v == 5
        db.execute('INSERT INTO objects VALUES(?,?,?)', ('strategy', item['id'], json.dumps(item)))
        ids.append(item['id'])
        parent = item['id']
for n in range(args.runs):
    item = copy.deepcopy(job)
    item.update(id=str(uuid.uuid4()), strategy=ids[n % len(ids)], state='completed' if n % 13 else 'failed', result=None, progress=1.0, error='Synthetic layout fixture' if n % 13 == 0 else None)
    item['created_at'] -= (args.runs - n) * 60000
    item['updated_at'] = item['created_at']
    db.execute('INSERT INTO jobs VALUES(?,?,?,?)', (item['id'], f'ui-fixture-{n}', f'ui-fixture-{n}', json.dumps(item)))
for n in range(240):
    item = dict(id=str(uuid.uuid4()), strategy=ids[(n * args.versions) % len(ids)], campaign=job['campaign'], state='completed' if n % 9 else 'cancelled', jobs=[], trials=20, completed=20 if n % 9 else 8, best=None)
    db.execute('INSERT INTO objects VALUES(?,?,?)', ('tuning', item['id'], json.dumps(item)))
db.commit()
db.close()
print(f'Synthetic UI demo: {state}\n{len(ids)} versions · {args.runs} backtests · 240 tuning runs')
print(f'./target/release/tc --data-dir {state} serve --bind 127.0.0.1:8088 --assets dist')
