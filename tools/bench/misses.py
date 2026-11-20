#!/usr/bin/env python3
"""Reference matches we miss, split by cause: the rule's filter has no native
counterpart yet, or something else (listed with the shortest example).

    python3 tools/bench/misses.py NAME [--all]     (NAME: corpus, lt, gl)
"""
import collections
import json
import sys

sys.path.insert(0, "tools/bench")
from rule_filters import filters

flt = {}
for f in ("grammar.xml", "style.xml"):
    flt.update(filters(f"assets/fr/lt/{f}"))
name = sys.argv[1]
supported = set(sys.argv[sys.argv.index("--have") + 1].split(",")) if "--have" in sys.argv else set()
by_filter = collections.Counter()
other = collections.Counter()
ex = {}
for ln, (a, b, t) in enumerate(zip(open(f"eval/oracle/ltjs.{name}.matches.jsonl"), open(f"eval/out/new.{name}.matches.jsonl"), open(f"eval/bench/{name}.txt"))):
    ours = {(m[0], m[1], m[2]) for m in json.loads(b)}
    for m in json.loads(a):
        if m[7] not in ("PatternRule", "RegexPatternRule") or (m[0], m[1], m[2]) in ours:
            continue
        cls = flt.get(m[5])
        if cls and cls not in supported:
            by_filter[cls] += 1
        else:
            other[m[5]] += 1
            if m[5] not in ex or len(t) < len(ex[m[5]][1]):
                ex[m[5]] = (ln + 1, t.strip()[:90], m[1], m[2], m[3][:2])
print(name, "missing because of a filter:", dict(by_filter))
print(name, "missing otherwise:", sum(other.values()))
for k, v in other.most_common(60):
    print("   ", v, k, ex[k])
