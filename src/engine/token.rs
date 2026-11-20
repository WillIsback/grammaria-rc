//! Analysed tokens and sentences, with LanguageTool's data model.
//!
//! A sentence is a list of [`Readings`] (LanguageTool's
//! `AnalyzedTokenReadings`): one per token, whitespace included, framed by a
//! `SENT_START` token. Each holds the possible [`Reading`]s of the token
//! (`AnalyzedToken`: lemma and part-of-speech tag). The last token of a
//! sentence carries an extra `SENT_END` reading (and `PARA_END` at the end of a
//! paragraph): the end of a sentence is a reading, not a token of its own.

use std::sync::Arc;

pub const SENTENCE_START: &str = "SENT_START";
pub const SENTENCE_END: &str = "SENT_END";
pub const PARAGRAPH_END: &str = "PARA_END";

/// An interned part-of-speech tag. Tags form a small closed set, so they are
/// compared by handle, and [`Tag::id`] is a dense index usable in per-tag
/// caches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Tag(u32);

struct TagTable {
    by_name: std::collections::HashMap<&'static str, u32>,
    names: Vec<&'static str>,
}

fn tag_table() -> &'static std::sync::RwLock<TagTable> {
    static TABLE: std::sync::OnceLock<std::sync::RwLock<TagTable>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        std::sync::RwLock::new(TagTable { by_name: std::collections::HashMap::new(), names: Vec::new() })
    })
}

thread_local! {
    /// Per-thread view of the tag table: tagging creates a `Tag` for every
    /// reading, which must not take a shared lock each time.
    static LOCAL_TAGS: std::cell::RefCell<std::collections::HashMap<&'static str, u32>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

impl Tag {
    pub fn new(s: &str) -> Tag {
        if let Some(id) = LOCAL_TAGS.with(|l| l.borrow().get(s).copied()) {
            return Tag(id);
        }
        let known = {
            let t = tag_table().read().unwrap_or_else(|e| e.into_inner());
            t.by_name.get_key_value(s).map(|(k, v)| (*k, *v))
        };
        let (name, id) = match known {
            Some(x) => x,
            None => {
                let mut t = tag_table().write().unwrap_or_else(|e| e.into_inner());
                match t.by_name.get_key_value(s).map(|(k, v)| (*k, *v)) {
                    Some(x) => x,
                    None => {
                        let name: &'static str = Box::leak(s.to_owned().into_boxed_str());
                        let id = t.names.len() as u32;
                        t.names.push(name);
                        t.by_name.insert(name, id);
                        (name, id)
                    }
                }
            }
        };
        LOCAL_TAGS.with(|l| l.borrow_mut().insert(name, id));
        Tag(id)
    }

    pub fn as_str(self) -> &'static str {
        let t = tag_table().read().unwrap_or_else(|e| e.into_inner());
        t.names[self.0 as usize]
    }

    /// Dense index (0, 1, 2, ...) in creation order.
    #[inline]
    pub fn id(self) -> u32 {
        self.0
    }
}

/// One reading of a token (`AnalyzedToken`).
#[derive(Clone, Debug)]
pub struct Reading {
    pub token: Arc<str>,
    pub pos_tag: Option<Tag>,
    /// The tag's text (kept next to the handle: it is read constantly).
    pos_str: Option<&'static str>,
    pub lemma: Option<Arc<str>>,
    pub whitespace_before: bool,
    /// `hasNoTag()`: no tag, or only the structural `SENT_END`/`PARA_END`.
    pub has_no_tag: bool,
}

impl PartialEq for Reading {
    /// `AnalyzedToken.equals`: token, tag, lemma and whitespace flag.
    fn eq(&self, o: &Reading) -> bool {
        self.token == o.token
            && self.pos_tag == o.pos_tag
            && self.lemma == o.lemma
            && self.whitespace_before == o.whitespace_before
    }
}

impl Reading {
    pub fn new(token: Arc<str>, pos_tag: Option<Tag>, lemma: Option<Arc<str>>) -> Reading {
        let pos_str = pos_tag.map(Tag::as_str);
        let has_no_tag = matches!(pos_str, None | Some(SENTENCE_END) | Some(PARAGRAPH_END));
        Reading { token, pos_tag, pos_str, lemma, whitespace_before: false, has_no_tag }
    }

    pub fn from_strs(token: &str, pos_tag: Option<&str>, lemma: Option<&str>) -> Reading {
        Reading::new(Arc::from(token), pos_tag.map(|t| Tag::new(t.trim())), lemma.map(Arc::from))
    }

