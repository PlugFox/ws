#!/usr/bin/env python3
"""Summarizes build/autobahn/index.json: behaviour counts per agent and the
cases that are not OK."""
import collections, json, sys

path = sys.argv[1] if len(sys.argv) > 1 else 'build/autobahn/index.json'
for agent, cases in json.load(open(path)).items():
    counts = collections.Counter(v['behavior'] for v in cases.values())
    close = collections.Counter(v['behaviorClose'] for v in cases.values())
    print(f'{agent}: {len(cases)} cases, behavior {dict(counts)}, close {dict(close)}')
    for k, v in sorted(cases.items(), key=lambda kv: [int(x) for x in kv[0].split('.')]):
        if v['behavior'] not in ('OK', 'INFORMATIONAL') or v['behaviorClose'] not in ('OK', 'INFORMATIONAL'):
            print(f'  {k}: {v["behavior"]} / close {v["behaviorClose"]} ({v.get("duration")} ms)')
