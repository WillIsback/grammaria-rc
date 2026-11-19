# Third-party data and code

Grammaria's own source code is licensed under the **Mozilla Public License 2.0**
(see `LICENSE`).

The French linguistic data shipped in `assets/fr/` originates from open-source
projects and is redistributed unmodified (converted to compact binary form at
build time by `grammaria-build`, never edited by hand):

## LanguageTool

| file | content | license |
|---|---|---|
| `languagetool-grammar.xml` | 5 274 French grammar rules | LGPL-2.1 |
| `languagetool-style.xml` | 1 673 French style rules | LGPL-2.1 |
| `languagetool-disambiguation.xml` | 1 007 POS-disambiguation rules | LGPL-2.1 |
| `languagetool-pos-dict.txt` | 634 004 French word forms with lemmas/POS | LGPL-2.1 |
| `languagetool-multiwords.txt` | 4 886 multiword chunks | LGPL-2.1 |
| `languagetool-hyphenated_words.txt` | hyphenated compounds | LGPL-2.1 |
| `languagetool-confusion_sets.txt` | confusable word sets and probabilities | LGPL-2.1 |
| `spelling.txt`, `ignore.txt`, `prohibit.txt` | spell-checker word lists | LGPL-2.1 |
| `common_words.txt`, `compounds.txt` | auxiliary word lists | LGPL-2.1 |
| `tagset.txt` | POS tag documentation | LGPL-2.1 |

Source: <https://github.com/languagetool-org/languagetool> (French module) and
<https://github.com/languagetool-org/french-pos-dict>.
LanguageTool is © 2001-2025 Daniel Naber and the LanguageTool contributors,
distributed under the GNU Lesser General Public License, version 2.1 or later.
The behaviour re-implemented in `src/filters.rs` (`WordWithDeterminerFilter`)
follows the logic of the corresponding LanguageTool Java class.

## Grammalecte

| file | content | license |
|---|---|---|
| `grammalecte_rules.grx` | French grammar rule source (43 926 lines) | GPL-3.0 |
| `grammalecte_fr.aff` | Hunspell affix definitions for French | MPL-2.0 / GPL-3.0 |

Source: <https://github.com/Dicollecte/grammalecte>, © Olivier R.
These files are included as **reference material** for a future native port of the
Grammalecte rule language; they are not loaded by the checker at run time.

## Build/benchmark tooling (not redistributed)

LanguageTool 6.6 (LGPL) and a JRE are used only to produce evaluation fixtures and
benchmark numbers; they are not part of the Grammaria binary or its data.
