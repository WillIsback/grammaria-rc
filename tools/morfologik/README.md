# Morfologik oracle

`src/morfologik/` is a port of the Java library Morfologik 2.1.9. These tools
produce the reference outputs it is tested against
(`tests/fixtures/morfologik/`, read by `tests/morfologik.rs`).

## Build the Java harness

The jars `morfologik-fsa.jar`, `morfologik-stemming.jar` and
`morfologik-speller.jar` are in the `libs/` directory of a LanguageTool
distribution.

```sh
CP=libs/morfologik-fsa.jar:libs/morfologik-stemming.jar:libs/morfologik-speller.jar
javac -cp $CP -d out tools/morfologik/Oracle.java
oracle() { java -cp $CP:out Oracle "$@"; }
```

## Regenerate the fixtures

```sh
python3 tools/morfologik/make_wordlists.py . /tmp/w 50 speller_words.txt
F=tests/fixtures/morfologik
oracle lookup  assets/fr/lt/french.dict       < /tmp/w/lookup_words.txt  > $F/lookup_oracle.tsv
oracle lookup  assets/fr/lt/french_synth.dict < /tmp/w/synth_keys.txt    > $F/synth_oracle.tsv
oracle speller assets/fr/lt/french.dict       < /tmp/w/speller_words.txt > $F/speller_oracle.tsv
python3 tools/morfologik/make_encoder_pairs.py . | oracle encode         > $F/encoders_oracle.tsv
```

## Larger checks

`tests/morfologik.rs` reads other oracle files when these variables are set:

| variable | content |
|---|---|
| `MORFOLOGIK_DUMP_ORACLE` (+ `MORFOLOGIK_DUMP_DICT`) | `oracle dump <dict>`: every entry, in order |
| `MORFOLOGIK_LOOKUP_ORACLE` | `oracle lookup french.dict` on any word list |
| `MORFOLOGIK_SPELLER_ORACLE` | `oracle speller <dict> [distance]` on any word list |
| `MORFOLOGIK_SPELLER_SHARED_ORACLE` | `oracle speller-shared <dict> [distance]` |
| `MORFOLOGIK_SPELLER_DICT`, `MORFOLOGIK_SPELLER_DISTANCE` | dictionary and edit distance of the two speller files |
| `MORFOLOGIK_CHARS_ORACLE` | `oracle chars`: `java.lang.Character` over the BMP |

```sh
oracle dump assets/fr/lt/french.dict > /tmp/french.dump.tsv
MORFOLOGIK_DUMP_ORACLE=/tmp/french.dump.tsv \
  cargo test --release --test morfologik dump_matches_java -- --nocapture
```

Performance figures (load time, memory, lookups per second):

```sh
cargo test --release --test morfologik -- --ignored --nocapture
```

## Unicode tables

`src/morfologik/unicode_tables.rs` is generated from the JDK's character data:

```sh
oracle nfd > nfd.txt; oracle otheralpha > oa.txt; oracle letternumbers > nl.txt
python3 tools/morfologik/gen_unicode_tables.py nfd.txt oa.txt nl.txt src/morfologik/unicode_tables.rs
```