    #[inline]
    pub fn pos(&self) -> Option<&'static str> {
        self.pos_str
    }

    /// `lemmaOrToken`: what an inflected pattern token is compared with.
    #[inline]
    pub fn lemma_or_token(&self) -> &str {
        match &self.lemma {
            Some(l) => l,
            None => &self.token,
        }
    }

    /// `AnalyzedToken.matches`: does this reading satisfy the (partial)
    /// description `an`? Empty token, absent lemma or tag in `an` are
    /// wildcards; an all-wildcard description matches nothing.
    pub fn matches(&self, an: &Reading) -> bool {
        if self == an {
            return true;
        }
        if an.token.is_empty() && an.lemma.is_none() && an.pos_tag.is_none() {
            return false;
        }
        let mut found = true;
        if !an.token.is_empty() {
            found = an.token == self.token;
        }
        if an.lemma.is_some() {
            found &= an.lemma == self.lemma;
        }
        if let Some(t) = an.pos_tag {
            found &= Some(t) == self.pos_tag;
        }
        found
    }
}

/// All readings of one token (`AnalyzedTokenReadings`).
#[derive(Clone, Debug)]
pub struct Readings {
    pub readings: Vec<Reading>,
    pub token: Arc<str>,
    /// Byte offset of the token in the sentence text.
    pub start_pos: usize,
    pub is_whitespace: bool,
    pub is_linebreak: bool,
    pub is_sent_start: bool,
    pub is_sent_end: bool,
    pub is_para_end: bool,
    pub whitespace_before: bool,
    /// The whitespace token before this one, when there is one.
    pub whitespace_before_char: Arc<str>,
    pub is_pos_tag_unknown: bool,
    pub chunk_tags: Vec<Arc<str>>,
    pub immunized: bool,
    pub ignored_by_speller: bool,
    pub typographic_apostrophe: bool,
    pub has_same_lemmas: bool,
}

fn empty_str() -> Arc<str> {
    static EMPTY: std::sync::OnceLock<Arc<str>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(|| Arc::from("")).clone()
}

impl Readings {
    /// `new AnalyzedTokenReadings(List<AnalyzedToken>, startPos)`.
    pub fn new(readings: Vec<Reading>, start_pos: usize) -> Readings {
        assert!(!readings.is_empty(), "a token needs at least one reading");
        let token = readings[0].token.clone();
        let is_whitespace = super::strtools::is_whitespace(&token);
        let is_linebreak = matches!(&*token, "\n" | "\r\n" | "\r" | "\n\r");
        let mut r = Readings {
            whitespace_before: readings[0].whitespace_before,
            is_sent_start: readings[0].pos() == Some(SENTENCE_START),
            is_pos_tag_unknown: readings.len() == 1 && readings[0].pos_tag.is_none(),
            readings,
            token,
            start_pos,
            is_whitespace,
            is_linebreak,
            is_sent_end: false,
            is_para_end: false,
            whitespace_before_char: empty_str(),
            chunk_tags: Vec::new(),
            immunized: false,
            ignored_by_speller: false,
            typographic_apostrophe: false,
            has_same_lemmas: false,
        };
        r.is_para_end = r.has_pos_tag(PARAGRAPH_END);
        r.is_sent_end = r.has_pos_tag(SENTENCE_END);
        r.set_no_real_pos_tag();
        r.has_same_lemmas = r.are_lemmas_same();
        r
    }

    /// A token without any analysis (`createNullToken`).
    pub fn untagged(token: &str, start_pos: usize) -> Readings {
        Readings::new(vec![Reading::from_strs(token, None, None)], start_pos)
    }

    /// `new AnalyzedTokenReadings(old, newReadings, ruleApplied)`: new
    /// readings, same position and flags.
    pub fn with_readings(old: &Readings, readings: Vec<Reading>) -> Readings {
        let mut r = Readings::new(readings, old.start_pos);
        if old.is_sent_end {
            r.set_sent_end();
        }
        if old.is_para_end {
            r.set_paragraph_end();
        }
        r.set_whitespace_before(&old.whitespace_before_char.clone());
        r.chunk_tags = old.chunk_tags.clone();
        r.immunized = old.immunized;
        r.ignored_by_speller = old.ignored_by_speller;
        r.typographic_apostrophe = old.typographic_apostrophe;
        r
    }

    #[inline]
    pub fn end_pos(&self) -> usize {
        self.start_pos + self.token.len()
    }

    pub fn has_pos_tag(&self, tag: &str) -> bool {
        self.readings.iter().any(|r| r.pos() == Some(tag))
    }

