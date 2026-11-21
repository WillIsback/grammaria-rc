//! Part-of-speech tagging.
//!
//! Port of `org.languagetool.tagging.fr.FrenchTagger`, with what it uses of
//! `org.languagetool.tagging.BaseTagger`, `CombiningTagger` and
//! `ManualTagger` (LanguageTool, LGPL-2.1-or-later).
//!
//! The tagger gives every token its readings (lemma and part-of-speech tag).
//! They come from the dictionary and from `added.txt` (minus `removed.txt`),
//! looked up with the token as written, then in lower case when the token is
//! capitalised or in capitals, then with a few French fallbacks (`oe` for
//! `œ`, productive prefixes, emoji).

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::io;
use std::path::Path;
use std::sync::{Arc, RwLock};

use crate::re::{Flags, Regex};

use super::strtools;
use super::token::{Reading, Readings, Tag};

/// The binary dictionary of the tagger (LanguageTool's `MorfologikTagger`
/// over `french.dict`), behind a trait so that its storage can change.
pub trait WordDictionary: Send + Sync {
    /// Appends the (lemma, postag) readings of `word` to `out`, in dictionary
    /// order. The lookup is exact: no case folding.
    fn lookup(&self, word: &str, out: &mut Vec<(String, String)>);
}

// ---------------------------------------------------------------------------
// A dictionary read from a text dump
// ---------------------------------------------------------------------------

#[inline]
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// A [`WordDictionary`] over a text dump of the dictionary: one reading per
/// line, `form<TAB>lemma<TAB>postag`, the readings of a form on consecutive
/// lines and in dictionary order.
pub struct TextDictionary {
    data: String,
    /// Open-addressing table of line offsets (+1; 0 is an empty slot), one
    /// per distinct form, pointing at its first line.
    table: Vec<u32>,
}

impl TextDictionary {
    pub fn load(path: &Path) -> io::Result<TextDictionary> {
        Ok(TextDictionary::from_text(std::fs::read_to_string(path)?))
    }

    pub fn from_text(data: String) -> TextDictionary {
        assert!(data.len() < u32::MAX as usize, "dictionary dump too large");
        let mut firsts: Vec<u32> = Vec::new();
        let mut previous: &str = "\t";
        let mut offset = 0;
        for line in data.split_inclusive('\n') {
            let form = line.split('\t').next().unwrap_or("");
            if form != previous && line.contains('\t') {
                firsts.push(offset as u32);
                previous = form;
            }
            offset += line.len();
        }
        let size = (firsts.len() * 2).next_power_of_two().max(16);
        let mut table = vec![0u32; size];
        for first in firsts {
            let form = data[first as usize..].split('\t').next().unwrap_or("");
            let mut slot = fnv1a(form.as_bytes()) as usize & (size - 1);
            while table[slot] != 0 {
                slot = (slot + 1) & (size - 1);
            }
            table[slot] = first + 1;
        }
        TextDictionary { data, table }
    }

    /// Offset of the first line of `word`.
    fn find(&self, word: &str) -> Option<usize> {
        let mask = self.table.len() - 1;
        let mut slot = fnv1a(word.as_bytes()) as usize & mask;
        loop {
            match self.table[slot] {
                0 => return None,
                entry => {
                    let offset = entry as usize - 1;
                    let rest = &self.data.as_bytes()[offset..];
                    if rest.len() > word.len()
                        && rest[word.len()] == b'\t'
                        && &rest[..word.len()] == word.as_bytes()
                    {
                        return Some(offset);
                    }
                }
            }
            slot = (slot + 1) & mask;
        }
    }
}

