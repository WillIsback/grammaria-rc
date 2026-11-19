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
                # Walk the example, recording absolute marker spans relative to
                # the full sentence text (markers may nest inside tokens).
                spans = []
                buf = []
                pos = [0]

                def walk_example(n, inside):
                    marker_here = inside or strip_ns(n.tag) == 'marker'
                    if n.text:
                        if marker_here and not inside:
                            spans.append([pos[0], pos[0] + len(n.text)])
                        elif marker_here and inside:
                            # extend the last span
                            spans[-1][1] = pos[0] + len(n.text)
                        buf.append(n.text)
                        pos[0] += len(n.text)
                    for c in n:
                        walk_example(c, marker_here)
                    if n.tail:
                        buf.append(n.tail)
                        pos[0] += len(n.tail)

                walk_example(ex, False)
                text2 = ''.join(buf)
                text = text2
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
