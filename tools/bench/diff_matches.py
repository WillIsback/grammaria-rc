#!/usr/bin/env python3
"""Compare rule matches with the reference, rule by rule.

    python3 tools/bench/diff_matches.py REF.matches.jsonl OURS.matches.jsonl [--classes PatternRule,RegexPatternRule]
        [--top N] [--rule ID] [--ids PREFIX,PREFIX] [--list N] [--texts TEXTS.txt]

A reference match is "found" when we report the same rule id on the same
span. Lists, per rule, the reference matches we miss and the matches we add.

--classes  reference rule classes to keep (`all` for every class)
--rule     keep one rule id, on both sides
--ids      keep the rule ids that start with one of the prefixes, on both
           sides, and print a table per prefix (found, same suggestions, extra)
--list     print up to N missed and N added matches of each rule
--texts    the checked texts (one per line), to show the match in context
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
prefixes = [p for p in (opt("--ids") or "").split(",") if p]
list_n = int(opt("--list", "0"))
texts_path = opt("--texts")
texts = open(texts_path, encoding="utf-8").read().split("\n") if texts_path else None


def family(rule_id):
    """The --ids prefix a rule id is counted under."""
    return next((p for p in prefixes if rule_id.startswith(p)), None)


def context(ln, offset, length):
    """The match in its text, between brackets."""
    if not texts:
        return f"line {ln + 1}"
    text = texts[ln].replace("\\n", "\n")
    return repr(text[max(0, offset - 30):offset] + "[" + text[offset:offset + length] + "]" + text[offset + length:offset + length + 30])


found = missing = extra = total_ref = total_ours = same_sugg = 0
miss_by = Counter()
extra_by = Counter()
ex_miss = {}
ex_extra = {}
identical_texts = n_texts = 0
by_family = {p: Counter() for p in prefixes}
listed = {}
for ln, (a, b) in enumerate(zip(open(ref_path, encoding="utf-8"), open(our_path, encoding="utf-8"))):
    ref = [m for m in json.loads(a) if (m[7] in classes or "all" in classes)]
    ours = json.loads(b)
    if only:
        ref = [m for m in ref if m[0] == only]
        ours = [m for m in ours if m[0] == only]
    if prefixes:
        ref = [m for m in ref if family(m[0])]
        ours = [m for m in ours if family(m[0])]
    n_texts += 1
    rk = {(m[0], m[1], m[2]): m for m in ref}
    ok = {(m[0], m[1], m[2]): m for m in ours}
    total_ref += len(rk)
    total_ours += len(ok)
    if set(rk) == set(ok):
        identical_texts += 1
    for k, m in rk.items():
        stats = by_family.get(family(m[0]), Counter())
        stats["ref"] += 1
        if k in ok:
            found += 1
            stats["found"] += 1
            if m[3] == ok[k][3]:
                same_sugg += 1
                stats["same"] += 1
            elif list_n:
                listed.setdefault(("suggestions differ", m[0]), []).append((ln, k, f"{m[3]} != {ok[k][3]}"))
        else:
            missing += 1
            miss_by[m[0]] += 1
            ex_miss.setdefault(m[0], (ln, k, m[5]))
            if list_n:
                listed.setdefault(("missed", m[0]), []).append((ln, k, str(m[3])))
    for k, m in ok.items():
        if k not in rk:
            extra += 1
            extra_by[m[0]] += 1
            by_family.get(family(m[0]), Counter())["extra"] += 1
            ex_extra.setdefault(m[0], (ln, k, m[5]))
            if list_n:
                listed.setdefault(("added", m[0]), []).append((ln, k, str(m[3])))
print(f"texts with identical match sets: {identical_texts}/{n_texts} ({100 * identical_texts / max(n_texts, 1):.1f}%)")
print(f"reference matches: {total_ref}; found: {found} ({100 * found / max(total_ref, 1):.1f}%), of which same suggestions: {same_sugg} ({100 * same_sugg / max(found, 1):.1f}%)")
print(f"missing: {missing}; extra (ours only): {extra} of {total_ours}")
if prefixes:
    print(f"\n{'rule ids':<32} {'reference':>9} {'found':>7} {'%':>6} {'same sugg.':>10} {'extra':>6}")
    for p in prefixes:
        s = by_family[p]
        print(f"{p + '*':<32} {s['ref']:>9} {s['found']:>7} {100 * s['found'] / max(s['ref'], 1):>6.1f} {s['same']:>10} {s['extra']:>6}")
show = lambda ln: (texts[ln][:100] if texts else f"line {ln + 1}")
print("\nmost missed rules:")
for r, n in miss_by.most_common(top):
    ln, k, full = ex_miss[r]
    print(f"{n:>6}  {r:<36} {full:<28} @{k[1]}+{k[2]}  {show(ln)!r}")
print("\nmost added rules:")
for r, n in extra_by.most_common(top):
    ln, k, full = ex_extra[r]
    print(f"{n:>6}  {r:<36} {full:<28} @{k[1]}+{k[2]}  {show(ln)!r}")
for (kind, rule), items in sorted(listed.items()):
    print(f"\n{kind}: {rule} ({len(items)})")
    for ln, k, detail in items[:list_n]:
        print(f"  line {ln + 1} @{k[1]}+{k[2]} {detail}  {context(ln, k[1], k[2])}")
