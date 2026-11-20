#!/usr/bin/env python3
"""Check a rule file against its own examples, as LanguageTool tests its rules.

    python3 tools/rules/test_rules.py assets/fr/grammaria/FILE.xml [--bin PATH] [--assets DIR] [-v]

For every rule:
  * each <example correction="a|b"> with a <marker> must be matched by the
    rule exactly on the marker, and its suggestions must be those of the
    `correction` attribute (when the attribute is there);
  * each <example> without correction must not be matched by the rule.
The rule file is loaded alone, so a rule cannot lean on another file.
Exit status 1 when an example fails.
"""
import json
import os
import re
import subprocess
import sys
import tempfile

args = sys.argv[1:]
opt = lambda name, default: args[args.index(name) + 1] if name in args else default
verbose = "-v" in args
path = args[0]
root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
binary = opt("--bin", os.path.join(root, "target", "release", "grammaria-run"))
assets = opt("--assets", os.path.join(root, "assets", "fr", "lt"))

ENTITIES = {"lt": "<", "gt": ">", "amp": "&", "quot": '"', "apos": "'"}


def unescape(s):
    def one(m):
        name = m.group(1)
        if name.startswith("#x"):
            return chr(int(name[2:], 16))
        if name.startswith("#"):
            return chr(int(name[1:]))
        return ENTITIES.get(name, m.group(0))
    return re.sub(r"&(#x[0-9a-fA-F]+|#\d+|\w+);", one, s)


text = re.sub(r"<!--.*?-->", "", open(path, encoding="utf-8").read(), flags=re.S)
cases = []  # (full id, example text, marker span or None, corrections or None)
group, sub = None, 0
for m in re.finditer(r"<(/?)(rulegroup|rule)\b([^>]*)>", text):
    close, name, attrs = m.groups()
    if name == "rulegroup":
        group = None if close else re.search(r'\bid="([^"]+)"', attrs).group(1)
        sub = 0
        continue
    if close:
        continue
    rid = re.search(r'\bid="([^"]+)"', attrs)
    sub += 1
    full = f"{rid.group(1) if rid else group}[{sub if group else 1}]"
    body = text[m.end():text.index("</rule>", m.end())]
    body = re.sub(r"<antipattern>.*?</antipattern>", "", body, flags=re.S)
    for ex in re.finditer(r"<example\b([^>]*)>(.*?)</example>", body, flags=re.S):
        ex_attrs, inner = ex.groups()
        corr = re.search(r'correction="([^"]*)"', ex_attrs)
        mk = re.search(r"<marker>(.*?)</marker>", inner, flags=re.S)
        plain = unescape(re.sub(r"</?marker>", "", inner))
        span = None
        if mk:
            start = len(unescape(inner[:mk.start()]))
            span = (start, start + len(unescape(mk.group(1))))
        incorrect = corr is not None or 'type="incorrect"' in ex_attrs
        cases.append((full, plain, span if incorrect else None, unescape(corr.group(1)).split("|") if corr and corr.group(1) != "" else None, incorrect))

with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False, encoding="utf-8") as f:
    for _, plain, _, _, _ in cases:
        f.write(plain.replace("\\", "\\\\").replace("\n", "\\n").replace("\t", "\\t") + "\n")
    tmp = f.name
out = subprocess.run([binary, "matches", tmp, "--assets", assets, "--rules", path], capture_output=True, text=True)
os.unlink(tmp)
if out.returncode != 0:
    sys.exit(out.stderr)
failures = 0
rules = set()
for (full, plain, span, corrections, incorrect), line in zip(cases, out.stdout.split("\n")):
    rules.add(full)
    matches = [m for m in json.loads(line) if m[5] == full]
    problem = None
    if incorrect:
        if span is None:
            problem = "incorrect example without <marker>"
        else:
            exact = [m for m in matches if (m[1], m[1] + m[2]) == span]
            if not exact:
                got = [plain[m[1]:m[1] + m[2]] for m in matches]
                problem = f"not matched on «{plain[span[0]:span[1]]}»" + (f" (matched «{'», «'.join(got)}»)" if got else "")
            elif corrections is not None and exact[0][3] != corrections:
                problem = f"suggestions {exact[0][3]} instead of {corrections}"
    elif matches:
        problem = "correct example matched on «" + "», «".join(plain[m[1]:m[1] + m[2]] for m in matches) + "»"
    if problem:
        failures += 1
        print(f"FAIL {full}: {problem}\n     {plain}")
    elif verbose:
        print(f"ok   {full}: {plain}")
without = sorted(r for r in rules if not any(c[0] == r and c[4] for c in cases))
for r in without:
    print(f"WARN {r}: no incorrect example")
print(f"{len(rules)} rules, {len(cases)} examples, {failures} failures")
sys.exit(1 if failures else 0)
