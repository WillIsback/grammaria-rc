#!/usr/bin/env python3
"""Generate golden fixtures from LanguageTool (French) for validation.

Runs the LanguageTool command line for every line of the input file and stores
the analysis (sentences + POS tags, --taggeronly --verbose) and the grammar
matches (--json) so the Rust port can be compared against it.

Usage: python3 tools/gen_lt_fixtures.py --lt-dir /path/LanguageTool-6.6 \
          --java /path/to/java --input eval/langs_fr.txt --out eval/fixtures.json
"""
import argparse, json, os, subprocess, sys, tempfile

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--lt-dir', required=True)
    ap.add_argument('--java', required=True)
    ap.add_argument('--input', required=True)
    ap.add_argument('--out', required=True)
    a = ap.parse_args()
    cl = os.path.join(a.lt_dir, 'languagetool-commandline.jar')
    texts = [l for l in open(a.input, encoding='utf-8').read().split('\n') if l.strip()]
    out = []
    for i, t in enumerate(texts):
        with tempfile.NamedTemporaryFile('w', suffix='.txt', delete=False, encoding='utf-8') as f:
            f.write(t + '\n')
            path = f.name
        try:
            tagger = subprocess.run([a.java, '-jar', cl, '-l', 'fr', '-t', '--verbose', '--encoding', 'utf-8', path],
                                    capture_output=True, text=True, encoding='utf-8')
            js = subprocess.run([a.java, '-jar', cl, '-l', 'fr', '--json', '--encoding', 'utf-8', path],
                                capture_output=True, text=True, encoding='utf-8')
        finally:
            os.unlink(path)
        tg = '\n'.join(l for l in tagger.stdout.split('\n') if not l.startswith('Working on'))
        data = json.loads(js.stdout[js.stdout.index('{'):])
        out.append({'text': t, 'tagger': tg, 'matches': data.get('matches', []),
                    'sentences': data.get('extendedSentenceRanges', data.get('sentenceRanges', []))})
        print(f'[{i+1}/{len(texts)}] {t[:60]}', file=sys.stderr)
    json.dump(out, open(a.out, 'w', encoding='utf-8'), ensure_ascii=False)
    print(f'wrote {len(out)} fixtures to {a.out}', file=sys.stderr)

if __name__ == '__main__':
    main()
