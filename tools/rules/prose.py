#!/usr/bin/env python3
"""False alarms of a rule file on well-edited prose.

    python3 tools/rules/prose.py FILE.xml [FILE2.xml ...] [--prose DIR] [--show N] [--bin PATH] [--assets DIR]

Runs the rule files alone on the prose of DIR (default eval/prose, fetched by
tools/bench/fetch_prose.py: encyclopaedic and literary French, presumed
correct) and lists each rule's matches with their context. Nearly every
match here is a false alarm: read them all.
"""
import collections
import json
import os
import subprocess
import sys

args = sys.argv[1:]
opt = lambda name, default: args[args.index(name) + 1] if name in args else default
files = []
i = 0
while i < len(args):
    if args[i].startswith("--"):
        i += 2
        continue
    files.append(args[i])
    i += 1
root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
binary = opt("--bin", os.path.join(root, "target", "release", "grammaria-run"))
assets = opt("--assets", os.path.join(root, "assets", "fr", "lt"))
prose = opt("--prose", os.path.join(root, "eval", "prose"))
show = int(opt("--show", "5"))
by_rule = collections.defaultdict(list)
words = 0
for name in sorted(os.listdir(prose)):
    if not name.endswith(".txt"):
        continue
    path = os.path.join(prose, name)
    cmd = [binary, "matches", path, "--assets", assets]
    for f in files:
        cmd += ["--rules", f]
    out = subprocess.run(cmd, capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit(out.stderr)
    for text, line in zip(open(path, encoding="utf-8").read().split("\n"), out.stdout.split("\n")):
        words += len(text.split())
        for m in json.loads(line):
            a, b = m[1], m[1] + m[2]
            by_rule[m[5]].append(f"{name}: …{text[max(0, a - 60):a]}[{text[a:b]}]{text[b:b + 40]}… -> {m[3][:2]}")
total = sum(len(v) for v in by_rule.values())
print(f"{words} words of prose; {total} matches ({1000 * total / max(words, 1):.2f} per 1000 words)")
for rule in sorted(by_rule, key=lambda r: -len(by_rule[r])):
    print(f"{len(by_rule[rule]):5d}  {rule}")
    for e in by_rule[rule][:show]:
        print("        " + e)
