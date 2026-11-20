#!/usr/bin/env python3
"""Word/stem pairs for the stem encoder comparison (see README.md here).

usage: make_encoder_pairs.py <repo root> > pairs.tsv
"""
import sys

root = sys.argv[1]
pairs = []
with open(root + "/assets/fr/languagetool-pos-dict.txt", encoding="utf-8") as f:
    for i, line in enumerate(f):
        if i % 1500 == 0:
            form, lemma, _ = line.rstrip("\n").split("\t")
            pairs.append((form, lemma))
            # The reverse direction is what a synthesis dictionary stores.
            pairs.append((lemma, form))
pairs += [
    ("suis", "être"), ("yeux", "œil"), ("gemacht", "machen"), ("abgemacht", "abmachen"),
    ("najlepszy", "dobry"), ("nieładny", "ładny"), ("abc", "abc"), ("abc", ""), ("", "abc"),
    ("a", "b"), ("ab", "ba"), ("xaby", "ab"), ("aaaa", "aa"), ("été", "être"), ("œufs", "œuf"),
    ("x" * 300, "x" * 299 + "y"), ("y" + "x" * 300, "x" * 300), ("x" * 300, "z"),
    ("a" * 254 + "bc", "a" * 254), ("a" * 255 + "bc", "a" * 255), ("b" * 260 + "abc", "abc"),
]
for w, s in pairs:
    print(w + "\t" + s)
