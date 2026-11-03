#!/usr/bin/env python3
"""Measure an already-built Rust benchmark without emitting financial feedback."""
import json
import pathlib
import resource
import subprocess
import sys
import time
root = pathlib.Path(__file__).resolve().parent.parent
started = time.perf_counter()
result = subprocess.run([str(root / 'target/release/examples/benchmark'), *sys.argv[1:]], capture_output=True, text=True, check=True)
measurement = json.loads(result.stdout)
measurement['wall_seconds'] = time.perf_counter() - started
rss = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
measurement['peak_rss_mib'] = rss / (1024 * 1024 if sys.platform == 'darwin' else 1024)
print(json.dumps(measurement, indent=2))
