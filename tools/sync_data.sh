#!/bin/sh
# Copy the canonical rule/data assets into ./data (the runtime directory).
set -eu
mkdir -p data
cp assets/fr/languagetool-grammar.xml data/fr.grammar.xml
cp assets/fr/languagetool-style.xml data/fr.style.xml
cp assets/fr/languagetool-disambiguation.xml data/fr.disambig.xml
echo "data synced"
