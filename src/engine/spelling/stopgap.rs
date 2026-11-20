//! STOPGAP tagger and word tokenizer, to be replaced by the engine's ports of
//! `org.languagetool.tagging.fr.FrenchTagger` and
//! `org.languagetool.tokenizers.fr.FrenchWordTokenizer` once they are merged.
//!
//! The spelling classes only see them as an `Arc<dyn WordTagger>` and a
//! [`Tokenize`](super::Tokenize) closure, so swapping them is a change in the
//! code that builds the speller, not here.
//!
//! * [`StopgapTagger`] follows `FrenchTagger.tag` for one word: the word as
//!   written, its lowercase form when it is capitalised or in capitals, its
//!   title-case form when in capitals, `œ` for `oe`; readings come from the
//!   dictionary and `added.txt`, minus `removed.txt`. The productive prefixes
//!   (`FrenchTagger.additionalTags`) are **not** handled.
//! * [`StopgapTokenizer`] follows `FrenchWordTokenizer.tokenize`, without the
//!   final joining of e-mail addresses and URLs.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use super::super::pattern::WordTagger;
use super::super::strtools;
use super::super::token::{Reading, Readings};
use super::strings::{java_is_letter, java_trim};
use super::wordlist::{java_lines, read_resource};
use crate::morfologik::{Dictionary, WordData};
use crate::re::{self, Flags, Regex};

/// `ManualTagger`: `form<TAB>lemma<TAB>tag` lines.
fn manual_entries(dir: &Path, names: &[&str]) -> HashMap<String, Vec<(String, String)>> {
    let mut map: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for name in names {
        let Some(content) = read_resource(&dir.join(name)) else { continue };
        for line in java_lines(&content) {
            let line = java_trim(line);
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = line.split('#').next().unwrap_or("");
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 3 {
                continue;
            }
            map.entry(parts[0].to_string()).or_default().push((parts[1].to_string(), java_trim(parts[2]).to_string()));
        }
    }
    map
}

/// STOPGAP for `FrenchTagger` (see the module documentation).
pub struct StopgapTagger {
    dictionary: Arc<Dictionary>,
    manual: HashMap<String, Vec<(String, String)>>,
    removal: HashMap<String, Vec<(String, String)>>,
}

impl StopgapTagger {
    pub fn new(dictionary: Arc<Dictionary>, assets: &Path) -> StopgapTagger {
        StopgapTagger {
            dictionary,
            manual: manual_entries(assets, &["added.txt", "added_custom.txt"]),
            removal: manual_entries(assets, &["removed.txt", "removed_custom.txt"]),
        }
    }

    /// `CombiningTagger.tag`: the manual readings, then the dictionary's,
    /// minus the removed ones; as `(lemma, tag)` pairs.
    fn word_tagger(&self, word: &str, out: &mut Vec<(String, String)>) {
        let start = out.len();
        if let Some(manual) = self.manual.get(word) {
            out.extend(manual.iter().cloned());
        }
        let mut found: Vec<WordData> = Vec::new();
        self.dictionary.lookup(word, &mut found);
        out.extend(found.into_iter().map(|w| (w.stem, w.tag)));
        if let Some(removed) = self.removal.get(word) {
            let mut kept: Vec<(String, String)> = out.drain(start..).filter(|r| !removed.contains(r)).collect();
            out.append(&mut kept);
        }
    }

    /// `FrenchTagger.tagWord`, without `additionalTags`.
    fn tag_word_readings(&self, word: &str) -> Vec<(String, String)> {
        const AMBIGUOUS: [&str; 11] = ["-Le", "-Les", "-La", "-Elle", "-Elles", "-On", "-Tu", "-Vous", "-Il", "-Ils", "-Ce"];
        let mut l = Vec::new();
        let lower = word.to_lowercase();
        let is_start_upper = strtools::is_capitalized_word(word);
        let is_all_upper = strtools::is_all_uppercase(word);
        let is_hyphenated_title_case =
            !AMBIGUOUS.contains(&word) && word.contains('-') && word == strtools::convert_to_title_case_iterating_chars(&lower);
        self.word_tagger(word, &mut l);
        if is_all_upper || is_start_upper || is_hyphenated_title_case {
            self.word_tagger(&lower, &mut l);
        }
        if l.is_empty() && is_all_upper {
            self.word_tagger(&strtools::convert_to_title_case_iterating_chars(&lower), &mut l);
        }
        l
    }

