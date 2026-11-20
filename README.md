# Grammaria

**A state-of-the-art French grammar and spelling checker, written in Rust — a drop-in
replacement for LanguageTool that is 15–23× faster on the same text.**

Grammaria re-implements the LanguageTool French checking pipeline natively in Rust:
tokenizer → POS tagger (634k-entry French lexicon) → rule-based disambiguator
(1 007 rules) → grammar rule engine (6 947 rules from `grammar.xml`/`style.xml`) →
native implementations of the Java rule filters that decide what a real error is.

```
$ time grammaria check article.txt          $ time java -jar languagetool-commandline.jar -l fr article.txt
real  0m0.16s                               real  0m4.06s
```

| | Grammaria | LanguageTool 6.6 | speed-up |
|---|---|---|---|
| simple French paragraph (3 sentences) | **0.16 s** | 3.69 s | **22.7×** |
| 10-sentence article (108 words) | **0.27 s** | 4.19 s | 15.3× |

Both numbers are complete CLI invocations (process start, data load, checking, output)
measured with `tools/bench.py`.

## What it finds

```
$ echo "Cette événement est très important pour tout les participants. La toit fuit." | grammaria check -
0,17   D_N              Le déterminant s'accorde avec le nom "événement".
                        → Ce événement
43,8   AGREEMENT_TOUT_LE "Tout" et le groupe nominal ne semblent pas bien accordés.
                        → tous
71,7   D_N              Le déterminant s'accorde avec le nom "toit".
                        → Le toit
```

Errors covered (all from the LanguageTool French rule set):

* **agreement** — determiner/noun gender & number (`Cette événement` → *Ce événement*,
  `La toit` → *Le toit*, `Les voiture` → *Les voitures*, `Un belle femme` → *Une belle*),
  adjective/noun agreement, subject-verb agreement, `tout/toute/tous/toutes`,
* **auxiliaries and past participles** — `avoir/être` + participle, infinitive vs participle,
* **typography** — French spacing (`! ? : ;` non-breaking space), apostrophes, hyphens,
  non-breaking spaces before units, NBSP rules,
* **confusions** — homophones and false friends, elision, capitalisation,
* **style** — 1 673 style rules (wordiness, pleonasms, register) when `--style` is given.

Each match carries the rule id, a human-readable message and corrected suggestions,
in LanguageTool's JSON shape (`--json`).

## Design

```
assets/fr/*.xml,*.txt                 open-source data (LanguageTool, LGPL)
        │  grammaria-build (one-off, ~7 min: fr.lex dominates)
        ▼
data/fr.lex      double-array trie     475 097 word forms / 616 695 senses   ~37 MB
data/fr.rules    compiled rule cache   6 947 rules                           ~7 MB
data/fr.words    multiwords, hyphenated compounds, extra spellings           ~1 MB
data/fr.disambig.xml  disambiguation rules, copied verbatim              ~0.4 MB
        │
        ▼  grammaria (runtime: ~0.14 s load, ~7 ms per sentence)
tokenizer → tagger → chunker → disambiguator → rule engine → native filters
```

`data/` is **generated, not tracked**: it is rebuilt from `assets/` by
`grammaria-build`. A fresh clone therefore runs the build once before checking
text; the binary prints that instruction if the data is missing. The
disambiguation and rule XML are also read straight from `assets/fr/`, so only
the three binary files above are strictly required.

* **Tokenizer** — French-aware: elisions (`l'`, `qu'`), hyphenated forms (`dit-il`),
  apostrophe compounds (`aujourd'hui`), acronyms, numbers, URLs, sentence splitting
  with abbreviation and ordinal handling.
* **Tagging** — every token gets lemma + LanguageTool POS tags (`N m s`, `V ind pres 3 s`,
  `R pers suj 1 s`, …) from a **double-array trie** over 475 k word forms with interned
  senses (lookups are allocation-free).
* **Disambiguation** — the 1 007 `disambiguation.xml` rules run with LanguageTool's
  semantics (marks, `filter`/`remove`/`add`/`replace`, match groups, antipatterns).
* **Rule engine** — patterns, `min`/`max`/`skip` (`-1` = unbounded), `<exception>` with
  scope, `<and>`/`<or>` groups, `<unify>` feature agreement, `<marker>` spans,
  antipatterns, priorities, `case_sensitive`, `spacebefore`, `default="off"`/`"temp_off"`,
  `picky` tags, and LanguageTool's `SENT_START`/`SENT_END` structural tokens.
* **Regex engine** — a Pike-VM NFA over bytes with full Java `Matcher` semantics
  (`find`, `matches`, `(?i)`, `\p{Ll}`-style classes, lookahead), compiled lazily and
  memoised per source string.
* **Native filters** — `WordWithDeterminerFilter` is re-implemented in Rust: it checks
  determiner/noun gender and number agreement and synthesises the corrected forms
  (`Les voiture` → *Les voitures*), validating each candidate against the lexicon.
* **Performance** — compiled rule cache, first-token and POS letter indexes, lazy regex
  compilation, and a fail-fast membership pre-filter keep a 107-word article check at
  ~10 ms of CPU after load.

## Usage

