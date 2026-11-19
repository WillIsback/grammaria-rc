#!/usr/bin/env python3
"""Compare our POS tagger against LanguageTool's output on the fixtures.

Agreement is measured token by token: a token agrees when the sets of tags
(ignoring LT-internal chunker tags such as _GN_*) match exactly.
"""
import json, re, subprocess, sys, tempfile, os

def parse_lt(s):
    """Parse a block of <S>... lines into [(word, [tags])]."""
    out = []
    for line in s.split('\n'):
        line = line.strip()
        if not line.startswith('<S>'):
            continue
        body = line[3:]
        i = 0
        while i < len(body):
            if body[i] == ' ':
                i += 1
                continue
            j = body.find('[', i)
            if j < 0:
                break
            word = body[i:j]
            k = body.find(']', j)
            if k < 0:
                break
            tags = [t.split('/')[-1] for t in body[j + 1:k].split(',') if t.strip()]
            out.append((word, tags))
            i = k + 1
    return out

def main():
    fx = json.load(open(sys.argv[1], encoding='utf-8'))
    binp = sys.argv[2]
    data = sys.argv[3] if len(sys.argv) > 3 else 'data'
    verbose = '-v' in sys.argv
    tot = ok = 0
    bad = []
    for item in fx:
        lt = parse_lt(item['tagger'])
        if not lt:
            continue
        p = subprocess.run([binp, 'check', '--tagger', '--data', data, '-'], input=item['text'], capture_output=True, text=True)
        ours = parse_lt(p.stdout)
        ltw = [w for w, _ in lt]
        ourw = [w for w, _ in ours]
        if ltw != ourw:
            bad.append((item['text'], f"TOKENIZATION\n  LT  : {ltw}\n  ours: {ourw}"))
            tot += len(ltw)
            continue
        for (w, ltt), (_, ott) in zip(lt, ours):
            tot += 1
            lts = set(ltt)
            ots = set(ott)
            if lts == ots:
                ok += 1
            else:
                bad.append((item['text'], f"  {w}: LT {sorted(lts)}  ours {sorted(ots)}"))
    print(f"POS agreement: {ok}/{tot} = {100.0*ok/max(tot,1):.1f}%")
    for text, b in bad[: (40 if verbose else 12)]:
        print(f"[{text}]\n{b}")
    return 0

if __name__ == '__main__':
    sys.exit(main())
