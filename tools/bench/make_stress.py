#!/usr/bin/env python3
"""Texts that exercise the rules LanguageTool writes in code (`src/engine/native/`).

    python3 tools/bench/make_stress.py N SEED eval/bench/lt.txt eval/bench/gl.txt assets/fr/lt > stress.txt

Each output line is one text (line breaks escaped as backslash-n): a few
benchmark sentences with typographic accidents sprinkled in (spaces around
punctuation, brackets, lower-case sentence starts, compounds written apart,
words of `replace.txt`, repeated words of `synonyms.txt`). The benchmark sets
have few matches of some of these rules; run the reference on the output
(`oracle_ltjs.mjs tokens` and `matches`), then `grammaria-stage match` and
`diff_matches.py --classes all --ids ...`.
"""
import random
import re
import sys

n, seed, lt_path, gl_path, assets = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3], sys.argv[4], sys.argv[5]
rnd = random.Random(seed)
lines = [l for p in (lt_path, gl_path) for l in open(p, encoding="utf-8").read().split("\n") if 15 < len(l) < 160 and "\\" not in l]

compounds = []
for l in open(assets + "/compounds.txt", encoding="utf-8"):
    l = l.split("#")[0].strip()
    if l:
        compounds.append(l.rstrip("+*?$"))
wrong = []
for name in ("replace.txt", "replace_custom.txt"):
    for l in open(assets + "/" + name, encoding="utf-8"):
        l = l.rstrip("\n")
        if l and not l.startswith("#") and "=" in l:
            wrong += l.split("=")[0].split("|")
synonyms = []
for l in open(assets + "/synonyms.txt", encoding="utf-8"):
    l = l.split("#")[0].strip()
    if l:
        head = l.split("=")[0].split("/")[0]
        synonyms += [head] if "=" in l else l.split(";")
synonym_forms = {"facile": ["facile", "faciles"], "génial": ["génial", "géniale", "géniaux"], "bouger": ["bouge", "bougent", "bouger", "bougeait"],
                 "démarrer": ["démarre", "démarrer", "démarrent"], "diminuer": ["diminue", "diminuer"], "déçu": ["déçu", "déçue", "déçus"]}


def cap(s):
    return s[:1].upper() + s[1:]


def low(s):
    return s[:1].lower() + s[1:]


def words_edit(s, fn, p):
    parts = s.split(" ")
    return " ".join(fn(w) if rnd.random() < p else w for w in parts)


def compound_variant():
    c = rnd.choice(compounds)
    k = rnd.random()
    if k < 0.45:
        c = c.replace("-", " ")
    elif k < 0.6:
        c = c.replace("-", " ", 1)
    elif k < 0.7:
        c = c.replace("-", "")
    elif k < 0.75:
        c = c.replace("-", " - ")
    elif k < 0.8:
        c = c.replace("-", "--")
    k = rnd.random()
    if k < 0.15:
        c = cap(c)
    elif k < 0.25:
        c = c.upper()
    elif k < 0.3:
        c = c.title()
    return c


def insert_word(s, w):
    parts = s.split(" ")
    i = rnd.randrange(len(parts) + 1)
    return " ".join(parts[:i] + [w] + parts[i:])


