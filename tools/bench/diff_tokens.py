#!/usr/bin/env python3
"""Compare two token dumps, text by text.

    diff_tokens.py REF OURS [--examples N] [--top N] [--class NAME] [--list]

REF and OURS are JSON-lines files in the shape of `oracle_ltjs.mjs raw|tokens`
and `grammaria-eval --raw`: one line per text, a list of sentences, each a list
of tokens `["token", [["lemma", "POSTAG"], ...], flags, ["chunk tag", ...]]`.

Prints how many texts are identical and, for the others, the first difference
of each text, classified:

    sentence split    the sentences do not cover the same stretches of text
    token sequence    same sentences, different tokens
    readings (set)    same tokens, a token has different readings
    readings (count)  same readings, some repeated a different number of times
    reading order     same readings, in a different order
    flags             immunized / ignored by the speller / whitespace before
    chunk tags

with the number of texts per class, the tokens most often involved and a few
examples. `--class NAME` restricts the examples to one class (any unambiguous
prefix), `--list` prints the line numbers of the differing texts per class.
The exit status is 0 when the files are identical, 1 otherwise.
"""
import argparse
import collections
import json
import sys

CLASSES = ["sentence split", "token sequence", "readings (set)", "readings (count)", "reading order", "flags", "chunk tags"]
FLAG_NAMES = [(1, "immunized"), (2, "ignored-by-speller"), (4, "whitespace-before")]


def show_token(text):
    return json.dumps(text, ensure_ascii=False)


def show_readings(readings):
    return " ".join(f"{'∅' if lemma is None else lemma}/{'∅' if tag is None else tag}" for lemma, tag in readings) or "(none)"


def context(tokens, i, width=4):
    lo = max(0, i - width)
    return "".join(t[0] for t in tokens[lo:i]) + "⟦" + tokens[i][0] + "⟧" + "".join(t[0] for t in tokens[i + 1:i + 1 + width])


