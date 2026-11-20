#!/usr/bin/env python3
"""Which filter class each rule of the LanguageTool rule files uses.

    python3 tools/bench/rule_filters.py [assets/fr/lt]  ->  FULL_ID<TAB>FilterClass

Used to tell apart the detections we miss because a filter has no native
counterpart from those we miss for another reason.
"""
import re
import sys

root = sys.argv[1] if len(sys.argv) > 1 else "assets/fr/lt"
tag = re.compile(r"<(/?)(rulegroup|rule|filter)\b([^>]*)>")


def filters(path):
    out = {}
    text = re.sub(r"<!--.*?-->", "", open(path, encoding="utf-8").read(), flags=re.S)
    group = None
    sub = 0
    current = None
    for m in tag.finditer(text):
        close, name, attrs = m.groups()
        if name == "rulegroup":
            if close:
                group = None
            else:
                group = re.search(r'\bid="([^"]+)"', attrs).group(1)
                sub = 0
        elif name == "rule" and not close:
            rid = re.search(r'\bid="([^"]+)"', attrs)
            if group:
                # A rule may carry its own id inside a group; the numbering goes on.
                sub += 1
                current = f"{rid.group(1) if rid else group}[{sub}]"
            else:
                current = f"{rid.group(1)}[1]" if rid else None
        elif name == "filter" and current:
            cls = re.search(r'class="([^"]+)"', attrs).group(1).rsplit(".", 1)[-1]
            out[current] = cls
    return out


if __name__ == "__main__":
    for f in ("grammar.xml", "style.xml"):
        for k, v in filters(f"{root}/{f}").items():
            print(f"{k}\t{v}")