PUNCT_EDITS = [
    (", ", [" ,", ",", ",,", " , ", ", , ", ",\u00a0", " ,,"]),
    (". ", [" . ", ".. ", ".", "...", " .", ".\u00a0", ".  ", ". . "]),
    ("? ", ["? ", " ? ", "?? ", "?! ", " ?", "\u202f? ", "\u00a0? ", "?"]),
    ("! ", ["! ", " ! ", "!! ", "!? ", "\u202f! ", " !!! "]),
    (": ", [": ", " : ", ":", "\u00a0: ", " :"]),
    ("; ", ["; ", " ; ", ";", "\u202f; "]),
    (" ", ["  ", " \u00a0", "\u00a0 ", "\t", " \t ", "   ", "\u00a0", "\u202f", "\n", "\n  ", " \n", "\u200b ", "\n\t  "]),
    ("(", ["( ", "(", " (", "[", "{", "(("]),
    (")", [" )", ")", ") ", "]", "}", "))", ""]),
    ("«", ["« ", "«", "«\u00a0", "\""]),
    ("»", [" »", "»", "\u00a0»", "\""]),
]
INSERTS = [":-)", ":)", ";D", ":-(", "(", ")", "[", "]", "{", "}", "a)", "b)", "1)", "1.", "12.", "iv.", "(1)", "(a)", "«", "»", "« oui »", "«oui»",
           "23:20", "à 8:05", "00:1a:2b:3c", "a;b;c", "http://exemple.fr/a(b)", "www.exemple.fr", "jean@exemple.fr", "exemple.fr/page", "..", ",,",
           "...", "…", "!!", "?", "!", ";", ":", "- [ ]", "(voir .txt)", "fichier .pdf", "site .com", "3,5", "1 , 5", "pH", "iPhone", "eBay", "x86",
           "c.-à-d.", "etc.", "M.", "n°", "§", "—", "–", "-", "'", "’", "\"", "“", "”", "‘"]
JOINERS = ["", "", "!", "?", "…", ".»", ".\u00a0", ".\u202f", ". \u00a0", ".   ", "!  ", "?   ", ") ", " (", "] ", " [", "} ", " {", ".) ", ". (", " ", " ", " ", "\n", "\n", "\n\n", "", "  ", "\n ", " \n", "\u00a0", ".", ", ", "; ", " - ", "\n- ", "\n1. ", "\n2) ", "\na) ", "\r\n", "\t"]


def mutate(s):
    k = rnd.random()
    if k < 0.3:
        s = low(s)
    elif k < 0.33:
        s = s.upper()
    for _ in range(rnd.choice([0, 0, 1, 1, 2, 3])):
        k = rnd.random()
        if k < 0.3:
            old, news = rnd.choice(PUNCT_EDITS)
            places = [m.start() for m in re.finditer(re.escape(old), s)]
            if places:
                p = rnd.choice(places)
                s = s[:p] + rnd.choice(news) + s[p + len(old):]
        elif k < 0.55:
            s = insert_word(s, rnd.choice(INSERTS))
        elif k < 0.7:
            s = insert_word(s, compound_variant())
        elif k < 0.78:
            w = rnd.choice(wrong)
            w = rnd.choice([w, w, cap(w), w.upper()])
            s = insert_word(s, w)
        elif k < 0.86:
            w = rnd.choice(synonyms)
            w = rnd.choice(synonym_forms.get(w, [w]))
            s = insert_word(s, rnd.choice([w, w, w, cap(w), w.upper()]))
        elif k < 0.9:
            s = s.rstrip(".!?") + rnd.choice([",", ";", ":", "", " ", "…", ".", "?", "!", " ?", "\u00a0!", ".)", ". »", "\"", " »", "."])
        elif k < 0.95:
            s = rnd.choice(["« ", "«", "\"", "'", "(", "- ", "– ", "¿", "1. ", "a) ", "… ", " ", "\u00a0", "* "]) + s
        else:
            s = words_edit(s, lambda w: w.replace("-", " "), 0.5)
    return s


for _ in range(n):
    count = rnd.choice([1, 1, 2, 2, 3, 4, 6])
    text = ""
    for i in range(count):
        s = mutate(rnd.choice(lines))
        if i:
            text += rnd.choice(JOINERS)
        text += s
    # A repeated synonym a few sentences apart.
    if rnd.random() < 0.25:
        w = rnd.choice(synonyms)
        a, b = rnd.choice(synonym_forms.get(w, [w])), rnd.choice(synonym_forms.get(w, [w]))
        text = f"Il est {a} de le dire. " + text + rnd.choice([" ", "\n", "\n\n"]) + f"C'est {cap(b) if rnd.random() < 0.2 else b} et vrai" + rnd.choice([".", ".", " !", "", " ?"])
    print(text.replace("\\", "\\\\").replace("\n", "\\n").replace("\t", "\\t").replace("\r", "\\r"))