    pub fn has_pos_tag_and_lemma(&self, tag: &str, lemma: &str) -> bool {
        self.readings.iter().any(|r| r.pos() == Some(tag) && r.lemma.as_deref() == Some(lemma))
    }

    pub fn has_lemma(&self, lemma: &str) -> bool {
        self.readings.iter().any(|r| r.lemma.as_deref() == Some(lemma))
    }

    pub fn has_any_lemma(&self, lemmas: &[&str]) -> bool {
        lemmas.iter().any(|l| self.has_lemma(l))
    }

    pub fn has_partial_pos_tag(&self, part: &str) -> bool {
        self.readings.iter().any(|r| r.pos().is_some_and(|p| p.contains(part)))
    }

    pub fn has_pos_tag_starting_with(&self, prefix: &str) -> bool {
        self.readings.iter().any(|r| r.pos().is_some_and(|p| p.starts_with(prefix)))
    }

    /// `matchesPosTagRegex`: some reading's tag fully matches `re`.
    pub fn matches_pos_tag_regex(&self, re: &crate::re::Regex) -> bool {
        self.readings.iter().any(|r| r.pos().is_some_and(|p| re.is_full_match(p)))
    }

    pub fn reading_with_tag_regex(&self, re: &crate::re::Regex) -> Option<&Reading> {
        self.readings.iter().find(|r| r.pos().is_some_and(|p| re.is_full_match(p)))
    }

    pub fn reading_with_lemma(&self, lemma: &str) -> Option<&Reading> {
        self.readings.iter().find(|r| r.lemma.as_deref() == Some(lemma))
    }

    pub fn matches_chunk_regex(&self, re: &crate::re::Regex) -> bool {
        self.chunk_tags.iter().any(|c| re.is_full_match(c))
    }

    /// `addReading`: a trailing untagged reading is replaced, not kept.
    pub fn add_reading(&mut self, mut token: Reading) {
        if self.readings.last().is_some_and(|r| r.pos_tag.is_none()) {
            self.readings.pop();
        }
        token.whitespace_before = self.whitespace_before;
        if super::strtools::utf16_len(&token.token) > super::strtools::utf16_len(&self.token) {
            self.token = token.token.clone();
        }
        self.readings.push(token);
        self.is_para_end = self.has_pos_tag(PARAGRAPH_END);
        self.is_sent_end = self.has_pos_tag(SENTENCE_END);
        self.set_no_real_pos_tag();
        self.has_same_lemmas = self.are_lemmas_same();
    }

    /// `removeReading`: drop the readings `token` describes; the structural
    /// end-of-sentence readings are put back when they were removed.
    pub fn remove_reading(&mut self, token: &Reading) {
        let mut tmp = Reading::new(token.token.clone(), token.pos_tag, token.lemma.clone());
        tmp.whitespace_before = self.whitespace_before;
        let mut removed_sent_end = false;
        let mut removed_para_end = false;
        let mut kept = Vec::with_capacity(self.readings.len());
        for r in self.readings.drain(..) {
            if !r.matches(&tmp) {
                kept.push(r);
            } else if r.pos() == Some(SENTENCE_END) {
                removed_sent_end = true;
            } else if r.pos() == Some(PARAGRAPH_END) {
                removed_para_end = true;
            }
        }
        if kept.is_empty() {
            let mut r = Reading::new(self.token.clone(), None, None);
            r.whitespace_before = self.whitespace_before;
            kept.push(r);
        }
        self.readings = kept;
        self.set_no_real_pos_tag();
        if removed_sent_end {
            self.is_sent_end = false;
            self.set_sent_end();
        }
        if removed_para_end {
            self.is_para_end = false;
            self.set_paragraph_end();
        }
        self.has_same_lemmas = self.are_lemmas_same();
    }

    /// `leaveReading`: keep only the readings `token` describes.
    pub fn leave_reading(&mut self, token: &Reading) {
        let mut tmp = Reading::new(token.token.clone(), token.pos_tag, token.lemma.clone());
        tmp.whitespace_before = self.whitespace_before;
        let mut kept: Vec<Reading> = self.readings.drain(..).filter(|r| r.matches(&tmp)).collect();
        if kept.is_empty() {
            let mut r = Reading::new(self.token.clone(), None, None);
            r.whitespace_before = self.whitespace_before;
            kept.push(r);
        }
        self.readings = kept;
        self.set_no_real_pos_tag();
        self.has_same_lemmas = self.are_lemmas_same();
    }

