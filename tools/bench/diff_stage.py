#!/usr/bin/env python3
"""Compare two token dumps (reference first) token by token.

    python3 tools/bench/diff_stage.py REF.jsonl OURS.jsonl [--examples N]

Reports identical texts and tokens, then the differences grouped by kind
(readings, flags, chunk tags, structure) with the most frequent ones.
"""
import json
import sys
from collections import Counter

ref_path, our_path = sys.argv[1], sys.argv[2]
n_examples = int(sys.argv[sys.argv.index("--examples") + 1]) if "--examples" in sys.argv else 15
texts = same_texts = tokens = same_tokens = 0
kinds = Counter()
detail = Counter()
examples = {}
for ln, (a, b) in enumerate(zip(open(ref_path, encoding="utf-8"), open(our_path, encoding="utf-8"))):
    texts += 1
    if a == b:
        same_texts += 1
        tokens += sum(len(s) for s in json.loads(a))
        same_tokens += sum(len(s) for s in json.loads(a))
        continue
    x, y = json.loads(a), json.loads(b)
    if len(x) != len(y) or any(len(sx) != len(sy) for sx, sy in zip(x, y)):
        kinds["structure"] += 1
        examples.setdefault("structure", []).append((ln, "", "", ""))
        continue
    for sx, sy in zip(x, y):
        sent = "".join(t[0] for t in sx)
        for tx, ty in zip(sx, sy):
            tokens += 1
            if tx == ty:
                same_tokens += 1
                continue
            if tx[1] != ty[1]:
                rs = {tuple(r) for r in tx[1]}
                os_ = {tuple(r) for r in ty[1]}
                if rs == os_:
                    kind = "reading order/duplicates"
                else:
                    kind = "readings"
                    for r in rs - os_:
                        detail["missing " + str(r[1])] += 1
                    for r in os_ - rs:
                        detail["extra   " + str(r[1])] += 1
            elif tx[2] != ty[2]:
                kind = f"flags {tx[2]}->{ty[2]}"
            else:
                kind = "chunk tags"
            kinds[kind] += 1
            examples.setdefault(kind, []).append((ln, sent.strip()[:110], json.dumps(tx[1:], ensure_ascii=False), json.dumps(ty[1:], ensure_ascii=False)))
print(f"texts identical: {same_texts}/{texts} ({100 * same_texts / max(texts, 1):.2f}%)")
print(f"tokens identical: {same_tokens}/{tokens} ({100 * same_tokens / max(tokens, 1):.3f}%)")
for kind, n in kinds.most_common():
    print(f"\n{n:>6}  {kind}")
    for ln, sent, r, o in examples[kind][:n_examples]:
        print(f"        line {ln + 1}: {sent}\n          ref {r[:230]}\n          new {o[:230]}")
if detail:
    print("\nmost frequent reading differences:")
    for d, n in detail.most_common(30):
        print(f"{n:>6}  {d}")
