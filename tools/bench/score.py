#!/usr/bin/env python3
"""Scoreboard: compare checkers on the benchmark sets.

    python3 tools/bench/score.py NAME=eval/out/NAME [NAME2=...] [--split dev|test|all] [--misses NAME] [--set lt|gl|corpus]

Each NAME=PREFIX names a checker whose outputs are PREFIX.<set>.matches.jsonl
(one JSON array of matches per benchmark text, as printed by `grammaria-eval`
or `tools/bench/oracle_ltjs.mjs`).

For every set:
  recall   expected error spans touched by at least one match
  clean    texts without expected error that get no match at all
  noise    matches that touch no expected span, per 100 texts
  F1       harmonic mean of recall and clean

`--misses NAME` lists, per source rule, the expected errors NAME misses while
the first checker finds them (what to work on next).
"""
import json
import os
import sys
from collections import Counter

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SETS = ["lt", "gl", "corpus"]


def load_set(name):
    path = os.path.join(ROOT, "eval", "bench", name + ".jsonl")
    return [json.loads(l) for l in open(path, encoding="utf-8")]


def load_matches(prefix, name):
    path = f"{prefix}.{name}.matches.jsonl"
    if not os.path.exists(path):
        return None
    return [json.loads(l) for l in open(path, encoding="utf-8")]


def touches(m, span):
    a, b = m[1], m[1] + max(m[2], 1)
    return a < span[1] and span[0] < b


def score(items, matches, split):
    hit = total = clean = clean_total = noise = texts = 0
    for it, ms in zip(items, matches):
        if split != "all" and it["split"] != split:
            continue
        texts += 1
        if it["spans"]:
            for sp in it["spans"]:
                total += 1
                if any(touches(m, sp) for m in ms):
                    hit += 1
            noise += sum(1 for m in ms if not any(touches(m, sp) for sp in it["spans"]))
        else:
            clean_total += 1
            if not ms:
                clean += 1
            noise += len(ms)
    r = hit / total if total else 0.0
    c = clean / clean_total if clean_total else 0.0
    f1 = 2 * r * c / (r + c) if r + c else 0.0
    return {"hit": hit, "total": total, "recall": r, "clean": clean, "clean_total": clean_total, "clean_rate": c,
            "noise": 100.0 * noise / texts if texts else 0.0, "f1": f1}


def main():
    args = sys.argv[1:]
    split = "all"
    misses = None
    only = None
    checkers = []
    i = 0
    while i < len(args):
        a = args[i]
        if a == "--split":
            split = args[i + 1]
            i += 2
        elif a == "--misses":
            misses = args[i + 1]
            i += 2
        elif a == "--set":
            only = args[i + 1]
            i += 2
        else:
            name, prefix = a.split("=", 1)
            checkers.append((name, prefix))
            i += 1
    if not checkers:
        print(__doc__)
        sys.exit(2)
    print(f"split: {split}")
    totals = {name: [0, 0, 0, 0] for name, _ in checkers}
    for s in SETS:
        if only and s != only:
            continue
        items = load_set(s)
        print(f"\n== {s} ==")
        print(f"{'checker':<16}{'recall':>18}{'clean':>18}{'noise/100':>11}{'F1':>8}")
        loaded = {}
        for name, prefix in checkers:
            ms = load_matches(prefix, s)
            if ms is None:
                print(f"{name:<16}  (no output)")
                continue
            if len(ms) != len(items):
                print(f"{name:<16}  ({len(ms)} lines for {len(items)} texts: stale output)")
                continue
            loaded[name] = ms
            sc = score(items, ms, split)
            t = totals[name]
            t[0] += sc["hit"]; t[1] += sc["total"]; t[2] += sc["clean"]; t[3] += sc["clean_total"]
            print(f"{name:<16}{sc['hit']:>6}/{sc['total']:<5}{100 * sc['recall']:>5.1f}%"
                  f"{sc['clean']:>6}/{sc['clean_total']:<5}{100 * sc['clean_rate']:>5.1f}%{sc['noise']:>11.1f}{100 * sc['f1']:>8.1f}")
        if misses and misses in loaded and checkers[0][0] in loaded and checkers[0][0] != misses:
            ref = loaded[checkers[0][0]]
            mine = loaded[misses]
            by_rule = Counter()
            examples = {}
            for it, r, m in zip(items, ref, mine):
                if split != "all" and it["split"] != split:
                    continue
                for sp in it["spans"]:
                    ref_hits = [x for x in r if touches(x, sp)]
                    if ref_hits and not any(touches(x, sp) for x in m):
                        key = ref_hits[0][0]
                        by_rule[key] += 1
                        examples.setdefault(key, it["text"])
            print(f"\n  errors found by {checkers[0][0]} and missed by {misses}, by rule of {checkers[0][0]}:")
            for rule, n in by_rule.most_common(40):
                print(f"  {n:>5}  {rule:<40} {examples[rule][:90]!r}")
    print("\n== all sets ==")
    for name, _ in checkers:
        h, t, c, ct = totals[name]
        if t == 0:
            continue
        r = h / t
        cr = c / ct if ct else 0
        print(f"{name:<16}{h:>6}/{t:<5}{100 * r:>5.1f}%{c:>6}/{ct:<5}{100 * cr:>5.1f}%{'':>11}{100 * 2 * r * cr / (r + cr):>8.1f}")


if __name__ == "__main__":
    main()
