//! Pattern elements: what one `<token>` of a rule accepts.
//!
//! Port of LanguageTool's `org.languagetool.rules.patterns` classes
//! `StringMatcher`, `PatternToken` (with its exceptions, AND/OR groups and
//! back-references), `Match`/`MatchState` (the `<match>` element: token
//! references, inflection through the synthesizer, case conversion) and
//! `CaseConversionHelper` (LanguageTool, LGPL-2.1-or-later).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use super::strtools;
use super::token::{Reading, Readings, Tag, PARAGRAPH_END, SENTENCE_END, SENTENCE_START};
use crate::re::{self, Flags, Regex};

/// `StringMatcher.MAX_MATCH_LENGTH`: longer tokens never match (in UTF-16 units).
pub const MAX_MATCH_LENGTH: usize = 250;

pub const UNKNOWN_TAG: &str = "UNKNOWN";

/// How a token's text (or a tag) is tested: equality or a whole-string regex.
#[derive(Clone, Debug)]
pub struct StringMatcher {
    pub pattern: Arc<str>,
    pub case_sensitive: bool,
    pub is_regexp: bool,
    kind: Kind,
}

#[derive(Clone, Debug)]
enum Kind {
    Equals,
    Regex(Arc<Regex>),
    /// The expression does not compile: it matches nothing.
    Never,
}

impl StringMatcher {
    /// `StringMatcher.create`.
    pub fn create(pattern: &str, is_regexp: bool, case_sensitive: bool) -> StringMatcher {
        if !is_regexp || pattern == "\\0" {
            return StringMatcher { pattern: Arc::from(pattern), case_sensitive, is_regexp, kind: Kind::Equals };
        }
        let flags = if case_sensitive { Flags::default() } else { Flags::IGNORE_CASE };
        match re::compile(pattern, flags) {
            Ok(r) => {
                // A regex with a single possible value is a plain comparison.
                if let Some(values) = r.possible_values() {
                    if values.len() == 1 {
                        return StringMatcher {
                            pattern: Arc::from(values[0].as_str()),
                            case_sensitive,
                            is_regexp: true,
                            kind: Kind::Equals,
                        };
                    }
                }
                StringMatcher { pattern: Arc::from(pattern), case_sensitive, is_regexp: true, kind: Kind::Regex(r) }
            }
            Err(_) => StringMatcher { pattern: Arc::from(pattern), case_sensitive, is_regexp: true, kind: Kind::Never },
        }
    }

    /// `StringMatcher.regexp`: a case-sensitive regex (used for tags).
    pub fn regexp(pattern: &str) -> StringMatcher {
        StringMatcher::create(pattern, true, true)
    }

    pub fn is_valid(&self) -> bool {
        !matches!(self.kind, Kind::Never)
    }

    pub fn matches(&self, s: &str) -> bool {
        if s.len() > MAX_MATCH_LENGTH && strtools::utf16_len(s) > MAX_MATCH_LENGTH {
            return false;
        }
        match &self.kind {
            Kind::Equals => {
                if self.case_sensitive {
                    s == &*self.pattern
                } else {
                    strtools::equals_ignore_case(s, &self.pattern)
                }
            }
            Kind::Regex(r) => r.is_full_match(s),
            Kind::Never => false,
        }
    }

    /// `getPossibleValues`: the finite set of strings that can match, if known.
    pub fn possible_values(&self) -> Option<Vec<String>> {
        match &self.kind {
            Kind::Equals => Some(vec![self.pattern.to_string()]),
            Kind::Regex(r) => r.possible_values(),
            Kind::Never => None,
        }
    }
}

/// Number of tags whose regex verdict is cached per `PosToken`.
const TAG_CACHE: usize = 1024;

/// The part-of-speech condition of a pattern token (`PatternToken.PosToken`).
#[derive(Debug)]
pub struct PosToken {
    pub pos_tag: Option<Arc<str>>,
    pub negation: bool,
    pub pos_pattern: Option<StringMatcher>,
    pub pos_unknown: bool,
    exact: Option<Tag>,
    /// Verdict of `pos_pattern` per tag id: 0 unknown, 1 no, 2 yes.
    cache: Option<Box<[AtomicU8]>>,
}

impl PosToken {
    pub fn new(pos_tag: Option<&str>, regexp: bool, negation: bool) -> PosToken {
        let pos_pattern = match pos_tag {
            Some(t) if regexp => Some(StringMatcher::regexp(t)),
            _ => None,
        };
        let pos_unknown = match (&pos_pattern, pos_tag) {
            (Some(p), _) => p.matches(UNKNOWN_TAG),
            (None, Some(t)) => t == UNKNOWN_TAG,
            (None, None) => false,
        };
        let cache = pos_pattern
            .as_ref()
            .map(|_| (0..TAG_CACHE).map(|_| AtomicU8::new(0)).collect::<Vec<_>>().into_boxed_slice());
        PosToken {
            exact: if pos_pattern.is_none() { pos_tag.map(Tag::new) } else { None },
            pos_tag: pos_tag.map(Arc::from),
            negation,
            pos_pattern,
            pos_unknown,
            cache,
        }
    }

