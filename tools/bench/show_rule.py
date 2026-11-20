#!/usr/bin/env python3
"""Print the XML of one rule: python3 tools/bench/show_rule.py 'A_ACCENT[10]' [--examples]"""
import re
import sys

want = sys.argv[1]
for f in ("grammar.xml", "style.xml"):
    text = re.sub(r"<!--.*?-->", "", open(f"assets/fr/lt/{f}", encoding="utf-8").read(), flags=re.S)
    group, sub = None, 0
    for m in re.finditer(r"<(/?)(rulegroup|rule)\b([^>]*)>", text):
        close, name, attrs = m.groups()
        if name == "rulegroup":
            group = None if close else re.search(r'\bid="([^"]+)"', attrs).group(1)
            sub = 0
        elif not close:
            rid = re.search(r'\bid="([^"]+)"', attrs)
            sub += 1
            full = f"{rid.group(1) if rid else group}[{sub if group else 1}]"
            if full == want:
                body = text[m.start():text.index("</rule>", m.start()) + 7]
                if "--examples" not in sys.argv:
                    body = re.sub(r"\s*<example.*?</example>", "", body, flags=re.S)
                print(f, body)
