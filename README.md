# Grammaria

**A French grammar and spelling checker in Rust: LanguageTool's French
checker, reproduced match for match, then extended with its own rules.**

Grammaria runs LanguageTool's French rule files and dictionaries unchanged —
sentence splitting, tokenizing, tagging, disambiguation, 7 017 XML rules,
the rules and filters LanguageTool writes in Java, the Morfologik speller —
through a Rust port of the same pipeline, with no dependency. On top of it
come 1 320 rules of its own for errors LanguageTool misses, and a rule
profile that removes the alarms a careful writer does not want.

```
$ printf 'Elle semble être totalement ruiné. Les enfants mange des pommes.' | grammaria check -
29:6: Elle semble être totalement ruiné. Les enfants mange des pommes. [GRM_ATTRIBUT_GENRE_PRONOM]  suggestions: ruinée
  Accord en genre : « ruiné » se rapporte au sujet « Elle », féminin singulier.
49:5: Elle semble être totalement ruiné. Les enfants mange des pommes. [ACCORD_SUJET_VERBE]  suggestions: mangent
  Le verbe devrait être à la 3ᵉ personne du pluriel.
```

## Results

Measured against the reference this engine was built to equal: LanguageTool
6.9-SNAPSHOT as ported to JavaScript by `@valueerror/ltjs` 0.3.0 (the
checker of the Grammaria application before this engine), on the same
machine (4 virtual cores).

### Same results as LanguageTool

With LanguageTool's rules alone (`--lt-only`), on 29 047 benchmark texts:

| | |
|---|---|
| texts with exactly the same matches as the reference | 29 038 (99.97 %) |
| reference matches found (same rule, same span) | 22 159 of 22 168 |
| of which with the same suggestions | 99.65 % |
| matches the reference does not report | 2 |
| analysis (tokens, tags, disambiguation) identical | 500 000 tokens of 500 000 |

Seven of the nine missing matches are false alarms of the reference (a
word-repetition rule firing on `on dit ou on écrit`). The suggestion
differences are in spelling: checked against LanguageTool in Java, the
suggestions here are LanguageTool's, the JavaScript port's differ.

### Better than LanguageTool

The default configuration adds Grammaria's rules and profile:

| benchmark | | reference | Grammaria |
|---|---|---|---|
| LanguageTool's rule examples (8 195 errors) | errors found | 88.3 % | 88.3 % |
| | correct texts left alone | 65.5 % | 68.6 % |
| Grammalecte's test sentences (5 847 errors) | errors found | 59.6 % | **83.5 %** |
| | correct texts left alone | 62.4 % | 65.7 % |
| hand-corrected texts (92 errors) | errors found | 75 | **83** |
| all | errors found | 76.4 % | **86.4 %** |
| | correct texts left alone | 63.8 % | **67.0 %** |
| held-out half (never looked at while writing rules) | errors found | 76.2 % | **82.7 %** |
| | correct texts left alone | 63.1 % | **66.5 %** |
| well-edited prose (234 000 words) | alarms per 1 000 words | 21.6 | **8.8** |

"Correct texts left alone" counts the texts without a marked error that get
no match at all; many of them are sentence fragments, on which both checkers
rightly report a missing capital or full stop. The prose is encyclopaedic and
literary French, presumed correct: nearly every alarm there is a false one.

### Faster and lighter

| | reference | Grammaria |
|---|---|---|
| start (load everything, first check) | 4.3–4.6 s | **0.5 s** |
| memory | 924–937 MiB | **220–250 MiB** |
| a 160-word text, first check | 531–608 ms | **124–180 ms** |
| the same, warmed up | 179–192 ms | **47–58 ms** |
| 16 texts, 944 words, warmed up, one thread | 0.97–1.02 s | **0.29–0.35 s** |

No crash on 40 000 randomly mutated texts (emoji, typographic apostrophes,
no-break spaces, soft hyphens, other scripts, very long lines).

## What is in it

```
text ── sentences (SRX) ── word tokens ── tags (Morfologik dictionary, 666 726 entries)
     ── disambiguation (multi-word chunks, 1 072 XML rules)
     ── rules ── overlap and priority filtering ── profile ── matches
```

* **LanguageTool's rules** (`assets/fr/lt/`): `grammar.xml` (5 343 rules),
  `style.xml` (1 674), with their 14 Java filters ported
  (`src/engine/filters/`, `src/engine/spelling/`); the 13 rules written in
  Java (`src/engine/native/`: capital at sentence start, French spacing,
  brackets, compounds, replacements, repeated words...); the Morfologik
  spelling rule with LanguageTool's suggestion logic.