    /// True when the word has a reading (`tag(word).isTagged()`).
    pub fn is_tagged(&self, word: &str) -> bool {
        self.tag_word(word).is_tagged()
    }
}

impl WordTagger for StopgapTagger {
    /// `FrenchTagger.tag` for a single token.
    fn tag_word(&self, word: &str) -> Readings {
        let replaced;
        let mut word = word;
        if strtools::utf16_len(word) > 1 && word.contains('’') {
            replaced = word.replace('’', "'");
            word = &replaced;
        }
        let mut l = self.tag_word_readings(word);
        if l.is_empty() && word.to_lowercase().contains("oe") {
            l = self.tag_word_readings(&word.replace("oe", "œ").replace("OE", "Œ"));
        }
        if l.is_empty() && strtools::is_emoji(word) {
            l.push(("_emoji_".to_string(), "_emoji_".to_string()));
        }
        if l.is_empty() {
            return Readings::untagged(word, 0);
        }
        let readings: Vec<Reading> = l.iter().map(|(lemma, tag)| Reading::from_strs(word, Some(tag), Some(lemma))).collect();
        Readings::new(readings, 0)
    }
}

/// `FrenchWordTokenizer.wordCharacters`.
fn is_word_character(c: char) -> bool {
    matches!(c, '§' | '©' | '@' | '€' | '£' | '$' | '_' | '-' | '\u{a8}' | '°' | '%' | '‰' | '‱' | '&' | '\u{fffd}' | '\u{ad}' | '\u{ac}')
        || c.is_ascii_digit()
        || matches!(c as u32, 0x300..=0x36F | 0x2070..=0x209F)
        || java_is_letter(c)
}

struct TokenizerPatterns {
    typewriter_apostrophe: Arc<Regex>,
    typographic_apostrophe: Arc<Regex>,
    nearby_hyphens: Arc<Regex>,
    hyphens: Arc<Regex>,
    decimal_point: Arc<Regex>,
    decimal_comma: Arc<Regex>,
    space_digits0: Arc<Regex>,
    space_digits: Arc<Regex>,
    space_digits2: Arc<Regex>,
    /// `FrenchWordTokenizer.patterns`: how a chunk with an elision or a
    /// clitic is cut.
    splits: Vec<Arc<Regex>>,
}

fn tokenizer_patterns() -> &'static TokenizerPatterns {
    static PATTERNS: OnceLock<TokenizerPatterns> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let ci = |src: &str| re::compile(src, Flags::IGNORE_CASE).expect("valid tokenizer pattern");
        const CLITICS: &str = "-ce|-elle|-t-elle|-elles|-t-elles|-en|-il|-t-il|-ils|-t-ils|-je|-la|-le|-les|-leur|-lui|-moi|-nous|-on|-t-on|-toi|-tu|-vous|-vs|-y";
        const CLITICS_T_FIRST: &str = "-ce|-t-elle|-t-elles|-elle|-elles|-en|-il|-t-il|-ils|-t-ils|-je|-la|-le|-les|-leur|-lui|-moi|-nous|-on|-t-on|-toi|-tu|-vous|-vs|-y";
        const ELISIONS: &str = "[cç]['’]|j['’]|n['’]|m['’]|t['’]|s['’]|l['’]|d['’]|qu['’]|jusqu['’]|lorsqu['’]|puisqu['’]|quoiqu['’]";
        TokenizerPatterns {
            typewriter_apostrophe: ci("([\\p{L}])'([\\p{L}1\"‘“«])"),
            typographic_apostrophe: ci("([\\p{L}])’([\\p{L}1\"‘“«])"),
            nearby_hyphens: ci("([\\p{L}])-([\\p{L}])-([\\p{L}])"),
            hyphens: ci("([\\p{L}])-([\\p{L}\\d])"),
            decimal_point: ci("([\\d])\\.([\\d])"),
            decimal_comma: ci("([\\d]),([\\d])"),
            space_digits0: ci("([\\d]{4}) "),
            space_digits: ci("([\\d]) ([\\d][\\d][\\d])\\b"),
            space_digits2: ci("([\\d]) ([\\d][\\d][\\d]) ([\\d][\\d][\\d])\\b"),
            splits: vec![
                ci("^(c['’]te?|m['’]as-tu-vu|c['’]est-à-dire|add-on|add-ons|rendez-vous|garde-à-vous|chez-eux|chez-moi|chez-nous|chez-soi|chez-toi|chez-vous)$"),
                ci(&format!("^({ELISIONS})([^\\-]*)({CLITICS})$")),
                ci(&format!("^({ELISIONS})([^'’\\-].*)$")),
                ci(&format!("^([^\\-\\d]+)({CLITICS_T_FIRST})({CLITICS})$")),
                ci("^([^\\-]*)(-t|-m)(['’]en|['’]y)$"),
                ci("^(.*)(-t-elle|-t-elles|-t-il|-t-ils|-t-on)$"),
                ci(&format!("^(.*)({CLITICS})$")),
            ],
        }
    })
}