```sh
# build the data once (lexicon + rule cache); ~7 min, dominated by fr.lex
cargo run --release --bin grammaria-build

# rebuild a single stage after changing rule/tagging semantics (0.3 s)
cargo run --release --bin grammaria-build -- --only rules

# check a file as text
grammaria check article.txt

# LanguageTool-compatible JSON (offsets/lengths/messages/suggestions)
grammaria check --json article.txt

# also run style rules
grammaria check --style --json article.txt

# re-check a file on an interval, reusing the analysis of unchanged sentences
grammaria check --watch 1 article.txt

# inspect the POS analysis (LanguageTool --verbose format)
grammaria check --tagger article.txt

# pipe
echo "La toit fuit." | grammaria check --json -
```

### Long-running mode

`grammaria serve` loads the data once and answers requests on stdin, one JSON
line each, so a host application pays the load time a single time:

```sh
$ printf '1 20\nJe mange un pomme. ' | grammaria serve --data data
{"ready":true}
{"id":1,"matches":[{"message":"Le déterminant s'accorde avec le nom \"pomme\".", ...}]}
```

A request is `<id> <byte length>\n` followed by that many bytes of UTF-8 text.
Offsets in the reply are UTF-16 code units, as LanguageTool and JavaScript
count them (`grammaria check --json` reports bytes).

Exit status is `1` when problems were found, `0` when the text is clean.
Set `GRAMMARIA_DATA=/path/to/data` to relocate the data directory.

## Library

```rust
use grammaria::api::{Engine, OffsetUnit};

// Loads fr.lex, fr.words, fr.rules and fr.disambig.xml from the data directory.
let engine = Engine::load(std::path::Path::new("data"))?;
for m in engine.check("Cette événement est important.", false) {
    println!("{} {}: {} {:?}", m.offset, m.length, m.message, m.suggestions);
}
// LanguageTool-shaped `matches` array, offsets in UTF-16 code units.
let json = engine.check_json("Cette événement est important.", false, OffsetUnit::Utf16);
```

## Evaluation

Two harnesses measure the port against the reference implementation:

* `tools/eval_pos.py` — POS-tag agreement with LanguageTool on a sentence fixture
  (`tools/gen_lt_fixtures.py` records LanguageTool's own `--verbose` output and JSON
  matches). **88 %** of tokens (272/308) carry exactly the same tag set. The work that
  got it there: `<unify>` groups must keep their feature-agreement test (flattening them
  to plain tokens silently disabled it), a bare `<disambig postag="X"/>` is the *filter*
  action, `<pattern case_sensitive="yes">` propagates to every token, and
  `scope="previous"`/`"next"` exceptions use whole-token rather than `find()` semantics.
* `tools/eval_rules.py` — detection on the 8 193 `<example>` sentences embedded in
  LanguageTool's French rules (the ground truth used to validate the rules themselves).
  On a stratified 800-sentence sample: **49 %** of the marked errors are detected at the
  marked span, and **72 %** of the sentences LanguageTool considers clean are left clean.
  The common, user-facing agreement rules score far higher than the average —
  `AGREEMENT_TOUT_LE` 44/45 on a 1 000-example sample, `D_N` 49/60 with the identical
  rule id — while the remainder of the gap is concentrated in rules that need
  LanguageTool's disambiguation refinements (homophone sets such as `D_N_E_OU_E`,
  `ETRE_VPPA_OU_ADJ`) and Java filters that search the morphological dictionary.
  Only rules whose semantics are faithfully reproducible are enabled; the rest are
  skipped rather than left to over-fire.
* `tools/regress.py` — freezes the exact matches on 16 representative sentences
  (`eval/regression_fr.json`) so that memory/size work cannot silently change what
  the checker reports. Run it before and after any change to rule semantics; no
  LanguageTool installation required.
* `tools/bench.py` — the speed comparison above.
* `tools/memprobe.rs` (built as `--bin memprobe`) — prints RSS after each loading
  stage, so a memory change can be attributed to the lexicon, the rules, the word
  lists or the disambiguator.

```sh
python3 tools/gen_lt_fixtures.py --lt-dir /path/LanguageTool-6.6 --java $(which java) \
        --input eval/langs_fr.txt --out eval/fixtures_fr.json
python3 tools/gen_examples.py assets/fr/languagetool-grammar.xml eval/lt_examples_fr.json
python3 tools/eval_pos.py   eval/fixtures_fr.json      ./target/release/grammaria data
python3 tools/eval_rules.py eval/lt_examples_fr.json   ./target/release/grammaria data 400
python3 tools/regress.py
python3 tools/bench.py      --text article.txt
```

## Throughput

`check_text` splits a document into sentences and runs them on all cores
(`std::thread::scope`, no rayon). Sentences are independent and both the tagger
and the rule set are read-only, which makes this safe without shared locks.
Below 40 sentences it stays single-threaded, since spawning costs more than it
saves on short inputs.

`grammaria check --watch 1 FILE` re-checks on an interval and reuses the cached
analysis of unchanged sentences (`rules::Checker`): editing one paragraph of a
200-sentence document re-analyses one sentence (11 ms instead of 2 s).

## Tests

```sh
cargo test          # unit tests: XML parser, lexicon, regex engine, tokenizer
```

## Data and licensing

The rule files and dictionaries are taken from open-source projects and are **not**
modified; they are converted into compact binary forms at build time. See
[THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md) for the full list and terms:

* **LanguageTool** French data (grammar.xml, style.xml, disambiguation.xml,
  multiwords.txt, hyphenated_words.txt, POS dictionary) — LGPL 2.1,
* **Grammalecte** (`rules.grx`, Hunspell affixes) — GPL 3, included as reference
  material for future rule ports.

Grammaria's own code is licensed under MPL-2.0.