impl WordDictionary for TextDictionary {
    fn lookup(&self, word: &str, out: &mut Vec<(String, String)>) {
        if word.is_empty() || word.contains(['\t', '\n']) {
            return;
        }
        let Some(offset) = self.find(word) else {
            return;
        };
        for line in self.data[offset..].lines() {
            let mut fields = line.split('\t');
            if fields.next() != Some(word) {
                break;
            }
            if let (Some(lemma), Some(tag)) = (fields.next(), fields.next()) {
                out.push((lemma.to_string(), tag.to_string()));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ManualTagger
// ---------------------------------------------------------------------------

/// `ManualTagger`: readings from a plain text file (`added.txt`,
/// `removed.txt`), one `fullform<SEP>baseform<SEP>postag` per line.
#[derive(Debug, Default)]
pub struct ManualTagger {
    map: HashMap<String, Vec<(String, String)>>,
}

impl ManualTagger {
    /// `new ManualTagger(inputStream)`. Several files are read as one stream,
    /// as LanguageTool does (`text` is their concatenation).
    pub fn parse(text: &str) -> Result<ManualTagger, String> {
        let mut map: HashMap<String, Vec<(String, String)>> = HashMap::new();
        let mut separator = "\t".to_string();
        for (index, line) in text.lines().enumerate() {
            let line = java_trim(line);
            if let Some(rest) = line.strip_prefix("#separatorRegExp=") {
                separator = rest.replace("#separatorRegExp=", "");
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.contains('\u{a0}') {
                return Err(format!(
                    "Non-breaking space found in line #{}: '{line}', please remove it",
                    index + 1
                ));
            }
            let line = java_trim(line.split('#').next().unwrap_or(""));
            // String.split(regex): the separators LanguageTool's files use
            // are plain strings; trailing empty fields are dropped.
            let mut parts: Vec<&str> = line.split(separator.as_str()).collect();
            while parts.last() == Some(&"") {
                parts.pop();
            }
            if parts.len() != 3 {
                return Err(format!(
                    "Unknown line format in line {} when loading manual tagger dictionary, expected three tab-separated fields: '{line}'",
                    index + 1
                ));
            }
            map.entry(parts[0].to_string())
                .or_default()
                .push((parts[1].to_string(), java_trim(parts[2]).to_string()));
        }
        Ok(ManualTagger { map })
    }

    /// `tag(word)`: the (lemma, postag) readings of `word`, in file order.
    pub fn tag(&self, word: &str) -> &[(String, String)] {
        self.map.get(word).map_or(&[], Vec::as_slice)
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// `String.trim()`: strips the chars up to U+0020 at both ends.
fn java_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c <= ' ')
}

// ---------------------------------------------------------------------------
// FrenchTagger
// ---------------------------------------------------------------------------

/// `FrenchTagger.ambigousTokens`.
const AMBIGUOUS_TOKENS: [&str; 11] = [
    "-Le", "-Les", "-La", "-Elle", "-Elles", "-On", "-Tu", "-Vous", "-Il", "-Ils", "-Ce",
];

const CONTAINS_TYPEWRITER_APOSTROPHE: &str = "containsTypewriterApostrophe";
const CONTAINS_TYPOGRAPHIC_APOSTROPHE: &str = "containsTypographicApostrophe";

#[derive(Default)]
struct Fnv(u64);

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut h = if self.0 == 0 {
            0xcbf2_9ce4_8422_2325
        } else {
            self.0
        };
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.0 = h;
    }
}

type Shard = RwLock<HashMap<Box<str>, Arc<Readings>, BuildHasherDefault<Fnv>>>;

const SHARDS: usize = 64;
/// A shard that grows past this is emptied: the cache stays bounded on
/// texts with an endless supply of new tokens (numbers, identifiers).
const SHARD_CAPACITY: usize = 2048;
/// Longer tokens (URLs, mostly) are not worth keeping.
const LONGEST_CACHED: usize = 64;

struct Patterns {
    prefixes_for_verbs: Regex,
    prefixes_noun_adj: Regex,
    prefixes_for_noun_adj: Regex,
}

/// `FrenchTagger`.
pub struct FrenchTagger {
    dictionary: Arc<dyn WordDictionary>,
    /// `added.txt` and `added_custom.txt`.
    manual: ManualTagger,
    /// `removed.txt` and `removed_custom.txt`.
    removal: ManualTagger,
    patterns: Patterns,
    /// Tagged tokens by surface form: tagging a token only depends on it.
    cache: Vec<Shard>,
    chunk_typewriter: Arc<str>,
    chunk_typographic: Arc<str>,
}

/// The files of a tagger, read as one stream (`SequenceInputStream`); a
/// missing file is skipped.
fn read_all(dir: &Path, names: &[&str]) -> io::Result<String> {
    let mut text = String::new();
    for name in names {
        match std::fs::read_to_string(dir.join(name)) {
            Ok(content) => text.push_str(&content),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(text)
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

impl FrenchTagger {
    /// A tagger over `dictionary`, with the manual additions and removals
    /// found in `assets_dir` (`added.txt`, `added_custom.txt`, `removed.txt`,
    /// `removed_custom.txt`).
    pub fn new(dictionary: Arc<dyn WordDictionary>, assets_dir: &Path) -> io::Result<FrenchTagger> {
        let added = read_all(assets_dir, &["added.txt", "added_custom.txt"])?;
        let removed = read_all(assets_dir, &["removed.txt", "removed_custom.txt"])?;
        Ok(FrenchTagger::with_manual_data(
            dictionary,
            ManualTagger::parse(&added).map_err(invalid)?,
            ManualTagger::parse(&removed).map_err(invalid)?,
        ))
    }

    pub fn with_manual_data(
        dictionary: Arc<dyn WordDictionary>,
        manual: ManualTagger,
        removal: ManualTagger,
    ) -> FrenchTagger {
        let re = |src: &str| Regex::new(src, Flags::IGNORE_CASE).expect("valid pattern");
        FrenchTagger {
            dictionary,
            manual,
            removal,
            patterns: Patterns {
                prefixes_for_verbs: re("(auto|auto-|re-|sur-)([^-].*[aeiouêàéèíòóïü].+[aeiouêàéèíòóïü].*)"),
                prefixes_noun_adj: re("(post-|sur-|mini-|méga-|demi-|péri-|anti-|géo-|nord-|sud-|néo-|méga-|ultra-|pro-|inter-|micro-|macro-|sous-|haut-|auto-|ré-|pré-|super-|vice-|hyper-|proto-|grand-|pseudo-)(.+)"),
                prefixes_for_noun_adj: re("(mini|méga)([^-].*[aeiouêàéèíòóïü].+[aeiouêàéèíòóïü].*)"),
            },
            cache: (0..SHARDS).map(|_| RwLock::new(HashMap::default())).collect(),
            chunk_typewriter: Arc::from(CONTAINS_TYPEWRITER_APOSTROPHE),
            chunk_typographic: Arc::from(CONTAINS_TYPOGRAPHIC_APOSTROPHE),
        }
    }

    /// `CombiningTagger.tag(word)`: the manual readings, then the
    /// dictionary's, without those `removed.txt` lists.
    pub fn word_tagger(&self, word: &str) -> Vec<(String, String)> {
        let mut result = self.manual.tag(word).to_vec();
        self.dictionary.lookup(word, &mut result);
        let removal = self.removal.tag(word);
        if !removal.is_empty() {
            result.retain(|r| !removal.contains(r));
        }
        result
    }

    /// `FrenchTagger.tag(sentenceTokens)`. `start_pos` is the byte offset of
    /// each token in the concatenation of the tokens.
    pub fn tag(&self, tokens: &[String]) -> Vec<Readings> {
        let mut out = Vec::with_capacity(tokens.len());
        let mut pos = 0;
        for word in tokens {
            let mut readings = self.tag_token(word);
            readings.start_pos = pos;
            pos += word.len();
            out.push(readings);
        }
        out
    }

    /// Is `word` tagged, as a token of its own? (What
    /// `FrenchWordTokenizer.wordsToAdd` asks about a word with hyphens.)
    pub fn is_tagged(&self, word: &str) -> bool {
        self.cached(word).is_tagged()
    }

    /// The readings of one token (one step of `FrenchTagger.tag`), at
    /// position 0.
    pub fn tag_token(&self, word: &str) -> Readings {
        (*self.cached(word)).clone()
    }

    fn cached(&self, word: &str) -> Arc<Readings> {
        // Any spread will do for the shard; the map hashes the word itself.
        let bytes = word.as_bytes();
        let spread = bytes.len() * 31
            + bytes.first().map_or(0, |&b| b as usize) * 7
            + bytes.last().map_or(0, |&b| b as usize);
        let shard = &self.cache[spread % SHARDS];
        if let Some(hit) = shard.read().unwrap_or_else(|e| e.into_inner()).get(word) {
            return hit.clone();
        }
        let readings = Arc::new(self.tag_uncached(word));
        if word.len() > LONGEST_CACHED {
            return readings;
        }
        let mut map = shard.write().unwrap_or_else(|e| e.into_inner());
        if map.len() >= SHARD_CAPACITY {
            map.clear();
        }
        map.insert(Box::from(word), readings.clone());
        readings
    }

    fn tag_uncached(&self, token: &str) -> Readings {
        // This hack allows all rules and dictionary entries to work with
        // typewriter apostrophe
        let mut contains_typewriter_apostrophe = false;
        let mut contains_typographic_apostrophe = false;
        let mut word = std::borrow::Cow::Borrowed(token);
        if strtools::utf16_len(token) > 1 {
            if token.contains('\'') {
                contains_typewriter_apostrophe = true;
            }
            if token.contains('’') {
                contains_typographic_apostrophe = true;
                word = std::borrow::Cow::Owned(token.replace('’', "'"));
            }
        }
        let word: &str = &word;
        let mut l = self.tag_word(word, word);
        if l.is_empty() && word.to_lowercase().contains("oe") {
            l = self.tag_word(&word.replace("oe", "œ").replace("OE", "Œ"), word);
        }
        // emoji
        if l.is_empty() && strtools::is_emoji(word) {
            l.push(Reading::from_strs(word, Some("_emoji_"), Some("_emoji_")));
        }
        if l.is_empty() {
            l.push(Reading::from_strs(word, None, None));
        }
        let mut atr = Readings::new(l, 0);
        if contains_typewriter_apostrophe {
            atr.chunk_tags = vec![self.chunk_typewriter.clone()];
        }
        if contains_typographic_apostrophe {
            atr.chunk_tags = vec![self.chunk_typographic.clone()];
        }
        atr
    }

    /// `asAnalyzedTokenListForTaggedWords` and `addTokens`.
    fn add_tokens(token: &Arc<str>, tagged: Vec<(String, String)>, l: &mut Vec<Reading>) {
        for (lemma, tag) in tagged {
            let lemma: Arc<str> = if *lemma == **token {
                token.clone()
            } else {
                Arc::from(lemma)
            };
            l.push(Reading::new(
                token.clone(),
                Some(Tag::new(&tag)),
                Some(lemma),
            ));
        }
    }

    /// `FrenchTagger.tagWord`.
    fn tag_word(&self, word: &str, original_word: &str) -> Vec<Reading> {
        let mut l = Vec::new();
        let token: Arc<str> = Arc::from(original_word);
        let lower_word = word.to_lowercase();
        let is_start_upper = strtools::is_capitalized_word(word);
        let is_all_upper = strtools::is_all_uppercase(word);
        let is_hyphenated_title_case = !AMBIGUOUS_TOKENS.contains(&original_word)
            && original_word.contains('-')
            && original_word == strtools::convert_to_title_case_iterating_chars(&lower_word);
        // normal case:
        Self::add_tokens(&token, self.word_tagger(word), &mut l);
        // tag non-lowercase (alluppercase, startuppercase, hyphenated title case),
        // but not mixedcase word, with lowercase word tags:
        if is_all_upper || is_start_upper || is_hyphenated_title_case {
            Self::add_tokens(&token, self.word_tagger(&lower_word), &mut l);
        }
        // tag all-uppercase proper nouns (ex. FRANCE)
        if l.is_empty() && is_all_upper {
            let first_upper = strtools::convert_to_title_case_iterating_chars(&lower_word);
            Self::add_tokens(&token, self.word_tagger(&first_upper), &mut l);
        }
        // additional tagging with prefixes removed
        if l.is_empty() {
            if let Some(additional) = self.additional_tags(word) {
                l.extend(additional);
            }
        }
        l
    }

    /// The readings of `candidate` whose tag `keep` accepts, as readings of
    /// `word` with the prefix put back on the lemma.
    fn prefixed(
        &self,
        word: &Arc<str>,
        prefix: &str,
        candidate: &str,
        keep: fn(&str) -> bool,
        out: &mut Vec<Reading>,
    ) {
        for (lemma, tag) in self.word_tagger(candidate) {
            if keep(&tag) {
                let lemma = format!("{}{lemma}", prefix.to_lowercase());
                out.push(Reading::new(
                    word.clone(),
                    Some(Tag::new(&tag)),
                    Some(Arc::from(lemma)),
                ));
            }
        }
    }

    /// `FrenchTagger.additionalTags`: a known word behind a productive
    /// prefix takes the tags of that word.
    fn additional_tags(&self, word: &str) -> Option<Vec<Reading>> {
        // `V .+`
        fn verb(tag: &str) -> bool {
            tag.len() > 2 && tag.starts_with("V ")
        }
        // `[NJ] .+|V ppa.*`
        fn noun_adj(tag: &str) -> bool {
            (tag.len() > 2 && (tag.starts_with("N ") || tag.starts_with("J ")))
                || tag.starts_with("V ppa")
        }
        let chars: Vec<char> = word.chars().collect();
        let groups = |re: &Regex| -> Option<(String, String)> {
            let m = re.full_match_chars(&chars)?;
            Some((m.get(&chars, 1)?, m.get(&chars, 2)?))
        };
        let token: Arc<str> = Arc::from(word);
        let mut additional = Vec::new();
        // Any well-formed verb with prefixes is tagged as a verb copying the original tags
        if let Some((prefix, rest)) = groups(&self.patterns.prefixes_for_verbs) {
            self.prefixed(&token, &prefix, &rest.to_lowercase(), verb, &mut additional);
            if !additional.is_empty() {
                return Some(additional);
            }
        }
        if let Some((prefix, rest)) = groups(&self.patterns.prefixes_for_noun_adj) {
            self.prefixed(
                &token,
                &prefix,
                &rest.to_lowercase(),
                noun_adj,
                &mut additional,
            );
            if !additional.is_empty() {
                return Some(additional);
            }
        }
        if let Some((prefix, rest)) = groups(&self.patterns.prefixes_noun_adj) {
            let possible_noun = rest.to_lowercase();
            self.prefixed(&token, &prefix, &possible_noun, noun_adj, &mut additional);
            // with lower case
            if additional.is_empty() {
                self.prefixed(
                    &token,
                    &prefix,
                    &possible_noun.to_lowercase(),
                    noun_adj,
                    &mut additional,
                );
            }
            return Some(additional);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = "\
,\t,\tM nonfin
-il\til\tR pers suj 3 m s
France\tFrance\tZ f s
Peut-Être\tPeut-Être\tZ m s
a\ta\tN m sp
a\tavoir\tV avoir ind pres 3 s
chat\tchat\tN m s
entreprendre\tentreprendre\tV inf
jupe\tjupe\tN f s
l'\tle\tD e s
marché\tmarché\tN m s
marché\tmarcher\tV ppa m s
peut-être\tpeut-être\tA
président\tprésident\tN m s
sœur\tsœur\tN f s
";

    fn tagger() -> FrenchTagger {
        let dict = Arc::new(TextDictionary::from_text(DUMP.to_string()));
        let manual = ManualTagger::parse(
            "#separatorRegExp=;\nOuigo;Ouigo;Z m sp\nchat;chat;Z m s # comment\n",
        )
        .unwrap();
        let removal = ManualTagger::parse("a\ta\tN m sp\n").unwrap();
        FrenchTagger::with_manual_data(dict, manual, removal)
    }

    fn show(t: &Readings) -> String {
        t.readings
            .iter()
            .map(|r| {
                format!(
                    "{}/{}",
                    r.lemma.as_deref().unwrap_or("-"),
                    r.pos().unwrap_or("-")
                )
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn tag(word: &str) -> String {
        show(&tagger().tag_token(word))
    }

    #[test]
    fn text_dictionary_lookup_is_exact_and_ordered() {
        let d = TextDictionary::from_text(DUMP.to_string());
        let mut out = Vec::new();
        d.lookup("marché", &mut out);
        assert_eq!(
            out,
            [
                ("marché".to_string(), "N m s".to_string()),
                ("marcher".to_string(), "V ppa m s".to_string())
            ]
        );
        out.clear();
        d.lookup("Marché", &mut out);
        d.lookup("march", &mut out);
        d.lookup("", &mut out);
        assert!(out.is_empty());
        d.lookup(",", &mut out);
        d.lookup("sœur", &mut out);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn manual_tagger_reads_separators_and_comments() {
        let m = ManualTagger::parse(
            "# c\n\nun\tun\tD m s\n#separatorRegExp=;\ndeux;deux;K;\n  trois;trois; K  \n",
        )
        .unwrap();
        assert_eq!(m.tag("un"), [("un".to_string(), "D m s".to_string())]);
        assert_eq!(m.tag("deux"), [("deux".to_string(), "K".to_string())]);
        assert_eq!(m.tag("trois"), [("trois".to_string(), "K".to_string())]);
        assert!(m.tag("quatre").is_empty());
        assert!(ManualTagger::parse("un\tun\n").is_err());
    }

    #[test]
    fn combines_manual_dictionary_and_removals() {
        let t = tagger();
        // manual readings come first
        assert_eq!(
            t.word_tagger("chat"),
            [
                ("chat".to_string(), "Z m s".to_string()),
                ("chat".to_string(), "N m s".to_string())
            ]
        );
        // removed.txt takes a reading out
        assert_eq!(
            t.word_tagger("a"),
            [("avoir".to_string(), "V avoir ind pres 3 s".to_string())]
        );
    }

    #[test]
    fn case_fallbacks() {
        assert_eq!(tag("marché"), "marché/N m s marcher/V ppa m s");
        // capitalised: also the readings of the lower-case word
        assert_eq!(tag("Marché"), "marché/N m s marcher/V ppa m s");
        assert_eq!(tag("MARCHÉ"), "marché/N m s marcher/V ppa m s");
        // mixed case: the word as written only
        assert_eq!(tag("maRché"), "-/-");
        // all capitals: the capitalised word as a last resort
        assert_eq!(tag("FRANCE"), "France/Z f s");
        assert_eq!(tag("france"), "-/-");
        // a token without letters counts as upper case: looked up twice
        assert_eq!(tag(","), ",/M nonfin ,/M nonfin");
        // hyphenated title case
        assert_eq!(tag("Peut-Être"), "Peut-Être/Z m s peut-être/A");
        // ...but not for the clitics, which the rules want untagged in this shape
        assert_eq!(tag("-Il"), "-/-");
        assert_eq!(tag("-il"), "il/R pers suj 3 m s");
        assert_eq!(tag(" "), "-/-");
    }

    #[test]
    fn apostrophes_are_looked_up_as_typewriter_apostrophes() {
        let t = tagger();
        let a = t.tag_token("l’");
        assert_eq!(&*a.token, "l'");
        assert_eq!(show(&a), "le/D e s");
        assert_eq!(a.chunk_tags, [Arc::from(CONTAINS_TYPOGRAPHIC_APOSTROPHE)]);
        let b = t.tag_token("L'");
        assert_eq!(show(&b), "le/D e s");
        assert_eq!(b.chunk_tags, [Arc::from(CONTAINS_TYPEWRITER_APOSTROPHE)]);
        assert!(
            t.tag_token("'").chunk_tags.is_empty(),
            "a lone apostrophe is not a word with one"
        );
    }

    #[test]
    fn french_fallbacks() {
        assert_eq!(tag("soeur"), "sœur/N f s");
        assert_eq!(tag("SOEUR"), "sœur/N f s");
        assert_eq!(tag("😂"), "_emoji_/_emoji_");
        // prefixes
        assert_eq!(tag("auto-entreprendre"), "auto-entreprendre/V inf");
        assert_eq!(tag("mini-jupe"), "mini-jupe/N f s");
        assert_eq!(tag("minijupe"), "minijupe/N f s");
        assert_eq!(tag("Vice-Président"), "vice-président/N m s");
        assert_eq!(tag("anti-xyz"), "-/-");
    }

    #[test]
    fn tags_a_sentence_with_positions() {
        let t = tagger();
        let tokens: Vec<String> = ["l’", "chat", " ", "a"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let tagged = t.tag(&tokens);
        assert_eq!(
            tagged.iter().map(|r| r.start_pos).collect::<Vec<_>>(),
            [0, 4, 8, 9]
        );
        assert!(t.is_tagged("peut-être") && !t.is_tagged("peut-etre"));
        assert!(tagged[2].is_whitespace && !tagged[2].is_tagged());
        // cached or not, the same answer
        let long = "x".repeat(LONGEST_CACHED + 1);
        for word in [
            "chat",
            "chat",
            "Marché",
            "Marché",
            long.as_str(),
            long.as_str(),
        ] {
            assert_eq!(show(&t.tag_token(word)), show(&t.tag_uncached(word)));
        }
        // these all fall in one shard, which is emptied when full
        for i in 0..SHARD_CAPACITY + 500 {
            t.tag_token(&format!("w{i:05}x"));
        }
        let sizes: Vec<usize> = t
            .cache
            .iter()
            .map(|shard| shard.read().unwrap().len())
            .collect();
        assert!(sizes.iter().all(|&n| n <= SHARD_CAPACITY) && sizes.contains(&500));
        assert_eq!(show(&t.tag_token("chat")), "chat/Z m s chat/N m s");
    }
}