def first_difference(ref, ours):
    """(class, key, detail) for the first difference between two texts, or None."""
    ref_sentences = ["".join(t[0] for t in s) for s in ref]
    our_sentences = ["".join(t[0] for t in s) for s in ours]
    if ref_sentences != our_sentences:
        i = 0
        while i < min(len(ref_sentences), len(our_sentences)) and ref_sentences[i] == our_sentences[i]:
            i += 1
        r = ref_sentences[i] if i < len(ref_sentences) else None
        o = our_sentences[i] if i < len(our_sentences) else None
        if "".join(ref_sentences) != "".join(our_sentences):
            key = "(the texts differ)"
        elif r is not None and o is not None:
            # the end of the shorter sentence is where one side cut and the other did not
            shorter, side = (r, "ref cuts") if len(r) < len(o) else (o, "ours cuts")
            key = f"{side} after {show_token(shorter[-12:])}"
        else:
            key = "(sentence count)"
        return ("sentence split", key, f"sentence {i}:\n      ref  {show_token(r)}\n      ours {show_token(o)}")
    for si, (rs, os_) in enumerate(zip(ref, ours)):
        r_tokens = [t[0] for t in rs]
        o_tokens = [t[0] for t in os_]
        if r_tokens != o_tokens:
            i = 0
            while i < min(len(r_tokens), len(o_tokens)) and r_tokens[i] == o_tokens[i]:
                i += 1
            r_tail = "|".join(r_tokens[i:i + 5])
            o_tail = "|".join(o_tokens[i:i + 5])
            key = f"{show_token(r_tokens[i] if i < len(r_tokens) else '')} vs {show_token(o_tokens[i] if i < len(o_tokens) else '')}"
            before = "".join(r_tokens[max(0, i - 4):i])
            return ("token sequence", key, f"sentence {si}, token {i}, after {show_token(before)}:\n      ref  {r_tail}\n      ours {o_tail}")
    for kind in ("readings", "flags", "chunks"):
        for si, (rs, os_) in enumerate(zip(ref, ours)):
            for i, (rt, ot) in enumerate(zip(rs, os_)):
                where = f"sentence {si}, token {i}: {context(rs, i)}"
                if kind == "readings":
                    rr = [tuple(x) for x in rt[1]]
                    orr = [tuple(x) for x in ot[1]]
                    if rr == orr:
                        continue
                    if set(rr) != set(orr):
                        only_ref = [x for x in rr if x not in orr]
                        only_ours = [x for x in orr if x not in rr]
                        detail = f"{where}\n      only ref  {show_readings(only_ref)}\n      only ours {show_readings(only_ours)}"
                        return ("readings (set)", show_token(rt[0]), detail)
                    name = "readings (count)" if sorted(rr, key=repr) != sorted(orr, key=repr) else "reading order"
                    return (name, show_token(rt[0]), f"{where}\n      ref  {show_readings(rr)}\n      ours {show_readings(orr)}")
                if kind == "flags" and rt[2] != ot[2]:
                    changed = [name for bit, name in FLAG_NAMES if (rt[2] ^ ot[2]) & bit]
                    return ("flags", f"{'+'.join(changed)} {show_token(rt[0])}", f"{where}\n      ref  {rt[2]}\n      ours {ot[2]}")
                if kind == "chunks" and rt[3] != ot[3]:
                    return ("chunk tags", show_token(rt[0]), f"{where}\n      ref  {rt[3]}\n      ours {ot[3]}")
    return ("token sequence", "(unclassified)", "the texts differ in a way this script does not classify")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("ref")
    ap.add_argument("ours")
    ap.add_argument("--examples", type=int, default=3, help="examples printed per class (default 3)")
    ap.add_argument("--top", type=int, default=10, help="most frequent offending tokens printed per class (default 10)")
    ap.add_argument("--class", dest="only", help="only print the examples of this class")
    ap.add_argument("--list", action="store_true", help="print the line numbers of the differing texts")
    args = ap.parse_args()

    with open(args.ref, encoding="utf-8") as f:
        ref_lines = f.read().split("\n")
    with open(args.ours, encoding="utf-8") as f:
        our_lines = f.read().split("\n")
    for lines in (ref_lines, our_lines):
        if lines and lines[-1] == "":
            lines.pop()
    if len(ref_lines) != len(our_lines):
        print(f"warning: {len(ref_lines)} texts in REF, {len(our_lines)} in OURS; comparing the common prefix", file=sys.stderr)

    total = min(len(ref_lines), len(our_lines))
    identical = 0
    by_class = collections.defaultdict(list)
    for n in range(total):
        if ref_lines[n] == our_lines[n]:
            identical += 1
            continue
        ref, ours = json.loads(ref_lines[n]), json.loads(our_lines[n])
        if ref == ours:
            identical += 1
            continue
        cls, key, detail = first_difference(ref, ours)
        by_class[cls].append((n + 1, key, detail))

    pct = 100.0 * identical / total if total else 100.0
    print(f"{identical} / {total} texts identical ({pct:.3f} %), {total - identical} differ")
    for cls in CLASSES:
        items = by_class.get(cls)
        if not items:
            continue
        if args.only and not cls.startswith(args.only):
            continue
        print(f"\n{cls}: {len(items)} texts")
        counts = collections.Counter(key for _, key, _ in items)
        for key, count in counts.most_common(args.top):
            print(f"  {count:6d}  {key}")
        if len(counts) > args.top:
            print(f"          ... {len(counts) - args.top} more")
        for line, _, detail in items[:args.examples]:
            print(f"  line {line}: {detail}")
        if args.list:
            print("  lines: " + " ".join(str(line) for line, _, _ in items))
    return 0 if identical == total and len(ref_lines) == len(our_lines) else 1


if __name__ == "__main__":
    sys.exit(main())
