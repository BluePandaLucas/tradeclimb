#!/usr/bin/env python3
"""Compare fixed release workloads in fresh processes; requires GNU time (Linux)."""
import argparse
import hashlib
import json
import pathlib
import statistics
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('baseline')
parser.add_argument('candidate')
parser.add_argument('--pairs', type=int, default=7)
parser.add_argument('--minutes', type=int, default=100000)
parser.add_argument('--repetitions', type=int, default=3)
parser.add_argument('--output', type=pathlib.Path, required=True)
args = parser.parse_args()
if args.pairs < 3:
    parser.error('at least three pairs required')
samples = {'baseline': [], 'candidate': []}

def measure(label):
    with tempfile.NamedTemporaryFile() as usage:
        started = time.perf_counter()
        result = subprocess.run(
            ['/usr/bin/time', '-f', '%U %S %M', '-o', usage.name,
             str(pathlib.Path(getattr(args, label)).resolve()),
             str(args.minutes), str(args.repetitions)],
            capture_output=True, text=True, check=True)
        wall = time.perf_counter() - started
        user, system, rss = map(float, pathlib.Path(usage.name).read_text().split())
    row = json.loads(result.stdout)
    row.update(wall_seconds=wall, cpu_seconds=user + system,
               cpu_percent=100 * (user + system) / wall, peak_rss_mib=rss / 1024)
    return row

# Warm caches for each executable, then alternate order to reduce drift bias.
for label in samples:
    measure(label)
for pair in range(args.pairs):
    for label in (list(samples) if pair % 2 == 0 else list(reversed(samples))):
        row = measure(label)
        samples[label].append(row)
        print(f'{pair + 1}/{args.pairs} {label}: {row["simulation_seconds"]:.3f}s', flush=True)
checksums = {row['report_sha256'] for rows in samples.values() for row in rows}
assert len(checksums) == 1, 'report changed between runs or revisions'
fields = ['simulation_seconds', 'wall_seconds', 'cpu_seconds', 'cpu_percent', 'peak_rss_mib']
medians = {label: {key: statistics.median(r[key] for r in rows) for key in fields}
           for label, rows in samples.items()}
ratios = {key: medians['candidate'][key] / medians['baseline'][key] for key in fields}
passed = (ratios['simulation_seconds'] <= .75 and ratios['wall_seconds'] <= .75
          and ratios['cpu_percent'] <= 1.1 and ratios['peak_rss_mib'] <= 1.1)
report = dict(samples=samples, medians=medians, candidate_over_baseline=ratios,
              passed=passed, report_sha256=checksums.pop(),
              workload_sha256=hashlib.sha256((pathlib.Path(__file__).resolve().parents[1] /
                  'crates/tc-engine/examples/performance.rs').read_bytes()).hexdigest(),
              binary_sha256={label: hashlib.sha256(pathlib.Path(getattr(args, label)).read_bytes()).hexdigest()
                             for label in samples})
args.output.write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps({key: report[key] for key in ['medians', 'candidate_over_baseline', 'passed']}, indent=2))
raise SystemExit(0 if passed else 1)
