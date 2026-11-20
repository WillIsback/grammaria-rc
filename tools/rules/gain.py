#!/usr/bin/env python3
"""What a rule file adds to the reference checker on the benchmark.

    python3 tools/rules/gain.py FILE.xml [FILE2.xml ...] [--set gl|lt|corpus|all] [--split dev|test|all]
                                [--bin PATH] [--assets DIR] [--show N] [--rule ID]

The rule files are loaded alone and run on the benchmark texts. For each rule:
  new     expected error spans it touches that the reference misses (the gain)
  known   expected error spans it touches that the reference already finds
  false   its matches that touch no expected error span and no reference
          match: false alarms on text the benchmark holds correct
  also    its matches outside expected spans where the reference reports
          something too (not counted against the rule)
`--show N` prints up to N false alarms and N new hits per rule.
Develop on the dev split; keep the test split to check that a rule generalises.
"""
import collections
import json
import os
import subprocess
import sys

args = sys.argv[1:]
opt = lambda name, default: args[args.index(name) + 1] if name in args else default
files = []
i = 0
while i < len(args):
    if args[i].startswith("--"):
        i += 2
        continue
    files.append(args[i])
    i += 1
root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
binary = opt("--bin", os.path.join(root, "target", "release", "grammaria-run"))
assets = opt("--assets", os.path.join(root, "assets", "fr", "lt"))
sets = ["gl", "lt", "corpus"] if opt("--set", "gl") == "all" else [opt("--set", "gl")]
split = opt("--split", "dev")
show = int(opt("--show", "0"))
only = opt("--rule", None)


def touches(m, span):
    a, b = m[1], m[1] + max(m[2], 1)
    return a < span[1] and span[0] < b


stat = collections.defaultdict(lambda: collections.Counter())
examples = collections.defaultdict(lambda: collections.defaultdict(list))
total_new = set()
for name in sets:
    items = [json.loads(l) for l in open(os.path.join(root, "eval", "bench", name + ".jsonl"), encoding="utf-8")]
    ref = [json.loads(l) for l in open(os.path.join(root, "eval", "oracle", f"ltjs.{name}.matches.jsonl"), encoding="utf-8")]
    cmd = [binary, "matches", os.path.join(root, "eval", "bench", name + ".txt"), "--assets", assets]
    for f in files:
        cmd += ["--rules", f]
    out = subprocess.run(cmd, capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit(out.stderr)
    for n, (it, r, line) in enumerate(zip(items, ref, out.stdout.split("\n"))):
        if split != "all" and it["split"] != split:
            continue
        text = it["text"]
        for m in json.loads(line):
            rid = m[0]
            if only and rid != only:
                continue
            mark = text[:m[1]] + "[" + text[m[1]:m[1] + m[2]] + "]" + text[m[1] + m[2]:]
            hit = [sp for sp in it["spans"] if touches(m, sp)]
            if hit:
                for sp in hit:
                    if any(touches(x, sp) for x in r):
                        stat[rid]["known"] += 1
                    else:
                        stat[rid]["new"] += 1
                        total_new.add((name, n, tuple(sp)))
                        examples[rid]["new"].append(f"{name}:{n + 1} {mark} -> {m[3][:2]}")
            elif any(touches(m, (x[1], x[1] + max(x[2], 1))) for x in r):
                stat[rid]["also"] += 1
            else:
                stat[rid]["false"] += 1
                examples[rid]["false"].append(f"{name}:{n + 1} {mark} -> {m[3][:2]}")

print(f"sets: {','.join(sets)}; split: {split}")
print(f"{'rule':40s} {'new':>5s} {'known':>6s} {'false':>6s} {'also':>5s}")
tot = collections.Counter()
for rid in sorted(stat, key=lambda r: -stat[r]["new"]):
    s = stat[rid]
    tot.update(s)
    print(f"{rid:40s} {s['new']:5d} {s['known']:6d} {s['false']:6d} {s['also']:5d}")
    for kind in ("false", "new"):
        for e in examples[rid][kind][:show]:
            print(f"      {kind}: {e}")
print(f"{'TOTAL':40s} {tot['new']:5d} {tot['known']:6d} {tot['false']:6d} {tot['also']:5d}   (distinct new spans: {len(total_new)})")
