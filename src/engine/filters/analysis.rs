//! Analysing a text from inside a filter: what `JLanguageTool.analyzeText`
//! gives `WordWithDeterminerFilter` and `SuppressIfAnyRuleMatchesFilter`.
//!
//! [`SentenceAnalyzer`] is the hook. [`SimpleDictTagger`] and
//! [`SimpleAnalyzer`] are STOPGAPS, to be replaced by the engine's own
//! tokenizer and tagger: reduced ports of
//! `org.languagetool.tagging.fr.FrenchTagger`,
//! `org.languagetool.tokenizers.fr.FrenchWordTokenizer` and
//! `JLanguageTool.getAnalyzedSentence`, good for the short corrected phrases
//! filters check ("l'homme", "cet arbre", "de la maison").

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use super::static_regex;
use crate::engine::disambiguator::Disambiguator;
use crate::engine::pattern::WordTagger;
use crate::engine::rule::RuleContext;
use crate::engine::strtools;
use crate::engine::token::{Reading, Readings, Sentence, SENTENCE_START};
use crate::morfologik::{Dictionary, WordData};
use crate::re::Flags;

/// `JLanguageTool.analyzeText(text).get(0)`: the first sentence of a text,
/// tokenized, tagged and disambiguated.
pub trait SentenceAnalyzer: Send + Sync {
    fn analyze(&self, text: &str, ctx: &RuleContext) -> Option<Sentence>;
}

/// STOPGAP tagger over `french.dict`, `added.txt` and `removed.txt`: the
/// word, then its lowercase form when it is capitalized or in capitals. The
/// prefix analyses of `FrenchTagger.additionalTags` are left out.
pub struct SimpleDictTagger {
    dict: Dictionary,
    /// `added.txt`: form -> (lemma, tag).
    added: HashMap<String, Vec<(String, String)>>,
    removed: HashMap<String, Vec<(String, String)>>,
}

fn manual_entries(content: &str) -> HashMap<String, Vec<(String, String)>> {
    let mut map: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for line in content.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let parts: Vec<&str> = if line.contains('\t') { line.split('\t').collect() } else { line.split(';').collect() };
        if let [form, lemma, tag] = parts[..] {
            map.entry(form.to_string()).or_default().push((lemma.to_string(), tag.trim().to_string()));
        }
    }
    map
}

impl SimpleDictTagger {
    pub fn load(dir: &Path) -> std::io::Result<SimpleDictTagger> {
        let dict = Dictionary::load(dir.join("french.dict"), dir.join("french.info"))?;
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).map(|c| manual_entries(&c)).unwrap_or_default();
        Ok(SimpleDictTagger { dict, added: read("added.txt"), removed: read("removed.txt") })
    }

    /// `CombiningTagger.tag`: manual entries, then the dictionary's, minus
    /// the removed ones.
    fn lookup(&self, word: &str, original: &str, out: &mut Vec<Reading>) {
        let mut found: Vec<WordData> = Vec::new();
        self.dict.lookup(word, &mut found);
        let added = self.added.get(word).into_iter().flatten().map(|(l, t)| (l.as_str(), t.as_str()));
        let removed = self.removed.get(word);
        for (lemma, tag) in added.chain(found.iter().map(|w| (w.stem.as_str(), w.tag.as_str()))) {
            if removed.is_some_and(|r| r.iter().any(|(l, t)| l == lemma && t == tag)) {
                continue;
            }
            out.push(Reading::from_strs(original, Some(tag), Some(lemma)));
        }
    }
}

