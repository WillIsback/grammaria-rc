#!/usr/bin/env python3
"""Show how the engine analyses a sentence: the tokens a rule will see.

    python3 tools/rules/show.py "Elle semble être ruiné." [--raw] [--bin PATH] [--assets DIR]

Prints one token per line with its readings (lemma/POSTAG) after
disambiguation (`--raw`: as the tagger produced them, which is what
`raw_pos="yes"` rules match), and its chunk tags.
"""
import json
import os
import subprocess
import sys
import tempfile

args = sys.argv[1:]
opt = lambda name, default: args[args.index(name) + 1] if name in args else default
root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
binary = opt("--bin", os.path.join(root, "target", "release", "grammaria-run"))
assets = opt("--assets", os.path.join(root, "assets", "fr", "lt"))
mode = "raw" if "--raw" in args else "tokens"
with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False, encoding="utf-8") as f:
    f.write(args[0].replace("\\", "\\\\").replace("\n", "\\n") + "\n")
out = subprocess.run([binary, mode, f.name, "--assets", assets], capture_output=True, text=True)
os.unlink(f.name)
if out.returncode != 0:
    sys.exit(out.stderr)
for sentence in json.loads(out.stdout.split("\n")[0]):
    for token, readings, flags, chunks in sentence:
        if not token.strip() and not any(r[1] for r in readings):
            continue
        marks = ("  immunized" if flags & 1 else "") + ("  " + ",".join(chunks) if chunks else "")
        print(f"{token!r:18s} " + "  ".join(f"{r[0]}/{r[1]}" for r in readings) + marks)
    print()
