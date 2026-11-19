#!/usr/bin/env python3
"""Extract LanguageTool rule examples into a JSON corpus.

Each example carries the rule it belongs to, the sentence (marker tags removed)
and how many markers cover which characters, so the evaluator can check whether
our engine fires the right rule on the marked text.
"""
import json, sys
import xml.etree.ElementTree as ET


def strip_ns(tag):
    return tag.split('}')[-1]


def main():
    path, out = sys.argv[1], sys.argv[2]
    doc = ET.parse(path)
    root = doc.getroot()
    examples = []

    def walk(node, group_id=None):
        tag = strip_ns(node.tag)
        if tag in ('rule', 'rulegroup'):
            rid = node.get('id') or group_id or ''
            if tag == 'rulegroup':
                for child in node:
                    walk(child, rid)
                return
            exs = [c for c in node if strip_ns(c.tag) == 'example']
            if not exs:
                return
            for ex in exs:
                text = ''.join(ex.itertext())
                # marker spans
                spans = []
                marked_text = []
                def collect(n, inside):
                    if strip_ns(n.tag) == 'marker':
                        inside = True
                    if n.text:
                        if inside:
                            spans.append((len(marked_text), len(marked_text) + len(n.text)))
                        marked_text.append(n.text)
                    for c in n:
                        collect(c, inside)
                    if n.tail:
                        marked_text.append(n.tail)
                collect(ex, False)
                text2 = ''.join(marked_text)
                examples.append({
                    'rule': rid,
                    'text': text,
                    'correction': ex.get('correction'),
                    'type': ex.get('type'),
                    'markers': spans,
                })
            return
        for c in node:
            walk(c, group_id)

    walk(root)
    json.dump(examples, open(out, 'w', encoding='utf-8'), ensure_ascii=False)
    print(f'{len(examples)} examples -> {out}')


if __name__ == '__main__':
    main()