    /// Does `tag` satisfy the condition (negation not applied)?
    #[inline]
    pub fn tag_matches(&self, tag: Tag, text: &str) -> bool {
        match &self.pos_pattern {
            None => self.exact == Some(tag),
            Some(p) => {
                let id = tag.id() as usize;
                if let Some(slot) = self.cache.as_ref().and_then(|c| c.get(id)) {
                    match slot.load(Ordering::Relaxed) {
                        1 => return false,
                        2 => return true,
                        _ => {}
                    }
                    let hit = p.matches(text);
                    slot.store(if hit { 2 } else { 1 }, Ordering::Relaxed);
                    hit
                } else {
                    p.matches(text)
                }
            }
        }
    }
}

/// `chunk` / `chunk_re` condition of a pattern token.
#[derive(Clone, Debug)]
pub struct ChunkTagPattern {
    pub tag: Arc<str>,
    pub regexp: Option<Arc<Regex>>,
}

impl ChunkTagPattern {
    pub fn new(tag: &str, is_regexp: bool) -> ChunkTagPattern {
        ChunkTagPattern {
            tag: Arc::from(tag),
            regexp: if is_regexp { re::compile(tag, Flags::default()).ok() } else { None },
        }
    }

    pub fn matches(&self, token: &Readings) -> bool {
        match &self.regexp {
            Some(r) => token.chunk_tags.iter().any(|c| r.is_full_match(c)),
            None => token.chunk_tags.iter().any(|c| **c == *self.tag),
        }
    }
}

/// Features to unify on a token: feature name -> equivalence types (empty:
/// all the types of that feature).
pub type UniFeatures = BTreeMap<String, Vec<String>>;

/// One `<token>` (or `<exception>`) of a rule: `PatternToken`.
#[derive(Clone, Debug)]
pub struct PatternToken {
    pub text: StringMatcher,
    /// `TEST_STRING_MASK`: the text condition is not empty.
    pub test_string: bool,
    pub inflected: bool,
    pub negation: bool,
    pub test_whitespace: bool,
    pub whitespace_before: bool,
    pub inside_marker: bool,
    pub exception_valid_next: bool,
    pub may_be_omitted: bool,
    pub unification_neutral: bool,
    pub uni_negation: bool,
    pub last_in_unification: bool,
    pub pos: Option<Arc<PosToken>>,
    pub skip: i32,
    pub max_occurrence: i32,
    pub current_and_next_exceptions: Vec<PatternToken>,
    pub previous_exceptions: Vec<PatternToken>,
    pub unification_features: Option<Arc<UniFeatures>>,
    pub phrase_name: Option<Arc<str>>,
    pub and_group: Vec<PatternToken>,
    pub or_group: Vec<PatternToken>,
    pub token_reference: Option<Match>,
    pub chunk_tag: Option<ChunkTagPattern>,
}

impl PatternToken {
    /// `new PatternToken(inflected, textMatcher)`.
    pub fn new(inflected: bool, text: StringMatcher) -> PatternToken {
        let test_string = !text.pattern.is_empty();
        PatternToken {
            text,
            test_string,
            inflected,
            negation: false,
            test_whitespace: false,
            whitespace_before: false,
            inside_marker: true,
            exception_valid_next: false,
            may_be_omitted: false,
            unification_neutral: false,
            uni_negation: false,
            last_in_unification: false,
            pos: None,
            skip: 0,
            max_occurrence: 1,
            current_and_next_exceptions: Vec::new(),
            previous_exceptions: Vec::new(),
            unification_features: None,
            phrase_name: None,
            and_group: Vec::new(),
            or_group: Vec::new(),
            token_reference: None,
            chunk_tag: None,
        }
    }

    /// `new PatternToken(token, caseSensitive, regExp, inflected)`.
    pub fn from_text(token: &str, case_sensitive: bool, regexp: bool, inflected: bool) -> PatternToken {
        PatternToken::new(inflected, StringMatcher::create(&normalize_text_pattern(token), regexp, case_sensitive))
    }

    pub fn set_text_matcher(&mut self, m: StringMatcher) {
        self.test_string = !m.pattern.is_empty();
        self.text = m;
    }

    /// `setStringElement`.
    pub fn set_string_element(&mut self, token: &str) {
        let m = StringMatcher::create(&normalize_text_pattern(token), self.text.is_regexp, self.text.case_sensitive);
        self.set_text_matcher(m);
    }

    pub fn string(&self) -> &str {
        &self.text.pattern
    }

    pub fn is_case_sensitive(&self) -> bool {
        self.text.case_sensitive
    }

    pub fn min_occurrence(&self) -> i32 {
        if self.may_be_omitted {
            0
        } else {
            1
        }
    }

    pub fn pos_tag(&self) -> Option<&str> {
        self.pos.as_ref().and_then(|p| p.pos_tag.as_deref())
    }

    pub fn pos_negation(&self) -> bool {
        self.pos.as_ref().is_some_and(|p| p.negation)
    }

    pub fn set_whitespace_before(&mut self, is_white: bool) {
        self.whitespace_before = is_white;
        self.test_whitespace = true;
    }