impl WordTagger for SimpleDictTagger {
    fn tag_word(&self, original: &str) -> Readings {
        let chars = original.chars().count();
        let typewriter = chars > 1 && original.contains('\'');
        let typographic = chars > 1 && original.contains('’');
        let word = if typographic { original.replace('’', "'") } else { original.to_string() };
        let mut readings: Vec<Reading> = Vec::new();
        for candidate in [word.clone(), word.replace("oe", "œ").replace("OE", "Œ")] {
            let lower = candidate.to_lowercase();
            let is_all_upper = strtools::is_all_uppercase(&candidate);
            self.lookup(&candidate, &word, &mut readings);
            // Also for a word without letters: "." gets its reading twice.
            if is_all_upper || strtools::is_capitalized_word(&candidate) {
                self.lookup(&lower, &word, &mut readings);
            }
            if readings.is_empty() && is_all_upper {
                self.lookup(&strtools::convert_to_title_case_iterating_chars(&lower), &word, &mut readings);
            }
            if !readings.is_empty() || !lower.contains("oe") {
                break;
            }
        }
        if readings.is_empty() && strtools::is_emoji(&word) {
            readings.push(Reading::from_strs(&word, Some("_emoji_"), Some("_emoji_")));
        }
        if readings.is_empty() {
            readings.push(Reading::from_strs(&word, None, None));
        }
        let mut tagged = Readings::new(readings, 0);
        if typographic {
            tagged.chunk_tags = vec![Arc::from("containsTypographicApostrophe")];
        } else if typewriter {
            tagged.chunk_tags = vec![Arc::from("containsTypewriterApostrophe")];
        }
        tagged
    }
}

/// `FrenchWordTokenizer.wordCharacters`.
fn is_word_char(c: char) -> bool {
    c.is_alphabetic()
        || c.is_ascii_digit()
        || matches!(c, '§' | '©' | '@' | '€' | '£' | '$' | '_' | '-' | '\u{a8}' | '°' | '%' | '‰' | '‱' | '&' | '\u{fffd}' | '\u{ad}' | '\u{ac}')
        || ('\u{300}'..='\u{36f}').contains(&c)
        || ('\u{2070}'..='\u{209f}').contains(&c)
}

const SUFFIX: &str = "-ce|-elle|-t-elle|-elles|-t-elles|-en|-il|-t-il|-ils|-t-ils|-je|-la|-le|-les|-leur|-lui|-moi|-nous|-on|-t-on|-toi|-tu|-vous|-vs|-y";
const ELIDED: &str = "[cç]['’]|j['’]|n['’]|m['’]|t['’]|s['’]|l['’]|d['’]|qu['’]|jusqu['’]|lorsqu['’]|puisqu['’]|quoiqu['’]";

static_regex!(
    whole_word,
    "(c['’]te?|m['’]as-tu-vu|c['’]est-à-dire|add-on|add-ons|rendez-vous|garde-à-vous|chez-eux|chez-moi|chez-nous|chez-soi|chez-toi|chez-vous)",
    Flags::IGNORE_CASE
);
static_regex!(elided_verb_pronoun, &format!("({ELIDED})([^\\-]*)({SUFFIX})"), Flags::IGNORE_CASE);
static_regex!(elided_word, &format!("({ELIDED})([^'’\\-].*)"), Flags::IGNORE_CASE);
static_regex!(verb_two_pronouns, &format!("([^\\-\\d]+)({SUFFIX})({SUFFIX})"), Flags::IGNORE_CASE);
static_regex!(verb_t_en, "([^\\-]*)(-t|-m)(['’]en|['’]y)", Flags::IGNORE_CASE);
static_regex!(verb_t_pronoun, "(.*)(-t-elle|-t-elles|-t-il|-t-ils|-t-on)", Flags::IGNORE_CASE);
static_regex!(verb_pronoun, &format!("(.*)({SUFFIX})"), Flags::IGNORE_CASE);

const DO_NOT_SPLIT: [&str; 22] = [
    "mers-cov",
    "mcgraw-hill",
    "sars-cov-2",
    "sars-cov",
    "ph-metre",
    "ph-metres",
    "anti-ivg",
    "anti-uv",
    "anti-vih",
    "al-qaïda",
    "c'est-à-dire",
    "add-on",
    "add-ons",
    "rendez-vous",
    "garde-à-vous",
    "chez-eux",
    "chez-moi",
    "chez-nous",
    "chez-soi",
    "chez-toi",
    "chez-vous",
    "m'as-tu-vu",
];

