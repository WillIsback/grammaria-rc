#!/usr/bin/env python3
"""Matches we report and the reference does not, split by cause: the span
overlaps a reference match we do not produce yet (it would win the overlap),
or nothing explains it (listed with the shortest example).

    python3 tools/bench/extras.py NAME      (NAME: corpus, lt, gl)
"""
import collections
import json
import sys

name = sys.argv[1]
hidden = collections.Counter()
other = collections.Counter()
ex = {}
for ln, (a, b, t) in enumerate(zip(open(f"eval/oracle/ltjs.{name}.matches.jsonl"), open(f"eval/out/new.{name}.matches.jsonl"), open(f"eval/bench/{name}.txt"))):
    ref = json.loads(a)
    ours = json.loads(b)
    ref_keys = {(m[0], m[1], m[2]) for m in ref}
    our_keys = {(m[0], m[1], m[2]) for m in ours}
    for m in ours:
        if (m[0], m[1], m[2]) in ref_keys:
            continue
        rivals = [r for r in ref if (r[0], r[1], r[2]) not in our_keys and r[1] < m[1] + m[2] and m[1] < r[1] + r[2]]
        if rivals:
            hidden[rivals[0][7] if rivals[0][7] != "PatternRule" else "PatternRule(filter?)"] += 1
        else:
            other[m[5]] += 1
            if m[5] not in ex or len(t) < len(ex[m[5]][1]):
                ex[m[5]] = (ln + 1, t.strip()[:90], m[1], m[2], m[3][:2])
print(name, "extra, hidden in the reference by a match we lack:", dict(hidden))
print(name, "extra otherwise:", sum(other.values()))
for k, v in other.most_common(60):
    print("   ", v, k, ex[k])