    /// `setExceptionSpaceBefore`: applies to the most recent exception.
    pub fn set_exception_space_before(&mut self, is_white: bool) {
        let list = if !self.previous_exceptions.is_empty() {
            &mut self.previous_exceptions
        } else {
            &mut self.current_and_next_exceptions
        };
        if let Some(last) = list.last_mut() {
            last.set_whitespace_before(is_white);
        }
    }

    pub fn add_exception(&mut self, scope_next: bool, scope_previous: bool, mut exception: PatternToken) {
        exception.exception_valid_next = scope_next;
        if scope_previous {
            self.previous_exceptions.push(exception);
        } else {
            self.current_and_next_exceptions.push(exception);
        }
    }

    pub fn has_previous_exception(&self) -> bool {
        !self.previous_exceptions.is_empty()
    }

    pub fn has_current_or_next_exceptions(&self) -> bool {
        !self.current_and_next_exceptions.is_empty()
    }

    pub fn has_exception_list(&self) -> bool {
        !self.current_and_next_exceptions.is_empty() || !self.previous_exceptions.is_empty()
    }

    pub fn has_and_group(&self) -> bool {
        !self.and_group.is_empty()
    }

    pub fn has_or_group(&self) -> bool {
        !self.or_group.is_empty()
    }

    pub fn is_reference_element(&self) -> bool {
        self.token_reference.is_some()
    }

    pub fn is_unified(&self) -> bool {
        self.unification_features.is_some()
    }

    /// `isSentenceStart`: the element is `postag="SENT_START"`.
    pub fn is_sentence_start(&self) -> bool {
        self.pos.as_ref().is_some_and(|p| p.pos_tag.as_deref() == Some(SENTENCE_START) && !p.negation)
    }

    fn is_pos_token_matched(&self, token: &Reading) -> bool {
        let Some(pos) = &self.pos else { return true };
        if pos.pos_tag.is_none() || (pos.pos_unknown && token.has_no_tag) {
            return true;
        }
        match (token.pos_tag, token.pos()) {
            (Some(tag), Some(text)) => pos.tag_matches(tag, text),
            _ => false,
        }
    }

    #[inline]
    fn test_token<'a>(&self, token: &'a Reading) -> &'a str {
        if self.inflected {
            token.lemma_or_token()
        } else {
            &token.token
        }
    }

    /// `isMatched(AnalyzedToken)`: one reading against the element's own
    /// conditions (no exceptions).
    pub fn is_matched(&self, token: &Reading) -> bool {
        if self.test_whitespace && self.whitespace_before != token.whitespace_before {
            return false;
        }
        let pos_negation = self.pos_negation();
        if self.test_string {
            (self.text.matches(self.test_token(token)) != self.negation)
                && (self.is_pos_token_matched(token) != pos_negation)
        } else {
            !self.negation && (self.is_pos_token_matched(token) != pos_negation)
        }
    }

    /// `isExceptionMatched`: a current-scope exception matches the reading.
    pub fn is_exception_matched(&self, token: &Reading) -> bool {
        self.current_and_next_exceptions.iter().any(|e| !e.exception_valid_next && e.is_matched(token))
    }

    pub fn is_and_exception_group_matched(&self, token: &Reading) -> bool {
        self.and_group.iter().any(|t| t.is_exception_matched(token))
    }

    pub fn is_exception_matched_completely(&self, token: &Reading) -> bool {
        self.is_exception_matched(token) || self.is_and_exception_group_matched(token)
    }

    pub fn is_matched_by_scope_next_exception(&self, token: &Reading) -> bool {
        self.current_and_next_exceptions.iter().any(|e| e.exception_valid_next && e.is_matched(token))
    }

    pub fn is_matched_by_previous_exception_reading(&self, token: &Reading) -> bool {
        self.previous_exceptions.iter().any(|e| !e.exception_valid_next && e.is_matched(token))
    }

    pub fn is_matched_by_previous_exception(&self, prev: &Readings) -> bool {
        self.has_previous_exception() && prev.readings.iter().any(|r| self.is_matched_by_previous_exception_reading(r))
    }

    /// `compile`: resolve a `<match no="N"/>` inside this token against the
    /// token it refers to, giving the concrete element to test.
    pub fn compile(&self, token: &Readings, lang: &Lang) -> PatternToken {
        let mut compiled = self.clone();
        let Some(reference) = &self.token_reference else { return compiled };
        let mut state = MatchState::new(reference, lang);
        state.set_token(token);
        let marker = format!("\\{}", reference.token_ref);
        if reference.set_pos {
            if let Some(pos_reference) = state.target_pos_tag() {
                compiled.pos = Some(Arc::new(PosToken::new(Some(&pos_reference), reference.postag_regexp, self.negation)));
            }
            let s = self.string().replace(&marker, "");
            compiled.set_string_element(&s);
        } else {
            let s = self.string().replace(&marker, &state.to_token_string());
            compiled.set_string_element(&s);
        }
        compiled
    }

    /// `hasStringThatMustMatch`.
    pub fn has_string_that_must_match(&self) -> bool {
        !self.is_reference_element() && !self.may_be_omitted && !self.string().is_empty()
    }

    fn own_possible_string_values(&self) -> Option<Vec<String>> {
        if self.negation || !self.has_string_that_must_match() {
            return None;
        }
        self.text.possible_values()
    }

    /// `calcStringHints`: surface forms (`inflected == false`) or lemmas one of
    /// which must be present in the sentence for this element to match.
    pub fn calc_string_hints(&self, inflected: bool) -> Option<BTreeSet<String>> {
        if inflected != self.inflected {
            return None;
        }
        let mut result: BTreeSet<String> = self.own_possible_string_values()?.into_iter().collect();
        if !self.and_group.is_empty() {
            for t in &self.and_group {
                if let Some(h) = t.calc_string_hints(inflected) {
                    result.retain(|x| h.contains(x));
                }
            }
        } else if !self.or_group.is_empty() {
            for t in &self.or_group {
                result.extend(t.calc_string_hints(inflected)?);
            }
        }
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }
}