/// `wordsToAdd`: a hyphenated word is kept when the tagger knows it, split
/// at its hyphens otherwise.
fn words_to_add(s: &str, tagger: &dyn WordTagger, out: &mut Vec<String>) {
    if s.is_empty() {
        return;
    }
    if !s.contains('-') {
        out.push(s.to_string());
        return;
    }
    let normalized = s.replace('\u{ad}', "").replace('’', "'");
    if tagger.tag_word(&normalized).is_tagged() || DO_NOT_SPLIT.contains(&s.to_lowercase().as_str()) {
        out.push(s.to_string());
        return;
    }
    let mut rest = s;
    while let Some(p) = rest.find('-') {
        if p > 0 {
            out.push(rest[..p].to_string());
        }
        out.push("-".to_string());
        rest = &rest[p + 1..];
    }
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
}

/// STOPGAP for `FrenchWordTokenizer.tokenize`: elisions, verb-pronoun
/// hyphens and hyphenated words are handled as LanguageTool does; number
/// grouping, e-mail addresses and URLs are not.
pub fn tokenize(text: &str, tagger: &dyn WordTagger) -> Vec<String> {
    let chars: Vec<char> = text.chars().map(|c| if matches!(c, '\u{2010}' | '\u{2011}') { '-' } else { c }).collect();
    // An apostrophe between a letter and a letter (or an opening quote), a
    // point or comma between digits, stay inside the word.
    let inside_word = |i: usize| -> bool {
        let c = chars[i];
        if is_word_char(c) {
            return true;
        }
        let (Some(&prev), Some(&next)) = (i.checked_sub(1).and_then(|p| chars.get(p)), chars.get(i + 1)) else { return false };
        match c {
            '\'' | '’' => prev.is_alphabetic() && (next.is_alphabetic() || matches!(next, '1' | '"' | '‘' | '“' | '«')),
            '.' | ',' => prev.is_ascii_digit() && next.is_ascii_digit(),
            _ => false,
        }
    };
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !inside_word(i) {
            out.push(chars[i].to_string());
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && inside_word(i) {
            i += 1;
        }
        let mut word: &[char] = &chars[start..i];
        while word.len() > 1 && word[0] == '-' {
            out.push("-".to_string());
            word = &word[1..];
        }
        let mut hyphens_at_end = 0;
        while word.len() > 1 && word[word.len() - 1] == '-' {
            word = &word[..word.len() - 1];
            hyphens_at_end += 1;
        }
        let patterns = [whole_word(), elided_verb_pronoun(), elided_word(), verb_two_pronouns(), verb_t_en(), verb_t_pronoun(), verb_pronoun()];
        match patterns.iter().find_map(|p| p.full_match_chars(word).map(|caps| (p, caps))) {
            Some((p, caps)) => {
                for g in 1..=p.group_count() {
                    if let Some(group) = caps.get(word, g) {
                        words_to_add(&group, tagger, &mut out);
                    }
                }
            }
            None => words_to_add(&word.iter().collect::<String>(), tagger, &mut out),
        }
        out.extend(std::iter::repeat("-".to_string()).take(hyphens_at_end));
    }
    out
}

/// STOPGAP for `JLanguageTool.getAnalyzedSentence` on a one-sentence text:
/// [`tokenize`], the context's tagger, then the disambiguator.
pub struct SimpleAnalyzer {
    disambiguator: Arc<Disambiguator>,
}

impl SimpleAnalyzer {
    pub fn new(disambiguator: Arc<Disambiguator>) -> SimpleAnalyzer {
        SimpleAnalyzer { disambiguator }
    }
}