/// STOPGAP for `FrenchWordTokenizer` (see the module documentation).
pub struct StopgapTokenizer {
    tagger: Arc<StopgapTagger>,
}

impl StopgapTokenizer {
    pub fn new(tagger: Arc<StopgapTagger>) -> StopgapTokenizer {
        StopgapTokenizer { tagger }
    }

    /// `FrenchWordTokenizer.tokenize`.
    pub fn tokenize(&self, text: &str) -> Vec<String> {
        // A run of letters and digits is one token whatever the rules below.
        if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit() || java_is_letter(c)) {
            return vec![text.to_string()];
        }
        const DO_NOT_SPLIT: [&str; 22] = [
            "mers-cov", "mcgraw-hill", "sars-cov-2", "sars-cov", "ph-metre", "ph-metres", "anti-ivg", "anti-uv", "anti-vih", "al-qaïda",
            "c'est-à-dire", "add-on", "add-ons", "rendez-vous", "garde-à-vous", "chez-eux", "chez-moi", "chez-nous", "chez-soi", "chez-toi",
            "chez-vous", "m'as-tu-vu",
        ];
        let p = tokenizer_patterns();
        let mut l: Vec<String> = Vec::new();
        let mut aux = text.replace(['\u{2010}', '\u{2011}'], "-");
        // Each rewriting needs the character it is about: skip the others.
        let has_digit = aux.chars().any(|c| c.is_ascii_digit());
        if aux.contains('\'') {
            aux = p.typewriter_apostrophe.replace_all(&aux, "$1xxFR_APOS_TYPEWxx$2");
        }
        if aux.contains('’') {
            aux = p.typographic_apostrophe.replace_all(&aux, "$1xxFR_APOS_TYPOGxx$2");
        }
        if aux.contains('-') {
            aux = p.nearby_hyphens.replace_all(&aux, "$1xxFR_HYPHENxx$2xxFR_HYPHENxx$3");
            aux = p.hyphens.replace_all(&aux, "$1xxFR_HYPHENxx$2");
        }
        if has_digit {
            if aux.contains('.') {
                aux = p.decimal_point.replace_all(&aux, "$1xxFR_DECIMALPOINTxx$2");
            }
            if aux.contains(',') {
                aux = p.decimal_comma.replace_all(&aux, "$1xxFR_DECIMALCOMMAxx$2");
            }
            if aux.contains(' ') {
                aux = p.space_digits2.replace_all(&aux, "$1xxFR_SPACExx$2xxFR_SPACExx$3");
                aux = p.space_digits0.replace_all(&aux, "$1xxFR_SPACE0xx");
                aux = p.space_digits.replace_all(&aux, "$1xxFR_SPACExx$2");
                aux = aux.replace("xxFR_SPACE0xx", " ");
            }
        }
        // `tokenizerPattern`: a run of word characters, or one other character.
        let chars: Vec<char> = aux.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let start = i;
            if is_word_character(chars[i]) {
                while i < chars.len() && is_word_character(chars[i]) {
                    i += 1;
                }
            } else {
                i += 1;
            }
            let chunk: String = chars[start..i].iter().collect();
            if !l.is_empty() && i - start == 1 && matches!(chars[start] as u32, 0xFE00..=0xFE0F) {
                // A variation selector stays with the previous token.
                l.last_mut().unwrap().push_str(&chunk);
                continue;
            }
            let mut s = chunk
                .replace("xxFR_APOS_TYPEWxx", "'")
                .replace("xxFR_APOS_TYPOGxx", "’")
                .replace("xxFR_HYPHENxx", "-")
                .replace("xxFR_DECIMALPOINTxx", ".")
                .replace("xxFR_DECIMALCOMMAxx", ",")
                .replace("xxFR_SPACExx", " ");
            while s.chars().count() > 1 && s.starts_with('-') {
                l.push("-".to_string());
                s.remove(0);
            }
            let mut hyphens_at_end = 0;
            while s.chars().count() > 1 && s.ends_with('-') {
                s.pop();
                hyphens_at_end += 1;
            }
            let mut groups: Option<Vec<String>> = None;
            if s.contains(['\'', '’', '-']) {
                let s_chars: Vec<char> = s.chars().collect();
                for pattern in &p.splits {
                    if let Some(caps) = pattern.find_chars(&s_chars, 0) {
                        groups = Some((1..=pattern.group_count()).map(|g| caps.get(&s_chars, g).unwrap_or_default()).collect());
                        break;
                    }
                }
            }
            match groups {
                Some(groups) => {
                    for group in groups {
                        self.words_to_add(&group, &DO_NOT_SPLIT, &mut l);
                    }
                }
                None => self.words_to_add(&s, &DO_NOT_SPLIT, &mut l),
            }
            for _ in 0..hyphens_at_end {
                l.push("-".to_string());
            }
        }
        l
    }

    /// `wordsToAdd`: a hyphenated chunk stays whole when the tagger knows it,
    /// and is cut at its hyphens otherwise.
    fn words_to_add(&self, s: &str, do_not_split: &[&str], l: &mut Vec<String>) {
        if s.is_empty() {
            return;
        }
        if !s.contains('-') {
            l.push(s.to_string());
            return;
        }
        let normalized = s.replace('\u{ad}', "").replace('’', "'");
        if self.tagger.is_tagged(&normalized) || do_not_split.contains(&s.to_lowercase().as_str()) {
            l.push(s.to_string());
            return;
        }
        // `new StringTokenizer(s, "-", true)`: the pieces and the hyphens.
        let mut piece = String::new();
        for c in s.chars() {
            if c == '-' {
                if !piece.is_empty() {
                    l.push(std::mem::take(&mut piece));
                }
                l.push("-".to_string());
            } else {
                piece.push(c);
            }
        }
        if !piece.is_empty() {
            l.push(piece);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> Option<(Arc<StopgapTagger>, StopgapTokenizer)> {
        let dir = Path::new("assets/fr/lt");
        let dictionary = Arc::new(Dictionary::load(dir.join("french.dict"), dir.join("french.info")).ok()?);
        let tagger = Arc::new(StopgapTagger::new(dictionary, dir));
        Some((tagger.clone(), StopgapTokenizer::new(tagger)))
    }

    #[test]
    fn tags_words_as_written_and_lowercased() {
        let Some((tagger, _)) = tools() else { return };
        assert!(tagger.tag_word("maison").has_pos_tag("N f s"));
        assert!(tagger.tag_word("Maison").is_tagged());
        assert!(tagger.tag_word("MAISON").is_tagged());
        assert!(tagger.tag_word("oeuvre").is_tagged(), "oe stands for œ");
        assert!(!tagger.tag_word("connaisance").is_tagged());
        assert!(!tagger.tag_word("maiSon").is_tagged());
        assert_eq!(&*tagger.tag_word("l’homme").token, "l'homme");
    }

    #[test]
    fn tokenizes_elisions_clitics_and_hyphens() {
        let Some((_, tokenizer)) = tools() else { return };
        assert_eq!(tokenizer.tokenize("maison"), ["maison"]);
        assert_eq!(tokenizer.tokenize("l'homme"), ["l'", "homme"]);
        assert_eq!(tokenizer.tokenize("qu’il"), ["qu’", "il"]);
        assert_eq!(tokenizer.tokenize("dit-il"), ["dit", "-il"]);
        assert_eq!(tokenizer.tokenize("a-t-il"), ["a", "-t-il"]);
        assert_eq!(tokenizer.tokenize("peut-être"), ["peut-être"]);
        assert_eq!(tokenizer.tokenize("Joe Biden"), ["Joe", " ", "Biden"]);
        assert_eq!(tokenizer.tokenize("Guillain-Barrx"), ["Guillain", "-", "Barrx"]);
        assert_eq!(tokenizer.tokenize("P. Sherman"), ["P", ".", " ", "Sherman"]);
        assert_eq!(tokenizer.tokenize("3,5 %"), ["3,5", " ", "%"]);
        assert_eq!(tokenizer.tokenize("B&B"), ["B&B"]);
        assert_eq!(tokenizer.tokenize("rendez-vous"), ["rendez-vous"]);
    }
}
