#!/usr/bin/env python3
"""Build the benchmark sets used to compare Grammaria with other checkers.

Three independent sources, one JSON object per line in `eval/bench/<name>.jsonl`:

  lt        LanguageTool's own rule examples (`eval/lt_examples_fr.json`): the
            marked span is an error LanguageTool's authors expect to be found;
            unmarked examples are sentences they consider clean for that rule.
  gl        Grammalecte's test sentences (`TEST:` lines of `rules.grx`):
            `{{...}}` marks an expected error, lines without it must stay clean.
            Tests bound to a non-default option (`__ocr__`, ...) are left out.
  corpus    Texts with a hand-written correction (`eval/corpus_fr.json`); the
            spans are the words the correction changes.

Fields: `id`, `text`, `spans` (code-point `[start, end)` of expected errors),
`rule` (source rule when known), `split` (`dev` or `test`: even/odd position,
so that tuning on `dev` can be checked on `test`).

`eval/bench/<name>.txt` holds the same texts, one per line, with `\\`, newline
and tab escaped: the input of `grammaria-eval` and of the oracle scripts.
"""
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "eval", "bench")


def escape(text):
    return text.replace("\\", "\\\\").replace("\n", "\\n").replace("\t", "\\t").replace("\r", "\\r")


def write(name, items):
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, name + ".jsonl"), "w", encoding="utf-8") as jf, open(
        os.path.join(OUT, name + ".txt"), "w", encoding="utf-8"
    ) as tf:
        for i, it in enumerate(items):
            it = {"id": f"{name}-{i}", **it, "split": "dev" if i % 2 == 0 else "test"}
            jf.write(json.dumps(it, ensure_ascii=False) + "\n")
            tf.write(escape(it["text"]) + "\n")
    pos = sum(1 for it in items if it["spans"])
    print(f"{name}: {len(items)} texts, {pos} with expected errors, {len(items) - pos} clean")


def lt_examples():
    src = json.load(open(os.path.join(ROOT, "eval", "lt_examples_fr.json"), encoding="utf-8"))
    items = []
    for ex in src:
        raw = ex["text"]
        text = raw.rstrip()
        lead = len(raw) - len(raw.lstrip())
        text = text.lstrip()
        spans = [[max(0, a - lead), max(0, b - lead)] for a, b in ex["markers"]]
        spans = [s for s in spans if s[1] > s[0] and s[1] <= len(text)]
        if ex["markers"] and not spans:
            continue
        if not text:
            continue
        items.append({"text": text, "spans": spans, "rule": ex["rule"]})
    return items


TEST_RE = re.compile(r"^TEST:\s?(.*)$")
OPTION_RE = re.compile(r"^__([a-z0-9_]+)__\s+")


def grammalecte_tests():
    path = os.path.join(ROOT, "eval", "sources", "grammalecte_rules.grx")
    items = []
    rule = ""
    seen = set()
    for line in open(path, encoding="utf-8"):
        line = line.rstrip("\n")
        if line.startswith("__") and not line.startswith("__<") and not line.startswith("__["):
            rule = line.strip().strip("_").split("(")[0]
        elif line.startswith("__"):
            m = re.search(r"\(([^)]+)\)", line)
            rule = m.group(1) if m else rule
        m = TEST_RE.match(line)
        if not m:
            continue
        body = m.group(1)
        if OPTION_RE.match(body):
            continue  # needs a non-default option
        body = body.split("->>")[0].rstrip()
        # Unescape the markup: {{error}} spans.
        text = ""
        spans = []
        pos = 0
        ok = True
        while True:
            a = body.find("{{", pos)
            if a < 0:
                text += body[pos:]
                break
            b = body.find("}}", a)
            if b < 0:
                ok = False
                break
            text += body[pos:a]
            start = len(text)
            text += body[a + 2 : b]
            spans.append([start, len(text)])
            pos = b + 2
        if not ok or not text.strip():
            continue
        spans = [s for s in spans if s[1] > s[0]]
        if "{{" in body and not spans:
            continue
        if text in seen:
            continue
        seen.add(text)
        items.append({"text": text, "spans": spans, "rule": rule})
    return items


def corpus():
    path = os.path.join(ROOT, "eval", "corpus_fr.json")
    if not os.path.exists(path):
        return None
    return [
        {"text": t["text"], "spans": t["spans"], "rule": t["name"], "corrected": t["corrected"]}
        for t in json.load(open(path, encoding="utf-8"))
    ]


def main():
    write("lt", lt_examples())
    write("gl", grammalecte_tests())
    c = corpus()
    if c is not None:
        write("corpus", c)
    else:
        print("corpus: eval/corpus_fr.json missing, skipped", file=sys.stderr)


if __name__ == "__main__":
    main()
