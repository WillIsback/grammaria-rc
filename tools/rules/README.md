# Writing Grammaria's own rules

The engine runs LanguageTool's French rule files unchanged, and then this
project's own rule files: every `assets/fr/grammaria/*.xml`. They use the
same XML format (`<category>`, `<rulegroup>`, `<rule>`, `<pattern>`,
`<antipattern>`, `<token>`, `<exception>`, `<marker>`, `<message>`,
`<suggestion>`, `<match>`, `<example>`), so LanguageTool's
`assets/fr/lt/grammar.xml` is the best source of idioms, and its tag set is
listed in `assets/fr/lt/french_tags.txt`.

## Conventions

- Rule ids start with `GRM_`; category ids too. Messages are in French,
  short, and say what is wrong and why; suggestions are concrete corrections.
- Every rule has at least one incorrect example
  (`<example correction="…">… <marker>…</marker> …</example>`) and one
  correct example close to it. Write your own sentences.
- A rule must be worth its false alarms: a detection the writer cannot trust
  is worse than none. Prefer a narrow, certain rule to a broad, uncertain
  one, and use `<antipattern>` and `<exception>` for the known exceptions.
- Rules see the text after disambiguation; `raw_pos="yes"` on `<pattern>`
  matches the tags the tagger produced instead.

## Tools

All take `--bin PATH` (the `grammaria-run` binary, default
`target/release/grammaria-run`) and `--assets DIR`.

| Tool | What it does |
|---|---|
| `show.py "phrase" [--raw]` | the tokens and readings a rule sees |
| `test_rules.py FILE.xml [-v]` | checks every example of the file (must pass) |
| `gain.py FILE.xml --set all --split dev --show 5` | per rule: errors of the benchmark it finds that the reference checker misses (`new`), and its false alarms (`false`) |
| `prose.py FILE.xml` | its matches on well-edited prose (`eval/prose/`, see `tools/bench/fetch_prose.py`): nearly all false alarms |

The benchmark (`eval/bench/*.jsonl`) has three sets — `lt` (LanguageTool's
rule examples), `gl` (Grammalecte's test sentences, each with the name of
the Grammalecte rule it tests) and `corpus` (hand-corrected texts) — split
into `dev` and `test` halves. Work on `dev`; look at `test` only to check
that a rule generalises.

`python3 tools/bench/score.py` gives the overall scoreboard of a checker.
