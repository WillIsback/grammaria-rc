# Third-party notices

Grammaria is distributed as a whole under the **GNU Lesser General Public
License, version 2.1 or later** (see `LICENSE`). That covers the project's
own code and rule files (`assets/fr/grammaria/`) and the parts below that
come from LanguageTool.

This file keeps the notices of the projects Grammaria builds on, as their
licenses require. Two of them keep their own terms inside the whole, both
compatible with the LGPL: Morfologik's (BSD-3-Clause) and the dictionary
data's (MPL-2.0).

## LanguageTool — data (`assets/fr/lt/`)

The French resources of LanguageTool 6.9-SNAPSHOT (commit 59fb512), read as
they are by the engine:

| file | content |
|---|---|
| `grammar.xml`, `style.xml` | 5 343 grammar rules and 1 674 style rules |
| `disambiguation.xml`, `disambiguation-global.xml` | part-of-speech disambiguation rules |
| `french.dict`, `french.info` | tagger and speller dictionary (666 726 entries, Morfologik automaton) |
| `french_synth.dict`, `french_synth.info`, `french_tags.txt` | synthesis dictionary and its tag list |
| `added*.txt`, `removed*.txt` | manual additions and removals of readings |
| `multiwords.txt`, `spelling_global.txt`, `hyphenated_words.txt`, `compounds.txt` | multi-word and compound lists |
| `hunspell/spelling*.txt`, `hunspell/ignore.txt`, `hunspell/prohibit.txt` | speller word lists |
| `replace*.txt`, `synonyms.txt`, `confusion_sets.txt` | replacement, synonym and confusion lists |
| `segment.srx` | sentence segmentation rules |
| `fr.sor`, `Roman.sor` | number-to-text programs (from the Numbertext project) |
| `remote-rule-filters.xml` | not used |

Source: <https://github.com/languagetool-org/languagetool>. LanguageTool is
© Daniel Naber and the LanguageTool contributors, distributed under the
**GNU Lesser General Public License, version 2.1 or later**. The French
dictionary derives from Dicollecte/Grammalecte data (MPL-2.0) through
<https://github.com/languagetool-org/french-pos-dict>.

## LanguageTool — ported code (`src/engine/`)

To run those rule files exactly as LanguageTool does, the engine is a Rust
port of the corresponding parts of LanguageTool's Java source (upstream
`master`, which the 6.9-SNAPSHOT resources belong to): the token model,
sentence splitting, word tokenizing, tagging, the multi-word chunker and the
rule-based disambiguator, the pattern-rule matcher and unifier, the XML rule
loaders, the synthesizer and number speller, the rule filters
(`src/engine/filters/`), the rules written in Java (`src/engine/native/`),
the spell-checking rule (`src/engine/spelling/`) and the selection and
filtering of matches (`src/engine/checker.rs`, `src/engine/french.rs`). Each
file names the Java classes it follows.

These files are translations of LGPL-2.1-or-later code: the LGPL's terms
apply to them. The full text of the license is in `LICENSE`. The message strings of the native rules come from
LanguageTool's `MessagesBundle_fr.properties`.

No code comes from `@valueerror/ltjs` (a GPL-3 JavaScript port of
LanguageTool): it is only run as a reference, through
`tools/bench/oracle_ltjs.mjs`, to check that this engine gives the same
results.

## Morfologik (`src/morfologik/`)

A Rust port of the dictionary reader and speller of
[Morfologik](https://github.com/morfologik/morfologik-stemming) 2.1.9
(© Dawid Weiss and Marcin Miłkowski, **BSD-3-Clause**), with Unicode
character tables generated from the JDK it was checked against.

```
Copyright (c) 2006 Dawid Weiss
Copyright (c) 2007-2015 Dawid Weiss, Marcin Miłkowski
All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

    * Redistributions of source code must retain the above copyright notice,
    this list of conditions and the following disclaimer.

    * Redistributions in binary form must reproduce the above copyright notice,
    this list of conditions and the following disclaimer in the documentation
    and/or other materials provided with the distribution.

    * Neither the name of Morfologik nor the names of its contributors
    may be used to endorse or promote products derived from this software
    without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

## Benchmark material (`eval/`, not used at run time)

| path | content | license |
|---|---|---|
| `eval/bench/lt.*`, `eval/lt_examples_fr.json` | the `<example>` sentences of LanguageTool's French rules | LGPL-2.1-or-later |
| `eval/bench/gl.*` | the test sentences of Grammalecte's French rules | GPL-3.0 |
| `eval/sources/grammalecte_rules.grx`, `grammalecte_fr.aff` | Grammalecte's rule source (the origin of those test sentences) and affix file, © Olivier R. | GPL-3.0 (affix file: MPL-2.0) |
| `eval/sources/languagetool-pos-dict.txt` | text dump of LanguageTool 6.6's French dictionary, used by the dictionary reader's tests | LGPL-2.1-or-later |
| `eval/oracle/` | matches reported by the reference checker on the benchmark | — |
| `eval/corpus_fr.json`, `eval/bench/corpus.*` | texts with a hand-written correction, from the Grammaria application | project's own |

Grammaria's own rules (`assets/fr/grammaria/`) were written against the
*behaviour* these sentences describe — which errors a checker should find —
and not from Grammalecte's rule source.

`tools/bench/fetch_prose.py` downloads encyclopaedic and literary prose
(French Wikipedia, CC BY-SA; French Wikisource, public domain) to measure
false alarms; that text is not part of the repository.