    pub fn set_paragraph_end(&mut self) {
        if !self.is_para_end {
            let lemma = self.readings[0].lemma.clone();
            self.add_reading(Reading::new(self.token.clone(), Some(Tag::new(PARAGRAPH_END)), lemma));
        }
    }

    pub fn set_sent_end(&mut self) {
        if !self.is_sent_end {
            let lemma = self.readings[0].lemma.clone();
            self.add_reading(Reading::new(self.token.clone(), Some(Tag::new(SENTENCE_END)), lemma));
        }
    }

    /// `setWhitespaceBefore(prevToken)`.
    pub fn set_whitespace_before(&mut self, prev_token: &Arc<str>) {
        self.whitespace_before = !prev_token.is_empty() && super::strtools::is_whitespace(prev_token);
        for r in &mut self.readings {
            r.whitespace_before = self.whitespace_before;
        }
        if self.whitespace_before {
            self.whitespace_before_char = prev_token.clone();
        }
    }

    fn set_no_real_pos_tag(&mut self) {
        let mut has_no_pos_tag = !self.is_linebreak;
        for r in &self.readings {
            match r.pos() {
                Some(PARAGRAPH_END) | Some(SENTENCE_END) => continue,
                Some(_) => {
                    has_no_pos_tag = false;
                    break;
                }
                None => {}
            }
        }
        for r in &mut self.readings {
            r.has_no_tag = has_no_pos_tag;
        }
    }

    /// `isTagged`: some reading carries a real tag.
    pub fn is_tagged(&self) -> bool {
        self.readings.iter().any(|r| !r.has_no_tag)
    }

    fn are_lemmas_same(&self) -> bool {
        let first = &self.readings[0].lemma;
        self.readings.iter().all(|r| &r.lemma == first)
    }

    /// `isNonWord`: a single punctuation or operator character.
    pub fn is_non_word(&self) -> bool {
        let mut it = self.token.chars();
        match (it.next(), it.next()) {
            (Some(c), None) => ".?!…:;,~’'\"„“”»«‚‘›‹()[]-–—*×∗·+÷/=".contains(c),
            _ => false,
        }
    }

    /// `isFieldCode`: an OpenOffice field marker.
    pub fn is_field_code(&self) -> bool {
        matches!(&*self.token, "\u{1}" | "\u{2}")
    }
}

/// A tagged and disambiguated sentence (`AnalyzedSentence`).
#[derive(Clone, Debug)]
pub struct Sentence {
    /// The sentence text, exactly as cut out of the document.
    pub text: Arc<str>,
    /// All tokens, whitespace included; index 0 is `SENT_START`.
    pub tokens: Vec<Readings>,
    /// Indices into `tokens` of the non-whitespace tokens
    /// (`getTokensWithoutWhitespace`); index 0 is `SENT_START`.
    pub non_ws: Vec<usize>,
    /// Tokens as the tagger produced them, before disambiguation
    /// (`<pattern raw_pos="yes">` reads these).
    pub pre_disambig: Vec<Readings>,
    pub pre_non_ws: Vec<usize>,
    /// Lower-cased surface form -> indices among the non-whitespace tokens
    /// (`getTokenOffsets`), to rule out rules quickly.
    pub token_offsets: std::collections::HashMap<String, Vec<usize>>,
    /// Lower-cased lemma (or surface form when there is none) -> indices
    /// (`getLemmaOffsets`).
    pub lemma_offsets: std::collections::HashMap<String, Vec<usize>>,
}

/// LanguageTool's "non blank" tokens: everything but whitespace, except that
/// a whitespace token marking the sentence start or end is kept.
fn non_blank(tokens: &[Readings]) -> Vec<usize> {
    tokens
        .iter()
        .enumerate()
        .filter(|(_, t)| !t.is_whitespace || t.is_sent_start || t.is_sent_end || t.is_para_end)
        .map(|(i, _)| i)
        .collect()
}

impl Sentence {
    pub fn new(text: Arc<str>, tokens: Vec<Readings>, pre_disambig: Vec<Readings>) -> Sentence {
        let non_ws = non_blank(&tokens);
        let pre_non_ws = non_blank(&pre_disambig);
        let mut token_offsets: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
        let mut lemma_offsets: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
        for (i, &ti) in non_ws.iter().enumerate() {
            let t = &tokens[ti];
            token_offsets.entry(t.token.to_lowercase()).or_default().push(i);
            for r in &t.readings {
                let list = lemma_offsets.entry(r.lemma_or_token().to_lowercase()).or_default();
                if list.last() != Some(&i) {
                    list.push(i);
                }
            }
        }
        Sentence { text, tokens, non_ws, pre_disambig, pre_non_ws, token_offsets, lemma_offsets }
    }