* **Grammaria's rules** (`assets/fr/grammaria/*.xml`, same XML format, 1 320
  rules with 3 081 examples that are tested):

  | file | rules | what |
  |---|---|---|
  | `accords_participe.xml` | 120 | participle and adjective agreement with the subject, through *être*, state verbs and infinitive chains |
  | `conjugaison.xml` | 131 | mood after conjunctions and verbs, subject–verb agreement, imperative, inversions |
  | `infinitif.xml` | 56 | participle or conjugated form where an infinitive is required |
  | `confusions.xml` | 686 | homophones (*a/à*, *on/ont*, *peu/peut*...) and word confusions (*pore/port/porc*, *différent/différend*...) |
  | `groupe_nominal.xml` | 114 | agreement inside the noun group, numerals, fixed number of complements |
  | `typographie.xml` | 213 | hyphens, large numbers, units, quotation marks, capitals, pleonasms, set phrases |

  When one of these rules and a LanguageTool rule flag the same words,
  LanguageTool's is kept.
* **The profile** (`assets/fr/grammaria/profile.txt`): seven LanguageTool
  rules are switched off because they mostly disturb on well-edited prose
  (the figures are in the file), and unknown words that look like proper
  nouns or acronyms are not reported as misspelt — 3 100 of the reference's
  3 800 spelling alarms on that prose are names such as *Fogg* or
  *Allahabad*; on the benchmark this costs one detection.

The engine reads these files as they are: there is no build step for the
data, and editing a rule file is enough to change the checker.

## Usage

```sh
cargo build --release

# check a file (or - for stdin); exit status 1 when problems were found
grammaria check article.txt
grammaria check --json article.txt        # LanguageTool's JSON shape
grammaria check --no-style article.txt    # without the style rules
grammaria check --tagger article.txt      # tokens and tags
grammaria check --watch 1 article.txt     # re-check when the file changes
```

The data directory holds `lt/` and `grammaria/`. From a source checkout it
is `assets/fr`; elsewhere set `--data DIR` or `GRAMMARIA_DATA`, and make the
directory with `grammaria-build --out DIR` (a plain copy of the assets).

### Long-running mode

`grammaria serve` loads the data once and answers requests on stdin, one
JSON line each:

```sh
$ printf '1 19\nJe mange un pomme. ' | grammaria serve
{"ready":true}
{"id":1,"matches":[{"message":"Le déterminant s'accorde avec le nom \"pomme\".", ...}]}
```

A request is `<id> <byte length>\n` followed by that many bytes of UTF-8
text. Offsets in the reply are UTF-16 code units, as LanguageTool and
JavaScript count them (`grammaria check --json` reports bytes).

### Library

```rust
use grammaria::api::{Engine, OffsetUnit};

let engine = Engine::load(std::path::Path::new("assets/fr"))?;
for m in engine.check("Cette événement est important.", true) {
    println!("{} {}: {} {:?}", m.offset, m.length, m.message, m.suggestions);
}
// LanguageTool-shaped `matches` array, offsets in UTF-16 code units.
let json = engine.check_json("Cette événement est important.", true, OffsetUnit::Utf16);
```

`Engine` is `Sync`: one instance serves all threads.

## Writing rules

`tools/rules/README.md` explains the conventions and the tools: `show.py`
(what a rule sees of a sentence), `test_rules.py` (every example of a rule
file must pass), `gain.py` (errors of the benchmark a rule file finds that
the reference misses, and its false alarms), `prose.py` (its matches on
well-edited prose) and `targets.py` (what the reference still misses).

## Reproducing the measurements

```sh
# the benchmark sets are in eval/bench, the reference's matches in eval/oracle
for n in corpus lt gl; do
  ./target/release/grammaria-run matches eval/bench/$n.txt --lt-only > eval/out/lt.$n.matches.jsonl
  ./target/release/grammaria-run matches eval/bench/$n.txt           > eval/out/grammaria.$n.matches.jsonl
done
python3 tools/bench/score.py reference=eval/oracle/ltjs lt-only=eval/out/lt grammaria=eval/out/grammaria [--split test]
python3 tools/bench/diff_matches.py eval/oracle/ltjs.lt.matches.jsonl eval/out/lt.lt.matches.jsonl --classes all

python3 tools/bench/fetch_prose.py           # well-edited prose, into eval/prose
./target/release/grammaria-run matches eval/bench/corpus.txt --time --passes 4   # times and memory
```

`tools/bench/oracle_ltjs.mjs` runs the reference (matches, or tokens at each
stage of the analysis) on any text file; it needs Bun and the `ltjs`
packages.

## Limits

* French only.
* LanguageTool's statistical rules (n-gram confusion pairs) and its remote
  rules are not part of this engine, as they were not part of the reference.
* Rule matching and spelling suggestions follow LanguageTool's algorithms
  faithfully rather than cleverly; memory (about 240 MiB) is mostly its 7 000
  rules held in parsed form.
* Grammaria's own rules were written against a benchmark: on the held-out
  half they find fewer new errors than on the half they were tuned on
  (+6.5 points against +13.5), and a few are tied to its word lists.

## Licenses

Grammaria's own code and rules: MPL-2.0. The engine modules that are ports
of LanguageTool's Java code, and LanguageTool's data, are under the LGPL 2.1
or later; the dictionary reader is a port of Morfologik (BSD-3-Clause). See
`THIRD_PARTY_LICENSES.md`.
