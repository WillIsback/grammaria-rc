#!/usr/bin/env python3
"""Errors of the benchmark that the reference checker misses: what to write rules for.

    python3 tools/rules/targets.py [PREFIX ...] [--set gl|lt|corpus] [--split dev|test|all] [--all] [--list]

Groups the missed error spans by the source rule named in the benchmark
(for `gl`: the Grammalecte rule the sentence tests), keeping the names that
start with one of the PREFIXes (all names when none is given). `--list`
prints every missed sentence with the error in brackets instead of two
examples per name; `--all` also counts the errors the reference finds.
"""
import collections
import json
import os
import sys

args = sys.argv[1:]
opt = lambda name, default: args[args.index(name) + 1] if name in args else default
prefixes = []
i = 0
while i < len(args):
    if args[i] in ("--set", "--split"):
        i += 2
        continue
    if not args[i].startswith("--"):
        prefixes.append(args[i])
    i += 1
root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
name = opt("--set", "gl")
split = opt("--split", "dev")
items = [json.loads(l) for l in open(os.path.join(root, "eval", "bench", name + ".jsonl"), encoding="utf-8")]
ref = [json.loads(l) for l in open(os.path.join(root, "eval", "oracle", f"ltjs.{name}.matches.jsonl"), encoding="utf-8")]


def touches(m, span):
    a, b = m[1], m[1] + max(m[2], 1)
    return a < span[1] and span[0] < b


missed = collections.Counter()
total = collections.Counter()
examples = collections.defaultdict(list)
for n, (it, r) in enumerate(zip(items, ref)):
    if split != "all" and it["split"] != split:
        continue
    rule = it.get("rule") or "?"
    if prefixes and not any(rule.startswith(p) for p in prefixes):
        continue
    for sp in it["spans"]:
        total[rule] += 1
        if not any(touches(m, sp) for m in r):
            missed[rule] += 1
            t = it["text"]
            examples[rule].append(f"{name}:{n + 1} {t[:sp[0]]}[{t[sp[0]:sp[1]]}]{t[sp[1]:]}")
print(f"{sum(missed.values())} missed of {sum(total.values())} error spans ({name}, {split})")
for rule, count in missed.most_common():
    print(f"{count:4d}/{total[rule]:<4d} {rule}")
    for e in examples[rule][: (10**6 if "--list" in args else 2)]:
        print("          " + e.replace("\n", "\\n")[:200])