/// `PatternToken.normalizeTextPattern` (`StringTools.trimWhitespace`).
pub fn normalize_text_pattern(token: &str) -> String {
    trim_whitespace(token)
}

/// `StringTools.trimWhitespace`: trim, drop tabs and line breaks, collapse
/// runs of ASCII whitespace.
pub fn trim_whitespace(s: &str) -> String {
    let chars: Vec<char> = s.trim_matches(|c: char| c <= ' ').chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < n {
        while chars[i] <= ' ' && i < n && (i + 1 < n && chars[i + 1] <= ' ' || i > 1 && chars[i - 1] <= ' ') {
            i += 1;
            if i >= n {
                break;
            }
        }
        if i >= n {
            break;
        }
        let c = chars[i];
        if c != '\n' && c != '\t' && c != '\r' {
            out.push(c);
        }
        i += 1;
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseConversion {
    None,
    StartLower,
    StartUpper,
    AllLower,
    AllUpper,
    Preserve,
    FirstUpper,
    NoTashkeel,
}

impl CaseConversion {
    pub fn parse(s: &str) -> CaseConversion {
        match s.to_ascii_uppercase().as_str() {
            "STARTLOWER" => CaseConversion::StartLower,
            "STARTUPPER" => CaseConversion::StartUpper,
            "ALLLOWER" => CaseConversion::AllLower,
            "ALLUPPER" => CaseConversion::AllUpper,
            "PRESERVE" => CaseConversion::Preserve,
            "FIRSTUPPER" => CaseConversion::FirstUpper,
            "NOTASHKEEL" => CaseConversion::NoTashkeel,
            _ => CaseConversion::None,
        }
    }
}

/// `CaseConversionHelper.convertCase`.
pub fn convert_case(conversion: CaseConversion, s: &str, sample: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    match conversion {
        CaseConversion::None | CaseConversion::NoTashkeel => s.to_string(),
        CaseConversion::Preserve => {
            if strtools::starts_with_uppercase(sample) {
                if strtools::is_all_uppercase(sample) {
                    s.to_uppercase()
                } else {
                    strtools::uppercase_first_char(s)
                }
            } else {
                s.to_string()
            }
        }
        CaseConversion::StartLower => {
            let mut it = s.chars();
            let first = it.next().unwrap();
            let mut out: String = first.to_lowercase().collect();
            out.push_str(it.as_str());
            out
        }
        CaseConversion::StartUpper => strtools::uppercase_first_char(s),
        CaseConversion::AllUpper => s.to_uppercase(),
        CaseConversion::FirstUpper => strtools::uppercase_first_char(&s.to_lowercase()),
        CaseConversion::AllLower => s.to_lowercase(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncludeRange {
    None,
    Following,
    All,
}

impl IncludeRange {
    pub fn parse(s: &str) -> IncludeRange {
        match s.to_ascii_uppercase().as_str() {
            "FOLLOWING" => IncludeRange::Following,
            "ALL" => IncludeRange::All,
            _ => IncludeRange::None,
        }
    }
}

/// A `<match>` element: a reference to a matched token, optionally rewritten
/// (regex replacement, re-inflection to another tag, case conversion).
#[derive(Clone, Debug)]
pub struct Match {
    pub pos_tag: Option<Arc<str>>,
    pub suppress_misspelled: bool,
    pub regex_replace: Option<Arc<str>>,
    pub pos_tag_replace: Option<Arc<str>>,
    pub case_conversion: CaseConversion,
    pub include_skipped: IncludeRange,
    pub regex_match: Option<Arc<Regex>>,
    pub set_pos: bool,
    pub postag_regexp: bool,
    pub static_lemma: bool,
    pub lemma: Option<Arc<str>>,
    pub token_ref: usize,
    pub pos_regex_match: Option<Arc<Regex>>,
    pub in_message_only: bool,
}

impl Match {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pos_tag: Option<&str>,
        pos_tag_replace: Option<&str>,
        postag_regexp: bool,
        regex_match: Option<&str>,
        regex_replace: Option<&str>,
        case_conversion: CaseConversion,
        set_pos: bool,
        suppress_misspelled: bool,
        include_skipped: IncludeRange,
    ) -> Match {
        Match {
            pos_tag: pos_tag.map(Arc::from),
            suppress_misspelled,
            regex_replace: regex_replace.map(Arc::from),
            pos_tag_replace: pos_tag_replace.map(Arc::from),
            case_conversion,
            include_skipped,
            regex_match: regex_match.and_then(|r| re::compile(r, Flags::default()).ok()),
            set_pos,
            postag_regexp,
            static_lemma: false,
            lemma: None,
            token_ref: 0,
            pos_regex_match: match pos_tag {
                Some(t) if postag_regexp => re::compile(t, Flags::default()).ok(),
                _ => None,
            },
            in_message_only: false,
        }
    }

    /// `setLemmaString`: `<match ...>lemma</match>` synthesizes forms of a
    /// fixed lemma instead of the matched token.
    pub fn set_lemma_string(&mut self, lemma: &str) {
        if !lemma.is_empty() {
            self.lemma = Some(Arc::from(lemma));
            self.static_lemma = true;
            self.postag_regexp = true;
            if let Some(t) = &self.pos_tag {
                self.pos_regex_match = re::compile(t, Flags::default()).ok();
            }
        }
    }

    pub fn converts_case(&self) -> bool {
        self.case_conversion != CaseConversion::None
    }
}

/// Inflection: from a lemma and a tag to word forms (`Synthesizer`).
pub trait Synthesizer: Send + Sync {
    /// Forms of the reading's lemma that carry exactly `pos_tag`.
    fn synthesize(&self, token: &Reading, pos_tag: &str) -> Vec<String>;

    /// Same, `pos_tag` being a regular expression when `pos_tag_regexp`.
    fn synthesize_regexp(&self, token: &Reading, pos_tag: &str, pos_tag_regexp: bool) -> Vec<String>;

    /// `getTargetPosTag`: the tag to synthesize, given the tags found on the
    /// token (`BaseSynthesizer`: the last one, or the requested tag).
    fn target_pos_tag(&self, pos_tags: &[String], target: Option<&str>) -> Option<String> {
        match pos_tags.last() {
            Some(t) => Some(t.clone()),
            None => target.map(str::to_string),
        }
    }

    fn pos_tag_correction(&self, pos_tag: &str) -> String {
        pos_tag.to_string()
    }
}

/// Tag one word outside of any sentence (`Tagger.tag(List.of(word))`).
pub trait WordTagger: Send + Sync {
    fn tag_word(&self, word: &str) -> Readings;
}

/// What rules need from the language: inflection and tagging.
#[derive(Clone, Default)]
pub struct Lang {
    pub synthesizer: Option<Arc<dyn Synthesizer>>,
    pub tagger: Option<Arc<dyn WordTagger>>,
}

/// `MatchState`: a `Match` bound to the token(s) it refers to.
pub struct MatchState<'a> {
    m: &'a Match,
    lang: &'a Lang,
    formatted_token: Option<Readings>,
    matched_token: Option<Readings>,
    skipped_tokens: Option<String>,
}

impl<'a> MatchState<'a> {
    pub fn new(m: &'a Match, lang: &'a Lang) -> MatchState<'a> {
        let formatted_token = match &m.lemma {
            Some(lemma) if !lemma.is_empty() => {
                Some(Readings::new(vec![Reading::from_strs(lemma, m.pos_tag.as_deref(), Some(lemma))], 0))
            }
            _ => None,
        };
        MatchState { m, lang, formatted_token, matched_token: None, skipped_tokens: None }
    }

    /// `setToken(AnalyzedTokenReadings)`.
    pub fn set_token(&mut self, token: &Readings) {
        if self.m.static_lemma {
            self.matched_token = Some(token.clone());
        } else {
            self.formatted_token = Some(token.clone());
        }
    }

    /// `setToken(tokens, index, next)`: also collects the tokens skipped
    /// between this element and the next one (`include_skipped`).
    pub fn set_token_at(&mut self, tokens: &[&Readings], index: usize, next: i64) {
        let idx = index.min(tokens.len() - 1);
        self.set_token(tokens[idx]);
        let include = self.m.include_skipped;
        if include == IncludeRange::Following {
            self.formatted_token = None;
        }
        if next > 1 && include != IncludeRange::None {
            let mut sb = String::new();
            let end = (index as i64 + next) as usize;
            for k in index + 1..end.min(tokens.len()) {
                if tokens[k].whitespace_before && !(k == index + 1 && include == IncludeRange::Following) {
                    sb.push(' ');
                }
                sb.push_str(&tokens[k].token);
            }
            self.skipped_tokens = Some(sb);
        } else {
            self.skipped_tokens = Some(String::new());
        }
    }

    fn convert(&self, s: &str, sample: &str) -> String {
        convert_case(self.m.case_conversion, s, sample)
    }

    fn new_tokens(&self, formatted: &Readings, token: &str) -> Vec<Reading> {
        let pos_tag = self.m.pos_tag.as_deref();
        let mut list = Vec::new();
        let mut lemma: Option<Arc<str>> = None;
        for r in &formatted.readings {
            if let Some(temp) = r.pos() {
                if Some(temp) == pos_tag && r.lemma.is_some() {
                    lemma = r.lemma.clone();
                }
                if lemma.as_deref().map(str::is_empty).unwrap_or(true) {
                    lemma = formatted.readings[0].lemma.clone();
                }
                let mut n = Reading::new(Arc::from(token), pos_tag.map(Tag::new), lemma.clone());
                n.whitespace_before = formatted.whitespace_before;
                list.push(n);
            }
        }
        list
    }

    /// `filterReadings`: the referenced token restricted (or re-tagged) to
    /// what the `<match>` asks for. Used by the disambiguator.
    pub fn filter_readings(&mut self) -> Option<Readings> {
        let mut l: Vec<Reading> = Vec::new();
        if self.formatted_token.is_some() {
            if self.m.static_lemma {
                if let Some(mut matched) = self.matched_token.take() {
                    let lemma_token = self.formatted_token.as_ref().unwrap().token.clone();
                    let wanted = Reading::new(matched.readings[0].token.clone(), self.m.pos_tag.as_deref().map(Tag::new), Some(lemma_token));
                    matched.leave_reading(&wanted);
                    self.matched_token = Some(matched.clone());
                    self.formatted_token = Some(matched);
                }
            }
            let formatted = self.formatted_token.clone().unwrap();
            let mut token = formatted.readings[0].token.to_string();
            if let (Some(rm), Some(rr)) = (&self.m.regex_match, &self.m.regex_replace) {
                token = rm.replace_all(&token, rr);
            }
            token = self.convert(&token, &token);
            if let Some(pos_tag) = self.m.pos_tag.as_deref() {
                if self.m.postag_regexp {
                    if let Some(pre) = &self.m.pos_regex_match {
                        for r in &formatted.readings {
                            if let Some(test_tag) = r.pos() {
                                if pre.is_full_match(test_tag) {
                                    let target = match &self.m.pos_tag_replace {
                                        Some(repl) => pre.replace_all(test_tag, repl),
                                        None => test_tag.to_string(),
                                    };
                                    let mut n = Reading::new(Arc::from(token.as_str()), Some(Tag::new(&target)), r.lemma.clone());
                                    n.whitespace_before = formatted.whitespace_before;
                                    l.push(n);
                                }
                            }
                        }
                    }
                    if l.is_empty() {
                        l.extend(self.new_tokens(&formatted, &token));
                    }
                } else {
                    l.extend(self.new_tokens(&formatted, &token));
                }
                let _ = pos_tag;
                let lemma = formatted.readings[0].lemma.clone();
                if formatted.is_sent_end {
                    l.push(Reading::new(formatted.token.clone(), Some(Tag::new(SENTENCE_END)), lemma.clone()));
                }
                if formatted.is_para_end {
                    l.push(Reading::new(formatted.token.clone(), Some(Tag::new(PARAGRAPH_END)), lemma));
                }
            }
        }
        if l.is_empty() {
            return self.formatted_token.clone();
        }
        let formatted = self.formatted_token.as_ref().unwrap();
        let mut out = Readings::new(l, formatted.start_pos);
        out.set_whitespace_before(&formatted.whitespace_before_char.clone());
        if !formatted.chunk_tags.is_empty() {
            out.chunk_tags = formatted.chunk_tags.clone();
        }
        if formatted.immunized {
            out.immunized = true;
        }
        Some(out)
    }

    /// `getTargetPosTag`: the tag(s) to inflect to.
    pub fn target_pos_tag(&self) -> Option<String> {
        let mut target = self.m.pos_tag.as_deref().map(str::to_string);
        let synth = self.lang.synthesizer.as_deref();
        let mut pos_tags: Vec<String> = Vec::new();
        let source = if self.m.static_lemma { self.matched_token.as_ref() } else { self.formatted_token.as_ref() };
        if let (Some(tok), Some(pre)) = (source, &self.m.pos_regex_match) {
            for r in &tok.readings {
                if let Some(t) = r.pos() {
                    if pre.is_full_match(t) {
                        pos_tags.push(t.to_string());
                    }
                }
            }
        }
        if let Some(s) = synth {
            target = s.target_pos_tag(&pos_tags, target.as_deref());
        } else if let Some(last) = pos_tags.last() {
            target = Some(last.clone());
        }
        if let (Some(pre), Some(repl)) = (&self.m.pos_regex_match, &self.m.pos_tag_replace) {
            if self.m.static_lemma {
                if !pos_tags.is_empty() {
                    target = target.map(|t| pre.replace_all(&t, repl));
                }
            } else {
                if pos_tags.is_empty() {
                    if let Some(t) = &target {
                        pos_tags.push(t.clone());
                    }
                }
                let parts: Vec<String> = pos_tags
                    .iter()
                    .map(|t| {
                        let mut t = pre.replace_all(t, repl);
                        if self.m.set_pos {
                            if let Some(s) = synth {
                                t = s.pos_tag_correction(&t);
                            }
                        }
                        t
                    })
                    .collect();
                target = Some(parts.join("|"));
            }
        }
        target
    }

    /// `toFinalString`: the replacement text(s) for this reference. With
    /// `check_spelling`, forms unknown to the tagger become the `<mistake/>`
    /// marker (suggestions containing it are dropped).
    pub fn to_final_string(&mut self, check_spelling: bool) -> Vec<String> {
        let mut formatted_string: Vec<String> = vec![String::new()];
        let synth = self.lang.synthesizer.clone();
        if let Some(formatted) = &self.formatted_token {
            formatted_string[0] = formatted.token.to_string();
            if let Some(rm) = &self.m.regex_match {
                let repl = self.m.regex_replace.as_deref().unwrap_or("");
                formatted_string[0] = rm.replace_all(&formatted_string[0], repl);
            }
            if let Some(pos_tag) = self.m.pos_tag.as_deref() {
                match &synth {
                    None => formatted_string[0] = formatted.token.to_string(),
                    Some(synth) if self.m.postag_regexp => {
                        let mut word_forms: BTreeSet<String> = BTreeSet::new();
                        let mut one_form = false;
                        for r in &formatted.readings {
                            if r.lemma.is_none() {
                                match r.pos() {
                                    None => {
                                        word_forms.insert(formatted.token.to_string());
                                        one_form = true;
                                    }
                                    Some(SENTENCE_START) | Some(SENTENCE_END) | Some(PARAGRAPH_END) => {
                                        if !one_form {
                                            word_forms.insert(formatted.token.to_string());
                                        }
                                        one_form = true;
                                    }
                                    Some(_) => one_form = false,
                                }
                            }
                        }
                        let target = self.target_pos_tag();
                        if !one_form {
                            if let Some(target) = &target {
                                for r in &formatted.readings {
                                    word_forms.extend(synth.synthesize_regexp(r, target, true));
                                }
                            }
                        }
                        if word_forms.is_empty() {
                            formatted_string[0] = format!("({})", formatted.token);
                        } else {
                            formatted_string = word_forms.into_iter().collect();
                        }
                    }
                    Some(synth) => {
                        let mut word_forms: BTreeSet<String> = BTreeSet::new();
                        for r in &formatted.readings {
                            word_forms.extend(synth.synthesize(r, pos_tag));
                        }
                        formatted_string = word_forms.into_iter().collect();
                    }
                }
            }
        }
        let original: String = if self.m.static_lemma {
            self.matched_token.as_ref().map(|t| t.token.to_string()).unwrap_or_default()
        } else {
            self.formatted_token.as_ref().map(|t| t.token.to_string()).unwrap_or_default()
        };
        for s in formatted_string.iter_mut() {
            *s = self.convert(s, &original);
        }
        if self.m.include_skipped != IncludeRange::None {
            if let Some(skipped) = self.skipped_tokens.as_deref().filter(|s| !s.is_empty()) {
                for s in formatted_string.iter_mut() {
                    s.push_str(skipped);
                }
            }
        }
        if self.m.suppress_misspelled && check_spelling {
            if let Some(tagger) = &self.lang.tagger {
                for s in formatted_string.iter_mut() {
                    let analyzed = tagger.tag_word(s);
                    let first = &analyzed.readings[0];
                    if first.lemma.is_none() && first.has_no_tag {
                        *s = MISTAKE.to_string();
                    }
                }
            }
        }
        formatted_string
    }

    /// `toTokenString`: the alternatives joined with `|` (a regex).
    pub fn to_token_string(&mut self) -> String {
        self.to_final_string(false).join("|")
    }
}

/// `PatternRuleMatcher.MISTAKE`: marks a suggestion to drop.
pub const MISTAKE: &str = "<mistake/>";

#[cfg(test)]
mod tests {
    use super::*;

    fn rd(token: &str, tag: Option<&str>, lemma: Option<&str>) -> Reading {
        Reading::from_strs(token, tag, lemma)
    }

    #[test]
    fn string_matcher_modes() {
        assert!(StringMatcher::create("Tout", false, false).matches("tout"));
        assert!(!StringMatcher::create("Tout", false, true).matches("tout"));
        let m = StringMatcher::create("tout|toute|toutes", true, false);
        assert!(m.matches("Toutes"));
        assert!(!m.matches("toutou"), "a token regex must match the whole token");
        assert_eq!(m.possible_values().map(|v| v.len()), Some(3));
        // A regex with one possible value is a plain comparison.
        let m = StringMatcher::create("(?:être)", true, false);
        assert!(m.matches("ÊTRE"));
        assert_eq!(&*m.pattern, "être");
        assert!(!StringMatcher::create("a", true, false).matches(&"a".repeat(300)));
    }

    #[test]
    fn inflected_tokens_compare_the_lemma() {
        // <token inflected="yes">avoir</token> matches every form of "avoir".
        let t = PatternToken::from_text("avoir", false, false, true);
        assert!(t.is_matched(&rd("a", Some("V avoir ind pres 3 s"), Some("avoir"))));
        assert!(!t.is_matched(&rd("a", Some("N m sp"), Some("a"))));
        // Without a lemma the surface form is compared.
        assert!(t.is_matched(&rd("avoir", None, None)));
    }

    #[test]
    fn pos_conditions() {
        let mut t = PatternToken::from_text("", false, false, false);
        t.pos = Some(Arc::new(PosToken::new(Some("V .* ppa.*"), true, false)));
        assert!(t.is_matched(&rd("mangé", Some("V etre ppa m s"), Some("manger"))));
        assert!(!t.is_matched(&rd("mangé", Some("V ppa m s"), Some("manger"))), "whole-tag match");
        assert!(!t.is_matched(&rd("xyz", None, None)));
        // negate_pos
        t.pos = Some(Arc::new(PosToken::new(Some("N.*"), true, true)));
        assert!(t.is_matched(&rd("mangé", Some("V ppa m s"), Some("manger"))));
        assert!(!t.is_matched(&rd("chat", Some("N m s"), Some("chat"))));
        // UNKNOWN matches a token without any tag.
        t.pos = Some(Arc::new(PosToken::new(Some("UNKNOWN"), false, false)));
        assert!(t.is_matched(&rd("xyz", None, None)));
        assert!(!t.is_matched(&rd("chat", Some("N m s"), Some("chat"))));
    }

    #[test]
    fn negation_and_whitespace() {
        let mut t = PatternToken::from_text("le", false, false, false);
        t.negation = true;
        assert!(t.is_matched(&rd("la", None, None)));
        assert!(!t.is_matched(&rd("le", None, None)));
        let mut t = PatternToken::from_text(",", false, false, false);
        t.set_whitespace_before(true);
        let mut r = rd(",", None, None);
        assert!(!t.is_matched(&r));
        r.whitespace_before = true;
        assert!(t.is_matched(&r));
    }

    #[test]
    fn exceptions_by_scope() {
        let mut t = PatternToken::from_text("a", false, false, false);
        t.add_exception(false, false, {
            let mut e = PatternToken::from_text("", false, false, false);
            e.pos = Some(Arc::new(PosToken::new(Some("N.*"), true, false)));
            e
        });
        t.add_exception(true, false, PatternToken::from_text("beau", false, false, false));
        t.add_exception(false, true, PatternToken::from_text("il", false, false, false));
        assert!(t.is_exception_matched(&rd("a", Some("N m sp"), Some("a"))));
        assert!(!t.is_exception_matched(&rd("a", Some("V avoir ind pres 3 s"), Some("avoir"))));
        assert!(t.is_matched_by_scope_next_exception(&rd("beau", None, None)));
        assert!(!t.is_exception_matched(&rd("beau", None, None)), "a next-scope exception is not a current one");
        let prev = Readings::new(vec![rd("Il", None, None)], 0);
        assert!(t.is_matched_by_previous_exception(&prev));
    }

    #[test]
    fn case_conversion() {
        assert_eq!(convert_case(CaseConversion::Preserve, "chat", "Chien"), "Chat");
        assert_eq!(convert_case(CaseConversion::Preserve, "chat", "CHIEN"), "CHAT");
        assert_eq!(convert_case(CaseConversion::Preserve, "chat", "chien"), "chat");
        assert_eq!(convert_case(CaseConversion::StartLower, "École", ""), "école");
        assert_eq!(convert_case(CaseConversion::StartUpper, "école", ""), "École");
        assert_eq!(convert_case(CaseConversion::FirstUpper, "éCOLE", ""), "École");
        assert_eq!(convert_case(CaseConversion::AllUpper, "école", ""), "ÉCOLE");
    }

    #[test]
    fn match_regex_replacement_without_synthesis() {
        let m = Match::new(None, None, false, Some("(.*)s$"), Some("$1"), CaseConversion::None, false, false, IncludeRange::None);
        let lang = Lang::default();
        let mut st = MatchState::new(&m, &lang);
        st.set_token(&Readings::new(vec![rd("chats", Some("N m p"), Some("chat"))], 0));
        assert_eq!(st.to_final_string(false), vec!["chat".to_string()]);
    }

    #[test]
    fn hints() {
        let t = PatternToken::from_text("tout|toute", false, true, false);
        assert_eq!(t.calc_string_hints(false).map(|s| s.len()), Some(2));
        assert_eq!(t.calc_string_hints(true), None);
        let t = PatternToken::from_text("être", false, false, true);
        assert_eq!(t.calc_string_hints(true).map(|s| s.into_iter().collect::<Vec<_>>()), Some(vec!["être".to_string()]));
        let mut t = PatternToken::from_text("le", false, false, false);
        t.may_be_omitted = true;
        assert_eq!(t.calc_string_hints(false), None, "an optional token gives no hint");
    }

    #[test]
    fn trim_whitespace_like_languagetool() {
        // As in LanguageTool: a single space is kept, a longer run of
        // whitespace disappears altogether.
        assert_eq!(trim_whitespace("  a  b \n c "), "abc");
        assert_eq!(trim_whitespace(" a b "), "a b");
        assert_eq!(trim_whitespace("abc"), "abc");
        assert_eq!(trim_whitespace(""), "");
    }
}
