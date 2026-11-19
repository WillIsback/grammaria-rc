#!/usr/bin/env python3
"""Rule-level evaluation of Grammaria against LanguageTool's own rule examples.

For every example with a marker span, the marked text is the expected error.
We count the example as detected when our JSON output has a match whose range
overlaps the marker (for "corrected" examples) or fire when it should not
(negative examples have no markers).
"""
import json, subprocess, sys

def main():
    corpus = json.load(open(sys.argv[1], encoding='utf-8'))
    binp = sys.argv[2]
    data = sys.argv[3] if len(sys.argv) > 3 else 'data'
    limit = int(sys.argv[4]) if len(sys.argv) > 4 else 0
    n_pos = n_hit = 0
    n_neg = n_false = 0
    by_rule_hit = {}
    by_rule_tot = {}
    for i, ex in enumerate(corpus):
        if limit and i >= limit:
            break
        text = ex['text']
        p = subprocess.run([binp, 'check', '--json', '--data', data, '-'], input=text, capture_output=True, text=True)
        try:
            d = json.loads(p.stdout[p.stdout.index('{'):])
        except Exception:
            continue
        matches = d.get('matches', [])
        markers = ex['markers']
        if markers:
            n_pos += 1
            rule = ex['rule']
            by_rule_tot[rule] = by_rule_tot.get(rule, 0) + 1
            hit = False
            for m in matches:
                ms, me = m['offset'], m['offset'] + m['length']
                for (a, b) in markers:
                    if ms < b and me > a:
                        hit = True
            if hit:
                n_hit += 1
                by_rule_hit[rule] = by_rule_hit.get(rule, 0) + 1
        else:
            n_neg += 1
            if matches:
                n_false += 1
    print(f'positive examples detected: {n_hit}/{n_pos} = {100.0*n_hit/max(n_pos,1):.1f}%')
    print(f'negative examples with false positives: {n_false}/{n_neg} = {100.0*n_false/max(n_neg,1):.1f}%')
    worst = sorted(by_rule_tot.items(), key=lambda kv: -(kv[1] - by_rule_hit.get(kv[0], 0)))[:15]
    print('rules with most misses:', [(r, by_rule_hit.get(r, 0), t) for r, t in worst])

if __name__ == '__main__':
    main()