/// `getRawAnalyzedSentence`: the tagged tokens after a `SENT_START` token,
/// the last word marked as the end of the sentence.
pub fn raw_sentence(text: &str, tagger: &dyn WordTagger) -> Sentence {
    let mut tokens: Vec<Readings> = vec![Readings::new(vec![Reading::from_strs("", Some(SENTENCE_START), None)], 0)];
    let mut pos = 0;
    let mut prev: Option<Arc<str>> = None;
    for word in tokenize(text, tagger) {
        let mut t = tagger.tag_word(&word);
        t.start_pos = pos;
        if let Some(p) = &prev {
            t.set_whitespace_before(p);
        }
        pos += word.len();
        prev = Some(Arc::from(word.as_str()));
        tokens.push(t);
    }
    let last = tokens.iter().rposition(|t| !t.is_whitespace).filter(|&i| i > 0).unwrap_or(tokens.len() - 1);
    tokens[last].set_sent_end();
    if last + 1 == tokens.len() && tokens[last].is_linebreak {
        tokens[last].set_paragraph_end();
    }
    let pre = tokens.clone();
    Sentence::new(Arc::from(text), tokens, pre)
}

impl SentenceAnalyzer for SimpleAnalyzer {
    fn analyze(&self, text: &str, ctx: &RuleContext) -> Option<Sentence> {
        let tagger = ctx.lang.tagger.as_deref()?;
        let mut sentence = self.disambiguator.disambiguate(raw_sentence(text, tagger), ctx);
        // `markAsParagraphEnd`: the text ends here.
        if let Some(t) = sentence.tokens.last_mut() {
            t.set_paragraph_end();
        }
        Some(sentence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::filters::testing::ASSETS;

    fn tagger() -> Option<SimpleDictTagger> {
        SimpleDictTagger::load(Path::new(ASSETS)).ok()
    }

    fn tags(t: &Readings) -> Vec<(Option<&str>, Option<&str>)> {
        t.readings.iter().map(|r| (r.lemma.as_deref(), r.pos())).collect()
    }

    #[test]
    fn tags_words_like_the_dictionary() {
        let Some(t) = tagger() else { return };
        assert_eq!(tags(&t.tag_word("du")), [(Some("de+le"), Some("P+D m s"))]);
        assert_eq!(tags(&t.tag_word("Maison")), [(Some("maison"), Some("N f s"))]);
        assert_eq!(tags(&t.tag_word(".")), [(Some("."), Some("M fin")), (Some("."), Some("M fin"))]);
        assert!(t.tag_word("l'").is_tagged());
        assert_eq!(t.tag_word("l'").chunk_tags.len(), 1);
        let unknown = t.tag_word("xyzzyx");
        assert!(!unknown.is_tagged());
        assert_eq!(unknown.readings.len(), 1);
    }

    #[test]
    fn tokenizes_elisions_and_hyphens() {
        let Some(t) = tagger() else { return };
        let tok = |s: &str| tokenize(s, &t);
        assert_eq!(tok("l'homme"), ["l'", "homme"]);
        assert_eq!(tok("à l'arbre"), ["à", " ", "l'", "arbre"]);
        assert_eq!(tok("Cet homme."), ["Cet", " ", "homme", "."]);
        assert_eq!(tok("grand-mère"), ["grand-mère"]);
        assert_eq!(tok("vingt-et-une questions"), ["vingt-et-une", " ", "questions"]);
        assert_eq!(tok("dit-il"), ["dit", "-il"]);
        assert_eq!(tok("a-t-elle"), ["a", "-t-elle"]);
        assert_eq!(tok("qu'est-ce"), ["qu'", "est", "-ce"]);
        assert_eq!(tok("xyzz-abcc"), ["xyzz", "-", "abcc"]);
        assert_eq!(tok("3,5 kg"), ["3,5", " ", "kg"]);
    }

    #[test]
    fn raw_sentences_are_framed() {
        let Some(t) = tagger() else { return };
        let s = raw_sentence("L'homme dort", &t);
        assert!(s.tokens[0].is_sent_start);
        let words: Vec<&str> = s.non_ws_tokens().iter().map(|t| &*t.token).collect();
        assert_eq!(words, ["", "L'", "homme", "dort"]);
        assert_eq!(s.tok(2).start_pos, 2);
        assert!(s.tok(3).whitespace_before && !s.tok(2).whitespace_before);
        assert!(s.tok(3).is_sent_end);
    }
}
