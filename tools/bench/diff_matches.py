#!/usr/bin/env python3
"""Compare rule matches with the reference, rule by rule.

    python3 tools/bench/diff_matches.py REF.matches.jsonl OURS.matches.jsonl [--classes PatternRule,RegexPatternRule] [--top N] [--rule ID]

A reference match is "found" when we report the same rule id on the same
span. Lists, per rule, the reference matches we miss and the matches we add.
"""
import json
import sys
from collections import Counter

args = sys.argv[1:]
ref_path, our_path = args[0], args[1]
opt = lambda name, default=None: args[args.index(name) + 1] if name in args else default
classes = set(opt("--classes", "PatternRule,RegexPatternRule").split(","))
top = int(opt("--top", "40"))
only = opt("--rule")
texts_path = opt("--texts")
texts = open(texts_path, encoding="utf-8").read().split("\n") if texts_path else None

found = missing = extra = total_ref = total_ours = same_sugg = 0
miss_by = Counter()
extra_by = Counter()
ex_miss = {}
ex_extra = {}
identical_texts = n_texts = 0
for ln, (a, b) in enumerate(zip(open(ref_path, encoding="utf-8"), open(our_path, encoding="utf-8"))):
    ref = [m for m in json.loads(a) if (m[7] in classes or "all" in classes)]
    ours = json.loads(b)
    if only:
        ref = [m for m in ref if m[0] == only]
        ours = [m for m in ours if m[0] == only]
    n_texts += 1
    rk = {(m[0], m[1], m[2]): m for m in ref}
    ok = {(m[0], m[1], m[2]): m for m in ours}
    total_ref += len(rk)
    total_ours += len(ok)
    if set(rk) == set(ok):
        identical_texts += 1
    for k, m in rk.items():
        if k in ok:
            found += 1
            if m[3] == ok[k][3]:
                same_sugg += 1
        else:
            missing += 1
            miss_by[m[0]] += 1
            ex_miss.setdefault(m[0], (ln, k, m[5]))
    for k, m in ok.items():
        if k not in rk:
            extra += 1
            extra_by[m[0]] += 1
            ex_extra.setdefault(m[0], (ln, k, m[5]))
print(f"texts with identical match sets: {identical_texts}/{n_texts} ({100 * identical_texts / max(n_texts, 1):.1f}%)")
print(f"reference matches: {total_ref}; found: {found} ({100 * found / max(total_ref, 1):.1f}%), of which same suggestions: {same_sugg} ({100 * same_sugg / max(found, 1):.1f}%)")
print(f"missing: {missing}; extra (ours only): {extra} of {total_ours}")
show = lambda ln: (texts[ln][:100] if texts else f"line {ln + 1}")
print("\nmost missed rules:")
for r, n in miss_by.most_common(top):
    ln, k, full = ex_miss[r]
    print(f"{n:>6}  {r:<36} {full:<28} @{k[1]}+{k[2]}  {show(ln)!r}")
print("\nmost added rules:")
for r, n in extra_by.most_common(top):
    ln, k, full = ex_extra[r]
    print(f"{n:>6}  {r:<36} {full:<28} @{k[1]}+{k[2]}  {show(ln)!r}")
