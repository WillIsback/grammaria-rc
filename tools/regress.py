#!/usr/bin/env python3
"""Behavioural regression check for Grammaria.

Runs a frozen set of French sentences through the CLI and compares the exact
matches (rule id, offset, length, replacements) against a recorded baseline.

Use it before and after any change to rule semantics, patterns or the
dictionary: memory/size work must never alter what the checker reports.

    # record a baseline (after an intentional behaviour change)
    python3 tools/regress.py --write

    # verify against it
    python3 tools/regress.py
"""
import argparse
import json
import subprocess
import sys

BASELINE = "eval/regression_fr.json"

# A mix of real errors, correct text, and the cases that were false positives
# during development (determiner agreement, tout/tous, relative "que",
# capitalisation of pronouns, subject+auxiliary).
CASES = [
    "Cette événement est important.",
    "La toit fuit.",
    "Les voiture sont belles.",
    "Tout les enfants jouent.",
    "Toute les hommes sont grands.",
    "Tous les années sont bonnes.",
    "Il faut que je peux le faire.",
    "Voilà pourquoi Je convie tous les Français.",
    "Les femmes que j'ai aidé.",
    "Je vous envoie les documents ci-joint.",
    "Le chat mange la souris.",
    "Il faut que grand carre soit à gauche.",
    "Une homme est entré il y a cinq minutes.",
    "Les enfants joues dans le jardin.",
    "La femme que j'ai aidée.",
    "Sacha le rencontre alors qu'il parle.",
]


def run(binary, data, text):
    p = subprocess.run(
        [binary, "check", "--json", "--data", data, "-"],
        input=text,
        capture_output=True,
        text=True,
    )
    try:
        doc = json.loads(p.stdout[p.stdout.index("{"):])
    except Exception:
        return None
    return [
        [m["rule"]["id"], m["offset"], m["length"], m.get("replacements")]
        for m in doc.get("matches", [])
    ]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", default="./target/release/grammaria")
    ap.add_argument("--data", default="data")
    ap.add_argument("--write", action="store_true", help="record the baseline")
    a = ap.parse_args()

    got = []
    for text in CASES:
        matches = run(a.binary, a.data, text)
        if matches is None:
            print(f"FAILED to run on {text!r}", file=sys.stderr)
            return 2
        got.append({"text": text, "matches": matches})

    if a.write:
        with open(BASELINE, "w", encoding="utf-8") as f:
            json.dump(got, f, ensure_ascii=False, indent=1)
        total = sum(len(c["matches"]) for c in got)
        print(f"baseline written: {len(got)} cases, {total} matches -> {BASELINE}")
        return 0

    try:
        with open(BASELINE, encoding="utf-8") as f:
            want = json.load(f)
    except FileNotFoundError:
        print(f"{BASELINE} missing; create it with --write", file=sys.stderr)
        return 2

    bad = 0
    for exp, cur in zip(want, got):
        if exp["matches"] != cur["matches"]:
            bad += 1
            print(f"REGRESSION {exp['text'][:50]!r}")
            print(f"  expected {exp['matches']}")
            print(f"  got      {cur['matches']}")
    ok = len(want) - bad
    print(f"{ok}/{len(want)} cases identical" + ("  OK" if bad == 0 else "  FAILED"))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