    /// `AnalyzedSentence.copy`: fresh token objects (so that immunization of
    /// the copy leaves the original alone).
    pub fn copy(&self) -> Sentence {
        self.clone()
    }

    /// The non-whitespace tokens, in order.
    pub fn non_ws_tokens(&self) -> Vec<&Readings> {
        self.non_ws.iter().map(|&i| &self.tokens[i]).collect()
    }

    /// The non-whitespace tokens as the tagger produced them.
    pub fn pre_disambig_non_ws_tokens(&self) -> Vec<&Readings> {
        self.pre_non_ws.iter().map(|&i| &self.pre_disambig[i]).collect()
    }

    /// The i-th non-whitespace token.
    #[inline]
    pub fn tok(&self, i: usize) -> &Readings {
        &self.tokens[self.non_ws[i]]
    }

    /// Number of non-whitespace tokens (`SENT_START` included).
    #[inline]
    pub fn len(&self) -> usize {
        self.non_ws.len()
    }

    pub fn is_empty(&self) -> bool {
        self.non_ws.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rd(token: &str, tag: Option<&str>, lemma: Option<&str>) -> Reading {
        Reading::from_strs(token, tag, lemma)
    }

    #[test]
    fn sentence_end_is_a_reading() {
        let mut t = Readings::new(vec![rd(".", Some("M fin"), Some("."))], 3);
        t.set_sent_end();
        t.set_paragraph_end();
        assert!(t.is_sent_end && t.is_para_end);
        assert_eq!(t.readings.len(), 3);
        assert_eq!(t.readings[1].pos(), Some(SENTENCE_END));
        assert_eq!(t.readings[1].lemma.as_deref(), Some("."));
        // The structural readings do not make the token "tagged" on their own.
        let mut u = Readings::untagged("xyz", 0);
        u.set_sent_end();
        assert_eq!(u.readings.len(), 1, "the untagged reading is replaced");
        assert!(!u.is_tagged());
        assert!(u.readings[0].has_no_tag);
    }

    #[test]
    fn remove_and_leave_readings() {
        let mut t = Readings::new(
            vec![rd("a", Some("N m sp"), Some("a")), rd("a", Some("V avoir ind pres 3 s"), Some("avoir"))],
            0,
        );
        assert!(!t.has_same_lemmas);
        t.remove_reading(&rd("a", Some("N m sp"), None));
        assert_eq!(t.readings.len(), 1);
        assert_eq!(t.readings[0].lemma.as_deref(), Some("avoir"));
        assert!(t.has_same_lemmas);
        t.remove_reading(&rd("a", Some("V avoir ind pres 3 s"), None));
        assert_eq!(t.readings.len(), 1);
        assert!(t.readings[0].pos_tag.is_none(), "an emptied token keeps one untagged reading");

        let mut t = Readings::new(vec![rd("la", Some("D f s"), Some("le")), rd("la", Some("N m s"), Some("la"))], 0);
        t.leave_reading(&rd("la", Some("D f s"), None));
        assert_eq!(t.readings.len(), 1);
        assert_eq!(t.readings[0].pos(), Some("D f s"));
    }

    #[test]
    fn removing_the_sentence_end_reading_restores_it() {
        let mut t = Readings::new(vec![rd("fin", Some("N f s"), Some("fin"))], 0);
        t.set_sent_end();
        // A wildcard on the token text alone removes every reading...
        t.remove_reading(&rd("fin", None, None));
        // ...but the sentence end comes back.
        assert!(t.is_sent_end);
        assert!(t.has_pos_tag(SENTENCE_END));
    }

    #[test]
    fn whitespace_tokens_are_skipped_but_kept() {
        let mut start = Readings::new(vec![rd("", Some(SENTENCE_START), None)], 0);
        start.is_whitespace = true;
        let a = Readings::untagged("Il", 0);
        let sp = Readings::untagged(" ", 2);
        let mut b = Readings::untagged("dort", 3);
        b.set_whitespace_before(&sp.token.clone());
        assert!(b.whitespace_before);
        let s = Sentence::new(Arc::from("Il dort"), vec![start, a, sp, b], Vec::new());
        assert_eq!(s.len(), 3);
        assert_eq!(&*s.tok(2).token, "dort");
        assert_eq!(s.token_offsets.get("il"), Some(&vec![1]));
    }
}
