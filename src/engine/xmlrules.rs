//! Loading rule files (`grammar.xml`, `style.xml`, `disambiguation.xml`).
//!
//! Port of `XMLRuleHandler`, `PatternRuleHandler` and
//! `DisambiguationRuleHandler` (LanguageTool, LGPL-2.1-or-later). The
//! originals are SAX handlers; this is the same state machine driven by the
//! crate's pull parser, so that every attribute and nesting rule is
//! interpreted as LanguageTool interprets it.

use std::collections::HashMap;
use std::sync::Arc;

use super::disambig::{DisambigAction, DisambigRule};
use super::pattern::{CaseConversion, ChunkTagPattern, IncludeRange, Match, PatternToken, PosToken, StringMatcher, UniFeatures};
use super::performer::TokenPattern;
use super::rule::{FilterRef, PatternRule, RegexRule, RuleFilter, RuleInfo, TokenRule, PLEASE_SPELL_ME};
use super::token::{Reading, Tag};
use super::unifier::UnifierConfiguration;
use crate::re::{self, Flags};
use crate::xml::{Event, Parser};

/// Resolves a Java filter class name to its native implementation.
pub type FilterFactory<'a> = &'a dyn Fn(&str) -> Option<Arc<dyn RuleFilter>>;

/// A rule of a grammar file, in file order.
pub enum GrammarRule {
    Pattern(PatternRule),
    Regex(RegexRule),
}

impl GrammarRule {
    pub fn info(&self) -> &RuleInfo {
        match self {
            GrammarRule::Pattern(r) => &r.info,
            GrammarRule::Regex(r) => &r.info,
        }
    }
}

type Attrs = Vec<(String, String)>;

fn attr<'a>(attrs: &'a Attrs, name: &str) -> Option<&'a str> {
    attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

fn yes(attrs: &Attrs, name: &str) -> bool {
    attr(attrs, name) == Some("yes")
}

/// State shared by both handlers (`XMLRuleHandler`).
#[derive(Default)]
struct Common {
    in_pattern: bool,
    in_message: bool,
    in_suggestion: bool,
    in_match: bool,
    in_rule_group: bool,
    in_token: bool,
    in_exception: bool,
    in_phrases: bool,
    in_and_group: bool,
    in_or_group: bool,
    token_space_before: bool,
    token_space_before_set: bool,
    pos_token: Option<String>,
    chunk_tag: Option<ChunkTagPattern>,
    pos_negation: bool,
    pos_regexp: bool,
    case_sensitive: bool,
    reg_expression: bool,
    token_negated: bool,
    token_inflected: bool,
    token_level_case_sensitive: bool,
    token_level_case_set: bool,
    exception_pos_token: Option<String>,
    exception_string_regexp: bool,
    exception_string_negation: bool,
    exception_string_inflected: bool,
    exception_pos_negation: bool,
    exception_pos_regexp: bool,
    exception_valid_next: bool,
    exception_valid_prev: bool,
    exception_set: bool,
    exception_space_before: bool,
    exception_space_before_set: bool,
    exception_level_case_sensitive: Option<bool>,
    pattern_tokens: Vec<PatternToken>,
    last_phrase: bool,
    phrase_id: Option<String>,
    skip_pos: i32,
    min_occurrence: i32,
    max_occurrence: i32,
    id: Option<String>,
    pattern_token: Option<PatternToken>,
    token_reference: Option<Match>,
    suggestion_matches: Vec<Match>,
    suggestion_matches_out_msg: Vec<Match>,
    token_counter: i32,
    phrase_map: HashMap<String, Vec<Vec<PatternToken>>>,
    phrase_pattern_tokens: Vec<Vec<PatternToken>>,
    and_group_counter: i32,
    or_group_counter: i32,
    in_unification: bool,
    in_marker: bool,
    in_unification_def: bool,
    uni_negation: bool,
    in_unification_neutral: bool,
    u_feature: Option<String>,
    u_type: String,
    u_type_list: Vec<String>,
    equivalence_features: UniFeatures,
    elements: String,
    exceptions: String,
    message: String,
    suggestions_out_msg: String,
    match_text: String,
    pos_tokens: HashMap<(String, bool, bool), Arc<PosToken>>,
    matchers: HashMap<(String, bool, bool), StringMatcher>,
}

fn has_posix_character_class(s: &str) -> bool {
    s.contains("\\p{Lu}") || s.contains("\\p{Ll}")
}

impl Common {
    fn new() -> Common {
        Common { min_occurrence: 1, max_occurrence: 1, ..Default::default() }
    }

    fn intern_matcher(&mut self, text: &str, regexp: bool, case_sensitive: bool) -> StringMatcher {
        let text = super::pattern::normalize_text_pattern(text);
        self.matchers
            .entry((text.clone(), regexp, case_sensitive))
            .or_insert_with(|| StringMatcher::create(&text, regexp, case_sensitive))
            .clone()
    }

    fn obtain_pos_token(&mut self, pos: &str, regexp: bool, negated: bool) -> Arc<PosToken> {
        self.pos_tokens
            .entry((pos.to_string(), regexp, negated))
            .or_insert_with(|| Arc::new(PosToken::new(Some(pos), regexp, negated)))
            .clone()
    }

    fn reset_exception(&mut self) {
        self.exception_string_negation = false;
        self.exception_string_inflected = false;
        self.exception_pos_negation = false;
        self.exception_pos_regexp = false;
        self.exception_string_regexp = false;
        self.exception_valid_next = false;
        self.exception_valid_prev = false;
        self.exception_space_before = false;
        self.exception_space_before_set = false;
    }

    fn reset_token(&mut self) {
        self.pos_negation = false;
        self.pos_regexp = false;
        self.in_token = false;
        self.token_space_before = false;
        self.token_space_before_set = false;
        self.reset_exception();
        self.exception_set = false;
        self.token_reference = None;
    }

    fn set_token(&mut self, attrs: &Attrs) {
        self.in_token = true;
        if self.last_phrase {
            self.pattern_tokens.clear();
        }
        self.last_phrase = false;
        self.token_negated = yes(attrs, "negate");
        self.token_inflected = yes(attrs, "inflected");
        if let Some(v) = attr(attrs, "skip") {
            self.skip_pos = v.trim().parse().unwrap_or(0);
        }
        if let Some(v) = attr(attrs, "min") {
            self.min_occurrence = v.trim().parse().unwrap_or(1);
        }
        if let Some(v) = attr(attrs, "max") {
            self.max_occurrence = v.trim().parse().unwrap_or(1);
        }
        self.elements = String::new();
        if let Some(p) = attr(attrs, "postag") {
            self.pos_token = Some(p.to_string());
            self.pos_regexp = yes(attrs, "postag_regexp");
            self.pos_negation = yes(attrs, "negate_pos");
        }
        if let Some(c) = attr(attrs, "chunk") {
            self.chunk_tag = Some(ChunkTagPattern::new(c, false));
        } else if let Some(c) = attr(attrs, "chunk_re") {
            self.chunk_tag = Some(ChunkTagPattern::new(c, true));
        }
        self.reg_expression = yes(attrs, "regexp");
        if let Some(v) = attr(attrs, "spacebefore") {
            self.token_space_before = v == "yes";
            self.token_space_before_set = v != "ignore";
        }
        if !self.in_and_group && !self.in_or_group {
            self.token_counter += 1;
        }
        match attr(attrs, "case_sensitive") {
            Some(v) => {
                self.token_level_case_set = true;
                self.token_level_case_sensitive = v == "yes";
            }
            None => {
                self.token_level_case_sensitive = false;
                self.token_level_case_set = false;
            }
        }
    }

    fn set_exceptions(&mut self, attrs: &Attrs) {
        self.in_exception = true;
        self.exceptions = String::new();
        self.reset_exception();
        self.exception_string_negation = yes(attrs, "negate");
        self.exception_valid_next = attr(attrs, "scope") == Some("next");
        self.exception_valid_prev = attr(attrs, "scope") == Some("previous");
        self.exception_string_inflected = yes(attrs, "inflected");
        if let Some(p) = attr(attrs, "postag") {
            self.exception_pos_token = Some(p.to_string());
            self.exception_pos_regexp = yes(attrs, "postag_regexp");
            self.exception_pos_negation = yes(attrs, "negate_pos");
        }
        self.exception_string_regexp = yes(attrs, "regexp");
        if let Some(v) = attr(attrs, "spacebefore") {
            self.exception_space_before = v == "yes";
            self.exception_space_before_set = v != "ignore";
        }
        self.exception_level_case_sensitive = attr(attrs, "case_sensitive").map(|v| v == "yes");
    }

    fn finalize_exceptions(&mut self) {
        self.in_exception = false;
        if !self.exception_set {
            let token_case = if self.token_level_case_set { self.token_level_case_sensitive } else { self.case_sensitive };
            let text = self.elements.trim().to_string();
            let m = self.intern_matcher(&text, self.reg_expression, token_case);
            self.pattern_token = Some(PatternToken::new(self.token_inflected, m));
            self.exception_set = true;
        }
        let token_negated = self.token_negated;
        if let Some(t) = self.pattern_token.as_mut() {
            t.negation = token_negated;
        }
        if !self.exceptions.is_empty() || self.exception_pos_token.is_some() {
            if has_posix_character_class(&self.exceptions) {
                self.exception_level_case_sensitive = Some(true);
            }
            let token_cs = self.pattern_token.as_ref().map(|t| t.is_case_sensitive()).unwrap_or(false);
            let case_sensitive = self.exception_level_case_sensitive.unwrap_or(token_cs);
            let text = self.exceptions.trim().to_string();
            let m = self.intern_matcher(&text, self.exception_string_regexp, case_sensitive);
            let mut exception = PatternToken::new(self.exception_string_inflected, m);
            exception.negation = self.exception_string_negation;
            // `obtainPosToken(null, ...)` still yields a PosToken, whose
            // negation flag then applies to "no tag condition".
            exception.pos = Some(match self.exception_pos_token.clone() {
                Some(p) => self.obtain_pos_token(&p, self.exception_pos_regexp, self.exception_pos_negation),
                None => Arc::new(PosToken::new(None, self.exception_pos_regexp, self.exception_pos_negation)),
            });
            let (next, prev) = (self.exception_valid_next, self.exception_valid_prev);
            if let Some(t) = self.pattern_token.as_mut() {
                t.add_exception(next, prev, exception);
            }
            self.exception_pos_token = None;
            self.exception_level_case_sensitive = None;
        }
        if self.exception_space_before_set {
            let sb = self.exception_space_before;
            if let Some(t) = self.pattern_token.as_mut() {
                t.set_exception_space_before(sb);
            }
        }
        self.reset_exception();
    }

    fn finalize_tokens(&mut self, unifier: &mut UnifierConfiguration) {
        let text = self.elements.trim().to_string();
        if !self.exception_set || self.pattern_token.is_none() {
            let mut token_case = self.case_sensitive || has_posix_character_class(&self.elements);
            if self.token_level_case_set {
                token_case = self.token_level_case_sensitive;
            }
            let m = self.intern_matcher(&text, self.reg_expression, token_case);
            let mut t = PatternToken::new(self.token_inflected, m);
            t.negation = self.token_negated;
            self.pattern_token = Some(t);
        } else {
            let (cs, re) = {
                let t = self.pattern_token.as_ref().unwrap();
                (t.is_case_sensitive() || has_posix_character_class(&self.elements), t.text.is_regexp)
            };
            let m = self.intern_matcher(&text, re, cs);
            self.pattern_token.as_mut().unwrap().set_text_matcher(m);
        }
        let mut token = self.pattern_token.take().unwrap();
        if self.skip_pos != 0 {
            token.skip = self.skip_pos;
            self.skip_pos = 0;
        }
        if self.min_occurrence == 0 {
            token.may_be_omitted = true;
        }
        if self.max_occurrence != 1 {
            token.max_occurrence = self.max_occurrence;
            self.max_occurrence = 1;
        }
        if let Some(p) = self.pos_token.take() {
            token.pos = Some(self.obtain_pos_token(&p, self.pos_regexp, self.pos_negation));
        }
        if let Some(c) = self.chunk_tag.take() {
            token.chunk_tag = Some(c);
        }
        if let Some(r) = self.token_reference.clone() {
            token.token_reference = Some(r);
        }
        // These are set after insertion in the original; the object is the
        // same, so the order does not matter.
        if self.in_unification {
            token.unification_features = Some(Arc::new(self.equivalence_features.clone()));
        }
        if self.in_unification_neutral {
            token.unification_neutral = true;
        }
        token.inside_marker = self.in_marker;
        if self.token_space_before_set {
            token.set_whitespace_before(self.token_space_before);
        }
        if self.in_unification_def {
            if let Some(f) = self.u_feature.clone() {
                unifier.set_equivalence(&f, &self.u_type.clone(), token);
            }
            self.pattern_tokens.clear();
            self.min_occurrence = 1;
        } else if self.in_and_group && self.and_group_counter > 0 {
            if let Some(last) = self.pattern_tokens.last_mut() {
                last.and_group.push(token);
            }
        } else if self.in_or_group && self.or_group_counter > 0 {
            if let Some(last) = self.pattern_tokens.last_mut() {
                last.or_group.push(token);
            }
        } else {
            if self.min_occurrence < 1 {
                self.pattern_tokens.push(token.clone());
            }
            for _ in 1..=self.min_occurrence {
                self.pattern_tokens.push(token.clone());
            }
            self.min_occurrence = 1;
        }
        if self.in_and_group {
            self.and_group_counter += 1;
        }
        if self.in_or_group {
            self.or_group_counter += 1;
        }
        self.reset_token();
    }

    fn new_match(attrs: &Attrs, suppress_misspelled: bool) -> Match {
        Match::new(
            attr(attrs, "postag"),
            attr(attrs, "postag_replace"),
            yes(attrs, "postag_regexp"),
            attr(attrs, "regexp_match"),
            attr(attrs, "regexp_replace"),
            attr(attrs, "case_conversion").map(CaseConversion::parse).unwrap_or(CaseConversion::None),
            yes(attrs, "setpos"),
            suppress_misspelled,
            attr(attrs, "include_skipped").map(IncludeRange::parse).unwrap_or(IncludeRange::None),
        )
    }

    /// `setMatchElement` (grammar files).
    fn set_match_element(&mut self, attrs: &Attrs, suppress_misspelled: bool) {
        self.in_match = true;
        self.match_text = String::new();
        let mut m = Common::new_match(attrs, suppress_misspelled);
        m.in_message_only = !self.in_suggestion;
        let no = attr(attrs, "no").unwrap_or("");
        if self.in_message {
            self.suggestion_matches.push(m);
            self.message.push_str("\u{1}\\");
            self.message.push_str(no);
        } else if self.in_suggestion {
            self.suggestion_matches_out_msg.push(m);
            self.suggestions_out_msg.push_str("\u{1}\\");
            self.suggestions_out_msg.push_str(no);
        } else if self.in_token && !no.is_empty() {
            if let Ok(n) = no.parse::<usize>() {
                m.token_ref = n;
                self.token_reference = Some(m);
                self.elements.push('\\');
                self.elements.push_str(no);
            }
        }
    }

    /// `addLegacyMatches`: plain `\N` references in a message get a default
    /// `Match`; those written as `<match>` keep theirs (marked by U+0001).
    fn add_legacy_matches(existing: Vec<Match>, message: &mut String) -> Vec<Match> {
        let mut sug: Vec<Match> = Vec::new();
        let chars: Vec<char> = message.chars().collect();
        let mut removals: Vec<usize> = Vec::new();
        let mut match_counter = 0;
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '\\' && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit()) {
                if i == 0 || chars[i - 1] != '\u{1}' {
                    let mut m = Match::new(None, None, false, None, None, CaseConversion::None, false, false, IncludeRange::None);
                    m.in_message_only = true;
                    sug.push(m);
                } else if let Some(m) = existing.get(match_counter) {
                    sug.push(m.clone());
                    removals.push(i - 1);
                    match_counter += 1;
                }
            }
            i += 1;
        }
        if !removals.is_empty() {
            *message = chars.iter().enumerate().filter(|(i, _)| !removals.contains(i)).map(|(_, c)| *c).collect();
        }
        if sug.is_empty() {
            existing
        } else {
            sug
        }
    }

    fn finalize_phrase(&mut self) {
        let in_marker = self.in_marker;
        for t in &mut self.pattern_tokens {
            t.inside_marker = in_marker;
        }
        if self.phrase_pattern_tokens.is_empty() {
            self.phrase_pattern_tokens.push(self.pattern_tokens.clone());
        } else {
            for ph in &mut self.phrase_pattern_tokens {
                ph.extend(self.pattern_tokens.iter().cloned());
            }
        }
        if let Some(id) = self.phrase_id.clone() {
            self.phrase_map.insert(id, self.phrase_pattern_tokens.clone());
        }
        self.pattern_tokens.clear();
        self.phrase_pattern_tokens.clear();
    }

    fn prepare_phrase(&mut self, attrs: &Attrs) {
        let Some(idref) = attr(attrs, "idref") else { return };
        let Some(phrases) = self.phrase_map.get(idref).cloned() else { return };
        for cur in phrases {
            let mut copy = cur;
            for t in &mut copy {
                t.phrase_name = Some(Arc::from(idref));
                t.inside_marker = self.in_marker;
            }
            if self.pattern_tokens.is_empty() {
                self.phrase_pattern_tokens.push(copy);
            } else {
                let mut prev = self.pattern_tokens.clone();
                prev.extend(copy);
                self.phrase_pattern_tokens.push(prev);
            }
        }
        self.last_phrase = true;
    }

    /// `processElement`: shift token references inside a phrase.
    fn process_element(tokens: &mut [PatternToken]) {
        for (counter, t) in tokens.iter_mut().enumerate() {
            if t.phrase_name.is_some() && counter > 0 {
                if let Some(mut r) = t.token_reference.clone() {
                    let tok_ref = r.token_ref;
                    r.token_ref = tok_ref + counter - 1;
                    let s = t.string().replace(&format!("\\{tok_ref}"), &format!("\\{}", tok_ref + counter - 1));
                    t.token_reference = Some(r);
                    t.set_string_element(&s);
                }
            }
        }
    }
}

fn make_filter(class: Option<&str>, args: Option<&str>, factory: FilterFactory) -> (Option<FilterRef>, Option<String>) {
    match (class, args) {
        (Some(class), Some(args)) => {
            let short = class.rsplit('.').next().unwrap_or(class);
            let filter = match factory(short) {
                Some(f) => FilterRef::Native(f),
                None => FilterRef::Unsupported(short.to_string()),
            };
            (Some(filter), Some(args.to_string()))
        }
        _ => (None, None),
    }
}

/// Load a grammar file (`PatternRuleHandler`). `<unification>` definitions
/// are added to `unifier`.
pub fn load_grammar(xml: &str, source_file: &str, is_style_file: bool, factory: FilterFactory, unifier: &mut UnifierConfiguration) -> Vec<GrammarRule> {
    let mut h = GrammarHandler {
        c: Common::new(),
        rules: Vec::new(),
        source_file: source_file.to_string(),
        is_style_file,
        category_id: String::new(),
        category_name: String::new(),
        category_default_off: false,
        category_issue_type: None,
        rule_group_issue_type: None,
        rule_issue_type: None,
        name: None,
        filter_class: None,
        filter_args: None,
        rulegroup_anti_patterns: Vec::new(),
        rule_anti_patterns: Vec::new(),
        sub_id: 0,
        raw_pos: false,
        default_off: false,
        default_temp_off: false,
        rule_group_default_off: false,
        rule_group_default_temp_off: false,
        rule_group_description: None,
        rule_group_id: None,
        start_pos: -1,
        end_pos: -1,
        token_count_for_marker: 0,
        anti_pattern_counter: 0,
        in_rule: false,
        in_anti_pattern: false,
        is_rule_suppress_misspelled: false,
        is_suggestion_suppress_misspelled: false,
        min_prev_matches: 0,
        rule_group_min_prev_matches: 0,
        distance_tokens: 0,
        rule_group_distance_tokens: 0,
        id_prefix: None,
        category_tags: Vec::new(),
        rule_group_tags: Vec::new(),
        rule_tags: Vec::new(),
        category_tone_tags: Vec::new(),
        rule_group_tone_tags: Vec::new(),
        rule_tone_tags: Vec::new(),
        goal_specific_category: None,
        goal_specific_group: None,
        goal_specific: false,
        prio_category: 0,
        prio_rule_group: 0,
        prio_rule: 0,
        in_regex: false,
        regex: String::new(),
        regex_exact: false,
        regex_case_sensitive: false,
        regexp_mark: 0,
        in_short_message: false,
        in_short_message_for_rule_group: false,
        short_message: String::new(),
        short_message_for_rule_group: String::new(),
        in_url: false,
        in_url_for_rule_group: false,
        url: String::new(),
        url_for_rule_group: String::new(),
        in_example: false,
    };
    let mut parser = Parser::new(xml);
    loop {
        match parser.next_event() {
            Event::Start { name, attrs, self_closing } => {
                h.start(&name, &attrs);
                if self_closing {
                    h.end(&name, factory, unifier);
                }
            }
            Event::End { name } => h.end(&name, factory, unifier),
            Event::Text(t) => h.characters(&t),
            Event::Eof => break,
        }
    }
    h.rules
}

struct GrammarHandler {
    c: Common,
    rules: Vec<GrammarRule>,
    source_file: String,
    is_style_file: bool,
    category_id: String,
    category_name: String,
    category_default_off: bool,
    category_issue_type: Option<String>,
    rule_group_issue_type: Option<String>,
    rule_issue_type: Option<String>,
    name: Option<String>,
    filter_class: Option<String>,
    filter_args: Option<String>,
    rulegroup_anti_patterns: Vec<Arc<DisambigRule>>,
    rule_anti_patterns: Vec<Arc<DisambigRule>>,
    sub_id: i32,
    raw_pos: bool,
    default_off: bool,
    default_temp_off: bool,
    rule_group_default_off: bool,
    rule_group_default_temp_off: bool,
    rule_group_description: Option<String>,
    rule_group_id: Option<String>,
    start_pos: i32,
    end_pos: i32,
    token_count_for_marker: i32,
    anti_pattern_counter: i32,
    in_rule: bool,
    in_anti_pattern: bool,
    is_rule_suppress_misspelled: bool,
    is_suggestion_suppress_misspelled: bool,
    min_prev_matches: i32,
    rule_group_min_prev_matches: i32,
    distance_tokens: i32,
    rule_group_distance_tokens: i32,
    id_prefix: Option<String>,
    category_tags: Vec<String>,
    rule_group_tags: Vec<String>,
    rule_tags: Vec<String>,
    category_tone_tags: Vec<String>,
    rule_group_tone_tags: Vec<String>,
    rule_tone_tags: Vec<String>,
    goal_specific_category: Option<bool>,
    goal_specific_group: Option<bool>,
    goal_specific: bool,
    prio_category: i32,
    prio_rule_group: i32,
    prio_rule: i32,
    in_regex: bool,
    regex: String,
    regex_exact: bool,
    regex_case_sensitive: bool,
    regexp_mark: usize,
    in_short_message: bool,
    in_short_message_for_rule_group: bool,
    short_message: String,
    short_message_for_rule_group: String,
    in_url: bool,
    in_url_for_rule_group: bool,
    url: String,
    url_for_rule_group: String,
    in_example: bool,
}

fn tone_tags_of(attrs: &Attrs) -> Vec<String> {
    attr(attrs, "tone_tags").map(|t| t.split(' ').filter(|x| !x.is_empty()).map(str::to_string).collect()).unwrap_or_default()
}

fn tags_of(attrs: &Attrs) -> Vec<String> {
    attr(attrs, "tags").map(|t| t.split(' ').filter(|x| !x.is_empty()).map(str::to_string).collect()).unwrap_or_default()
}

/// `replaceSpacesInRegex`: a space in a "smart" `<regexp>` matches any run of
/// (non-breaking) whitespace.
fn replace_spaces_in_regex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_bracket = false;
    for c in s.chars() {
        if c == '[' {
            in_bracket = true;
        } else if c == ']' {
            in_bracket = false;
        }
        if c == ' ' && !in_bracket {
            out.push_str("(?:[\\s\u{a0}\u{202f}]+)");
        } else {
            out.push(c);
        }
    }
    out
}

impl GrammarHandler {
    fn start(&mut self, q: &str, attrs: &Attrs) {
        match q {
            "category" => {
                self.category_name = attr(attrs, "name").unwrap_or("").to_string();
                self.category_id = attr(attrs, "id").unwrap_or("").to_string();
                self.category_default_off = attr(attrs, "default") == Some("off");
                if let Some(t) = attr(attrs, "type") {
                    self.category_issue_type = Some(t.to_string());
                }
                self.category_tags.extend(tags_of(attrs));
                self.category_tone_tags.extend(tone_tags_of(attrs));
                self.goal_specific_category = attr(attrs, "is_goal_specific").map(|v| v == "true");
                if let Some(p) = attr(attrs, "prio") {
                    self.prio_category = p.parse().unwrap_or(0);
                }
            }
            "rules" => {
                self.id_prefix = attr(attrs, "idprefix").map(str::to_string);
            }
            "regexp" => {
                self.in_regex = true;
                self.regex_exact = attr(attrs, "type") == Some("exact");
                self.regex_case_sensitive = yes(attrs, "case_sensitive");
                self.regexp_mark = attr(attrs, "mark").and_then(|m| m.parse().ok()).unwrap_or(0);
            }
            "rule" => {
                self.regex = String::new();
                self.in_rule = true;
                self.short_message = String::new();
                self.c.message = String::new();
                self.c.suggestions_out_msg = String::new();
                self.url = String::new();
                self.c.id = attr(attrs, "id").map(str::to_string);
                self.name = attr(attrs, "name").map(str::to_string);
                self.min_prev_matches = attr(attrs, "min_prev_matches").and_then(|v| v.parse().ok()).unwrap_or(self.rule_group_min_prev_matches);
                self.distance_tokens = attr(attrs, "distance_tokens").and_then(|v| v.parse().ok()).unwrap_or(self.rule_group_distance_tokens);
                if self.c.in_rule_group {
                    self.sub_id += 1;
                    if self.c.id.is_none() {
                        self.c.id = self.rule_group_id.clone();
                    }
                    if self.name.is_none() {
                        self.name = self.rule_group_description.clone();
                    }
                }
                if let (Some(prefix), Some(id)) = (&self.id_prefix, &self.c.id) {
                    self.c.id = Some(format!("{prefix}{id}"));
                }
                if self.c.in_rule_group && self.rule_group_default_off {
                    self.default_off = true;
                } else if self.c.in_rule_group && self.rule_group_default_temp_off {
                    self.default_temp_off = true;
                } else {
                    self.default_off = attr(attrs, "default") == Some("off");
                    self.default_temp_off = attr(attrs, "default") == Some("temp_off");
                }
                self.c.suggestion_matches.clear();
                self.c.suggestion_matches_out_msg.clear();
                if let Some(t) = attr(attrs, "type") {
                    self.rule_issue_type = Some(t.to_string());
                }
                self.is_rule_suppress_misspelled = false;
                self.rule_tags.extend(tags_of(attrs));
                self.rule_tone_tags.extend(tone_tags_of(attrs));
                self.goal_specific = attr(attrs, "is_goal_specific")
                    .map(|v| v == "true")
                    .or(self.goal_specific_group)
                    .or(self.goal_specific_category)
                    .unwrap_or(false);
                if let Some(p) = attr(attrs, "prio") {
                    self.prio_rule = p.parse().unwrap_or(0);
                }
            }
            "pattern" => {
                self.c.token_counter = 0;
                self.c.in_pattern = true;
                self.c.case_sensitive = yes(attrs, "case_sensitive");
                self.token_count_for_marker = 0;
                self.raw_pos = yes(attrs, "raw_pos");
            }
            "antipattern" => {
                self.in_anti_pattern = true;
                self.anti_pattern_counter += 1;
                self.c.case_sensitive = yes(attrs, "case_sensitive");
                self.c.token_counter = 0;
                self.token_count_for_marker = 0;
            }
            "and" => {
                self.c.in_and_group = true;
                self.token_count_for_marker += 1;
            }
            "or" => {
                self.c.in_or_group = true;
                self.token_count_for_marker += 1;
            }
            "unify" => {
                self.c.in_unification = true;
                self.c.uni_negation = yes(attrs, "negate");
            }
            "unify-ignore" => self.c.in_unification_neutral = true,
            "feature" => self.c.u_feature = attr(attrs, "id").map(str::to_string),
            "type" => {
                self.c.u_type = attr(attrs, "id").unwrap_or("").to_string();
                let t = self.c.u_type.clone();
                self.c.u_type_list.push(t);
            }
            "token" => {
                self.c.set_token(attrs);
                if !self.c.in_and_group && !self.c.in_or_group {
                    self.token_count_for_marker += 1;
                }
            }
            "exception" => self.c.set_exceptions(attrs),
            "example" => self.in_example = true,
            "filter" => {
                self.filter_class = attr(attrs, "class").map(str::to_string);
                self.filter_args = attr(attrs, "args").map(str::to_string);
            }
            "message" => {
                self.c.in_message = true;
                self.c.in_suggestion = false;
                self.c.message = String::new();
                self.is_rule_suppress_misspelled = yes(attrs, "suppress_misspelled");
                if self.is_rule_suppress_misspelled {
                    self.c.message.push_str(PLEASE_SPELL_ME);
                }
            }
            "suggestion" => {
                if self.in_example {
                    return;
                }
                let mut s = String::from("<suggestion>");
                self.is_suggestion_suppress_misspelled = yes(attrs, "suppress_misspelled");
                if self.is_suggestion_suppress_misspelled || self.is_rule_suppress_misspelled {
                    s.push_str(PLEASE_SPELL_ME);
                }
                if self.c.in_message {
                    self.c.message.push_str(&s);
                } else {
                    self.c.suggestions_out_msg.push_str(&s);
                }
                self.c.in_suggestion = true;
            }
            "short" => {
                if self.in_rule {
                    self.in_short_message = true;
                    self.short_message = String::new();
                } else {
                    self.in_short_message_for_rule_group = true;
                    self.short_message_for_rule_group = String::new();
                }
            }
            "url" => {
                if self.in_rule {
                    self.in_url = true;
                    self.url = String::new();
                } else {
                    self.in_url_for_rule_group = true;
                    self.url_for_rule_group = String::new();
                }
            }
            "rulegroup" => {
                self.rule_group_id = attr(attrs, "id").map(str::to_string);
                self.rule_group_description = attr(attrs, "name").map(str::to_string);
                self.rule_group_default_off = attr(attrs, "default") == Some("off");
                self.rule_group_default_temp_off = attr(attrs, "default") == Some("temp_off");
                self.url_for_rule_group = String::new();
                self.short_message_for_rule_group = String::new();
                self.c.in_rule_group = true;
                self.sub_id = 0;
                if let Some(t) = attr(attrs, "type") {
                    self.rule_group_issue_type = Some(t.to_string());
                }
                self.rule_group_tags.extend(tags_of(attrs));
                self.rule_group_tone_tags.extend(tone_tags_of(attrs));
                self.goal_specific_group = attr(attrs, "is_goal_specific").map(|v| v == "true");
                if let Some(v) = attr(attrs, "min_prev_matches").and_then(|v| v.parse().ok()) {
                    self.rule_group_min_prev_matches = v;
                }
                if let Some(v) = attr(attrs, "distance_tokens").and_then(|v| v.parse().ok()) {
                    self.rule_group_distance_tokens = v;
                }
                if let Some(p) = attr(attrs, "prio") {
                    self.prio_rule_group = p.parse().unwrap_or(0);
                }
            }
            "match" => {
                if self.in_example {
                    return;
                }
                let suppress = self.c.in_suggestion && (self.is_suggestion_suppress_misspelled || self.is_rule_suppress_misspelled);
                self.c.set_match_element(attrs, suppress);
            }
            "marker" => {
                if !self.in_example && (self.c.in_pattern || self.in_anti_pattern) {
                    self.start_pos = self.c.token_counter;
                    self.c.in_marker = true;
                }
            }
            "unification" => {
                self.c.u_feature = attr(attrs, "feature").map(str::to_string);
                self.c.in_unification_def = true;
            }
            "equivalence" => self.c.u_type = attr(attrs, "type").unwrap_or("").to_string(),
            "phrases" => self.c.in_phrases = true,
            "phrase" => {
                if self.c.in_phrases {
                    self.c.phrase_id = attr(attrs, "id").map(str::to_string);
                }
            }
            "phraseref" => {
                if attr(attrs, "idref").is_some() {
                    self.c.prepare_phrase(attrs);
                    self.token_count_for_marker += 1;
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, q: &str, factory: FilterFactory, unifier: &mut UnifierConfiguration) {
        match q {
            "category" => {
                self.category_issue_type = None;
                self.category_tags.clear();
                self.category_tone_tags.clear();
                self.goal_specific_category = None;
                self.prio_category = 0;
            }
            "regexp" => self.in_regex = false,
            "rule" => {
                let mut out = std::mem::take(&mut self.c.suggestions_out_msg);
                self.c.suggestion_matches_out_msg = Common::add_legacy_matches(std::mem::take(&mut self.c.suggestion_matches_out_msg), &mut out);
                self.c.suggestions_out_msg = out;
                if self.c.phrase_pattern_tokens.is_empty() {
                    let elems = std::mem::take(&mut self.c.pattern_tokens);
                    self.create_rules(&elems, Vec::new(), 0, factory);
                } else {
                    let tail = std::mem::take(&mut self.c.pattern_tokens);
                    let mut phrases = std::mem::take(&mut self.c.phrase_pattern_tokens);
                    for ph in &mut phrases {
                        ph.extend(tail.iter().cloned());
                        Common::process_element(ph);
                    }
                    for ph in &phrases {
                        self.create_rules(ph, Vec::new(), 0, factory);
                    }
                }
                self.c.pattern_tokens.clear();
                self.c.phrase_pattern_tokens.clear();
                self.rule_issue_type = None;
                self.in_rule = false;
                self.filter_class = None;
                self.filter_args = None;
                self.min_prev_matches = 0;
                self.distance_tokens = 0;
                self.rule_tags.clear();
                self.rule_tone_tags.clear();
                self.goal_specific = false;
                self.prio_rule = 0;
            }
            "exception" => self.c.finalize_exceptions(),
            "and" => {
                self.c.in_and_group = false;
                self.c.and_group_counter = 0;
                self.c.token_counter += 1;
            }
            "or" => {
                self.c.in_or_group = false;
                self.c.or_group_counter = 0;
                self.c.token_counter += 1;
            }
            "token" => self.c.finalize_tokens(unifier),
            "pattern" => {
                self.c.in_pattern = false;
                if self.c.last_phrase {
                    self.c.pattern_tokens.clear();
                }
                self.c.token_counter = 0;
            }
            "antipattern" => {
                let mut anti_id = self.c.id.clone().unwrap_or_default();
                if self.c.in_rule_group {
                    let group = self.rule_group_id.clone().unwrap_or_default();
                    anti_id = if self.sub_id > 0 { format!("{group}[{}]", self.sub_id) } else { group };
                }
                let mut tokens = std::mem::take(&mut self.c.pattern_tokens);
                let with_marker = self.start_pos != -1 && self.end_pos != -1;
                if !with_marker {
                    for t in &mut tokens {
                        t.inside_marker = true;
                    }
                }
                let mut rule = TokenRule::new(TokenPattern::new(tokens.into_iter().map(Arc::new).collect(), true));
                if with_marker {
                    rule.start_position_correction = self.start_pos;
                    rule.end_position_correction = self.end_pos - self.token_count_for_marker;
                }
                let anti = Arc::new(DisambigRule {
                    id: format!("{anti_id}_antipattern:{}", self.anti_pattern_counter),
                    rule,
                    disambiguated_pos: None,
                    match_element: None,
                    action: DisambigAction::Immunize,
                    new_token_readings: Vec::new(),
                    anti_patterns: Vec::new(),
                });
                if self.in_rule {
                    self.rule_anti_patterns.push(anti);
                } else {
                    self.rulegroup_anti_patterns.push(anti);
                }
                self.c.token_counter = 0;
                self.in_anti_pattern = false;
                self.end_pos = -1;
                self.start_pos = -1;
            }
            "example" => self.in_example = false,
            "message" => {
                let mut msg = std::mem::take(&mut self.c.message);
                self.c.suggestion_matches = Common::add_legacy_matches(std::mem::take(&mut self.c.suggestion_matches), &mut msg);
                self.c.message = msg;
                self.c.in_message = false;
            }
            "suggestion" => {
                if self.in_example {
                    return;
                }
                if self.c.in_message {
                    self.c.message.push_str("</suggestion>");
                } else {
                    self.c.suggestions_out_msg.push_str("</suggestion>");
                }
                self.c.in_suggestion = false;
            }
            "short" => {
                self.in_short_message = false;
                self.in_short_message_for_rule_group = false;
            }
            "url" => {
                self.in_url = false;
                self.in_url_for_rule_group = false;
            }
            "match" => {
                if self.in_example {
                    return;
                }
                let text = std::mem::take(&mut self.c.match_text);
                if self.c.in_message {
                    if let Some(m) = self.c.suggestion_matches.last_mut() {
                        m.set_lemma_string(&text);
                    }
                } else if self.c.in_suggestion {
                    if let Some(m) = self.c.suggestion_matches_out_msg.last_mut() {
                        m.set_lemma_string(&text);
                    }
                } else if self.c.in_token {
                    if let Some(m) = self.c.token_reference.as_mut() {
                        m.set_lemma_string(&text);
                    }
                }
                self.c.in_match = false;
            }
            "rulegroup" => {
                self.url_for_rule_group = String::new();
                self.short_message_for_rule_group = String::new();
                self.c.in_rule_group = false;
                self.rule_group_issue_type = None;
                self.rulegroup_anti_patterns.clear();
                self.anti_pattern_counter = 0;
                self.rule_group_default_off = false;
                self.rule_group_default_temp_off = false;
                self.default_off = false;
                self.default_temp_off = false;
                self.rule_group_min_prev_matches = 0;
                self.rule_group_distance_tokens = 0;
                self.rule_group_tags.clear();
                self.rule_group_tone_tags.clear();
                self.goal_specific_group = None;
                self.prio_rule_group = 0;
            }
            "marker" => {
                if !self.in_example && (self.c.in_pattern || self.in_anti_pattern) {
                    self.end_pos = self.token_count_for_marker;
                    self.c.in_marker = false;
                }
            }
            "phrase" => {
                if self.c.in_phrases {
                    self.c.finalize_phrase();
                }
            }
            "includephrases" => self.c.pattern_tokens.clear(),
            "phrases" => self.c.in_phrases = false,
            "unification" => self.c.in_unification_def = false,
            "feature" => {
                if let Some(f) = self.c.u_feature.clone() {
                    let list = std::mem::take(&mut self.c.u_type_list);
                    self.c.equivalence_features.insert(f, list);
                }
            }
            "unify" => {
                self.c.in_unification = false;
                self.c.equivalence_features = UniFeatures::new();
                let neg = self.c.uni_negation;
                if let Some(last) = self.c.pattern_tokens.last_mut() {
                    last.last_in_unification = true;
                    if neg {
                        last.uni_negation = true;
                    }
                }
            }
            "unify-ignore" => self.c.in_unification_neutral = false,
            _ => {}
        }
    }

    fn characters(&mut self, s: &str) {
        let c = &mut self.c;
        if c.in_exception {
            c.exceptions.push_str(s);
        } else if c.in_token {
            c.elements.push_str(s);
        } else if self.in_example {
        } else if c.in_match {
            c.match_text.push_str(s);
        } else if c.in_message {
            c.message.push_str(s);
        } else if c.in_suggestion {
            c.suggestions_out_msg.push_str(s);
        } else if self.in_short_message {
            self.short_message.push_str(s);
        } else if self.in_short_message_for_rule_group {
            self.short_message_for_rule_group.push_str(s);
        } else if self.in_url {
            self.url.push_str(s);
        } else if self.in_url_for_rule_group {
            self.url_for_rule_group.push_str(s);
        } else if self.in_regex {
            self.regex.push_str(s);
        }
    }

    fn info(&self) -> RuleInfo {
        let issue_type = self
            .rule_issue_type
            .clone()
            .or_else(|| self.rule_group_issue_type.clone())
            .or_else(|| self.category_issue_type.clone())
            .unwrap_or_else(|| "uncategorized".to_string());
        let mut prio = 0;
        if self.prio_category != 0 {
            prio = self.prio_category;
        }
        if self.prio_rule_group != 0 {
            prio = self.prio_rule_group;
        }
        if self.prio_rule != 0 {
            prio = self.prio_rule;
        }
        let mut tags = self.rule_tags.clone();
        tags.extend(self.rule_group_tags.iter().cloned());
        tags.extend(self.category_tags.iter().cloned());
        let mut tone_tags = self.rule_tone_tags.clone();
        tone_tags.extend(self.rule_group_tone_tags.iter().cloned());
        tone_tags.extend(self.category_tone_tags.iter().cloned());
        let url = if !self.url.is_empty() {
            Some(self.url.clone())
        } else if !self.url_for_rule_group.is_empty() {
            Some(self.url_for_rule_group.clone())
        } else {
            None
        };
        RuleInfo {
            id: self.c.id.clone().unwrap_or_default(),
            sub_id: Some(if self.c.in_rule_group { self.sub_id.to_string() } else { "1".to_string() }),
            description: self.name.clone().unwrap_or_default(),
            category_id: self.category_id.clone(),
            category_name: self.category_name.clone(),
            category_default_off: self.category_default_off,
            issue_type,
            default_off: self.default_off,
            default_temp_off: self.default_temp_off,
            tags,
            tone_tags,
            goal_specific: self.goal_specific,
            url,
            priority: prio,
            source_file: self.source_file.clone(),
            min_prev_matches: self.min_prev_matches,
            distance_tokens: self.distance_tokens,
            is_style_file: self.is_style_file,
        }
    }

    /// `createRules`: one rule per combination of `<or>` alternatives.
    fn create_rules(&mut self, elem_list: &[PatternToken], tmp: Vec<PatternToken>, num_element: usize, factory: FilterFactory) {
        let short_message = if !self.short_message.is_empty() { self.short_message.clone() } else { self.short_message_for_rule_group.clone() };
        if num_element >= elem_list.len() {
            let info = self.info();
            let (filter, filter_args) = make_filter(self.filter_class.as_deref(), self.filter_args.as_deref(), factory);
            if !tmp.is_empty() {
                let mut rule = TokenRule::new(TokenPattern::new(tmp.into_iter().map(Arc::new).collect(), false));
                rule.message = self.c.message.clone();
                rule.short_message = short_message;
                rule.suggestions_out_msg = self.c.suggestions_out_msg.clone();
                rule.raw_pos = self.raw_pos;
                rule.compute_element_no();
                rule.filter = filter;
                rule.filter_args = filter_args;
                if self.start_pos != -1 && self.end_pos != -1 {
                    rule.start_position_correction = self.start_pos;
                    rule.end_position_correction = self.end_pos - self.token_count_for_marker;
                }
                rule.suggestion_matches = self.c.suggestion_matches.clone();
                rule.suggestion_matches_out_msg = self.c.suggestion_matches_out_msg.clone();
                let mut anti = self.rulegroup_anti_patterns.clone();
                anti.extend(self.rule_anti_patterns.iter().cloned());
                self.rule_anti_patterns.clear();
                self.rules.push(GrammarRule::Pattern(PatternRule { info, rule, anti_patterns: anti }));
            } else if !self.regex.is_empty() {
                let flags = if self.regex_case_sensitive { Flags::UNICODE_CLASS } else { Flags::IGNORE_CASE_UNICODE_CLASS };
                let mut regex_str = self.regex.clone();
                if !self.regex_exact {
                    regex_str = replace_spaces_in_regex(&regex_str);
                }
                if let Ok(pattern) = re::compile(&regex_str, flags) {
                    self.rules.push(GrammarRule::Regex(RegexRule {
                        info,
                        pattern,
                        mark_group: self.regexp_mark,
                        message: self.c.message.clone(),
                        short_message,
                        suggestions_out_msg: self.c.suggestions_out_msg.clone(),
                        suggestion_matches: self.c.suggestion_matches.clone(),
                        suggestion_matches_out_msg: self.c.suggestion_matches_out_msg.clone(),
                        filter,
                    }));
                }
            }
            // prepareRule's bookkeeping
            self.start_pos = -1;
            self.end_pos = -1;
            self.c.case_sensitive = false;
            if self.c.phrase_pattern_tokens.len() <= 1 {
                self.c.suggestion_matches.clear();
            }
            self.c.suggestion_matches_out_msg.clear();
        } else {
            let token = &elem_list[num_element];
            if token.has_or_group() {
                let matches_backup = self.c.suggestion_matches.clone();
                let out_backup = self.c.suggestion_matches_out_msg.clone();
                let start_backup = self.start_pos;
                let end_backup = self.end_pos;
                let anti_backup = self.rule_anti_patterns.clone();
                for alt in &token.or_group {
                    let mut tmp2 = tmp.clone();
                    tmp2.push(alt.clone());
                    self.create_rules(elem_list, tmp2, num_element + 1, factory);
                    self.start_pos = start_backup;
                    self.end_pos = end_backup;
                    self.c.suggestion_matches = matches_backup.clone();
                    self.c.suggestion_matches_out_msg = out_backup.clone();
                    self.rule_anti_patterns.extend(anti_backup.iter().cloned());
                }
            }
            let mut tmp = tmp;
            tmp.push(token.clone());
            self.create_rules(elem_list, tmp, num_element + 1, factory);
        }
    }
}

/// Load a disambiguation file (`DisambiguationRuleHandler`).
pub fn load_disambiguation(xml: &str, factory: FilterFactory, unifier: &mut UnifierConfiguration) -> Vec<Arc<DisambigRule>> {
    let mut c = Common::new();
    let mut rules: Vec<Arc<DisambigRule>> = Vec::new();
    let mut in_disambiguation = false;
    let mut sub_id = 0;
    let mut rule_group_id: Option<String> = None;
    let mut filter_class: Option<String> = None;
    let mut filter_args: Option<String> = None;
    let mut anti_pattern_counter = 0;
    let mut in_rule = false;
    let mut rulegroup_anti: Vec<Arc<DisambigRule>> = Vec::new();
    let mut rule_anti: Vec<Arc<DisambigRule>> = Vec::new();
    let mut in_anti_pattern = false;
    let mut in_word = false;
    let mut in_example = false;
    let mut disambiguated_pos: Option<String> = None;
    let mut start_pos: i32 = -1;
    let mut end_pos: i32 = -1;
    let mut token_count_for_marker = 0;
    let mut pos_selector: Option<Match> = None;
    let mut new_wd_list: Option<Vec<Reading>> = None;
    let mut wd_lemma: Option<String> = None;
    let mut wd_pos: Option<String> = None;
    let mut wd = String::new();
    let mut action = DisambigAction::Replace;

    let mut parser = Parser::new(xml);
    loop {
        let ev = parser.next_event();
        let (name, attrs, is_start, also_end) = match ev {
            Event::Start { name, attrs, self_closing } => (name, attrs, true, self_closing),
            Event::End { name } => (name, Vec::new(), false, true),
            Event::Text(s) => {
                if c.in_exception {
                    c.exceptions.push_str(&s);
                } else if c.in_token && (c.in_pattern || in_anti_pattern) {
                    c.elements.push_str(&s);
                } else if in_example {
                } else if c.in_match {
                    c.match_text.push_str(&s);
                } else if in_word {
                    wd.push_str(&s);
                }
                continue;
            }
            Event::Eof => break,
        };
        if is_start {
            match name.as_str() {
                "rule" => {
                    in_rule = true;
                    c.id = attr(&attrs, "id").map(str::to_string);
                    if c.in_rule_group {
                        sub_id += 1;
                        if c.id.is_none() {
                            c.id = rule_group_id.clone();
                        }
                    }
                }
                "pattern" => {
                    c.in_pattern = true;
                    token_count_for_marker = 0;
                    if yes(&attrs, "case_sensitive") {
                        c.case_sensitive = true;
                    }
                }
                "antipattern" => {
                    in_anti_pattern = true;
                    anti_pattern_counter += 1;
                    c.case_sensitive = yes(&attrs, "case_sensitive");
                    c.token_counter = 0;
                    token_count_for_marker = 0;
                }
                "exception" => c.set_exceptions(&attrs),
                "and" => {
                    c.in_and_group = true;
                    token_count_for_marker += 1;
                }
                "unify" => {
                    c.in_unification = true;
                    c.uni_negation = yes(&attrs, "negate");
                }
                "unify-ignore" => c.in_unification_neutral = true,
                "feature" => c.u_feature = attr(&attrs, "id").map(str::to_string),
                "type" => {
                    c.u_type = attr(&attrs, "id").unwrap_or("").to_string();
                    let t = c.u_type.clone();
                    c.u_type_list.push(t);
                }
                "token" => {
                    c.set_token(&attrs);
                    if !c.in_and_group {
                        token_count_for_marker += 1;
                    }
                }
                "disambig" => {
                    in_disambiguation = true;
                    disambiguated_pos = attr(&attrs, "postag").map(str::to_string);
                    action = attr(&attrs, "action").and_then(DisambigAction::parse).unwrap_or(DisambigAction::Replace);
                }
                "match" => {
                    if !in_example {
                        c.in_match = true;
                        c.match_text = String::new();
                        let mut m = Common::new_match(&attrs, yes(&attrs, "suppress_mispelled"));
                        if let Some(n) = attr(&attrs, "no").and_then(|n| n.parse::<usize>().ok()) {
                            m.token_ref = n;
                            if in_disambiguation {
                                pos_selector = Some(m);
                            } else if c.in_token {
                                c.token_reference = Some(m);
                                c.elements.push('\\');
                                c.elements.push_str(&n.to_string());
                            }
                        }
                    }
                }
                "rulegroup" => {
                    rule_group_id = attr(&attrs, "id").map(str::to_string);
                    c.in_rule_group = true;
                    sub_id = 0;
                    rulegroup_anti.clear();
                    anti_pattern_counter = 0;
                }
                "unification" => {
                    c.u_feature = attr(&attrs, "feature").map(str::to_string);
                    c.in_unification_def = true;
                }
                "equivalence" => c.u_type = attr(&attrs, "type").unwrap_or("").to_string(),
                "wd" => {
                    wd_lemma = attr(&attrs, "lemma").map(str::to_string);
                    wd_pos = attr(&attrs, "pos").map(str::to_string);
                    in_word = true;
                    wd = String::new();
                }
                "example" => in_example = true,
                "filter" => {
                    filter_class = attr(&attrs, "class").map(str::to_string);
                    filter_args = attr(&attrs, "args").map(str::to_string);
                }
                "marker" => {
                    if !in_example && (c.in_pattern || in_anti_pattern) {
                        start_pos = c.token_counter;
                        c.in_marker = true;
                    }
                }
                _ => {}
            }
        }
        if also_end {
            match name.as_str() {
                "rule" => {
                    let tokens = std::mem::take(&mut c.pattern_tokens);
                    let mut rule = TokenRule::new(TokenPattern::new(tokens.into_iter().map(Arc::new).collect(), true));
                    if start_pos != -1 && end_pos != -1 {
                        rule.start_position_correction = start_pos;
                        rule.end_position_correction = end_pos - token_count_for_marker;
                    }
                    let (filter, args) = make_filter(filter_class.as_deref(), filter_args.as_deref(), factory);
                    rule.filter = filter;
                    rule.filter_args = args;
                    let mut new_readings = Vec::new();
                    if let Some(list) = new_wd_list.as_mut() {
                        if matches!(action, DisambigAction::AddChunk | DisambigAction::Add | DisambigAction::Remove | DisambigAction::Replace) {
                            new_readings = list.clone();
                        }
                        list.clear();
                    }
                    c.case_sensitive = false;
                    let mut anti = rulegroup_anti.clone();
                    anti.append(&mut rule_anti);
                    let id = c.id.clone().unwrap_or_default();
                    let full_id = if c.in_rule_group { format!("{id}[{sub_id}]") } else { format!("{id}[1]") };
                    rules.push(Arc::new(DisambigRule {
                        id: full_id,
                        rule,
                        disambiguated_pos: disambiguated_pos.clone(),
                        match_element: pos_selector.take(),
                        action,
                        new_token_readings: new_readings,
                        anti_patterns: anti,
                    }));
                    start_pos = -1;
                    end_pos = -1;
                    filter_class = None;
                    filter_args = None;
                    in_rule = false;
                }
                "exception" => c.finalize_exceptions(),
                "and" => {
                    c.in_and_group = false;
                    c.and_group_counter = 0;
                    c.token_counter += 1;
                }
                "token" => c.finalize_tokens(unifier),
                "pattern" => {
                    c.in_pattern = false;
                    c.token_counter = 0;
                }
                "match" => {
                    if !in_example {
                        let text = std::mem::take(&mut c.match_text);
                        if in_disambiguation {
                            if let Some(m) = pos_selector.as_mut() {
                                m.set_lemma_string(&text);
                            }
                        } else if c.in_token {
                            if let Some(m) = c.token_reference.as_mut() {
                                m.set_lemma_string(&text);
                            }
                        }
                        c.in_match = false;
                    }
                }
                "disambig" => in_disambiguation = false,
                "rulegroup" => c.in_rule_group = false,
                "unification" => {
                    if c.in_unification_def {
                        c.in_unification_def = false;
                        c.token_counter = 0;
                    }
                }
                "feature" => {
                    if let Some(f) = c.u_feature.clone() {
                        let list = std::mem::take(&mut c.u_type_list);
                        c.equivalence_features.insert(f, list);
                    }
                }
                "unify" => {
                    c.in_unification = false;
                    c.equivalence_features = UniFeatures::new();
                    let neg = c.uni_negation;
                    if let Some(last) = c.pattern_tokens.last_mut() {
                        last.last_in_unification = true;
                        if neg {
                            last.uni_negation = true;
                        }
                    }
                }
                "unify-ignore" => c.in_unification_neutral = false,
                "wd" => {
                    let reading = Reading::new(Arc::from(wd.as_str()), wd_pos.as_deref().map(|p| Tag::new(p.trim())), wd_lemma.as_deref().map(Arc::from));
                    new_wd_list.get_or_insert_with(Vec::new).push(reading);
                    in_word = false;
                }
                "antipattern" => {
                    let tokens = std::mem::take(&mut c.pattern_tokens);
                    let mut rule = TokenRule::new(TokenPattern::new(tokens.into_iter().map(Arc::new).collect(), true));
                    if start_pos != -1 && end_pos != -1 {
                        rule.start_position_correction = start_pos;
                        rule.end_position_correction = end_pos - token_count_for_marker;
                    }
                    let anti = Arc::new(DisambigRule {
                        id: format!("{}_antipattern:{anti_pattern_counter}", c.id.clone().unwrap_or_default()),
                        rule,
                        disambiguated_pos: None,
                        match_element: None,
                        action: DisambigAction::Immunize,
                        new_token_readings: Vec::new(),
                        anti_patterns: Vec::new(),
                    });
                    if in_rule {
                        rule_anti.push(anti);
                    } else {
                        rulegroup_anti.push(anti);
                    }
                    c.token_counter = 0;
                    in_anti_pattern = false;
                }
                "example" => in_example = false,
                "marker" => {
                    if !in_example && (c.in_pattern || in_anti_pattern) {
                        end_pos = token_count_for_marker;
                        c.in_marker = false;
                    }
                }
                _ => {}
            }
        }
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_filters(_: &str) -> Option<Arc<dyn RuleFilter>> {
        None
    }

    const GRAMMAR: &str = r#"<?xml version="1.0"?>
<!DOCTYPE rules [ <!ENTITY aux "avoir|être"> ]>
<rules lang="fr">
 <unification feature="number"><equivalence type="sg"><token postag=".* s" postag_regexp="yes"/></equivalence></unification>
 <category id="CAT_A" name="Catégorie A" type="grammar">
  <rule id="R_INF" name="participe attendu">
   <antipattern><token>a</token><token>manger</token><token>chaud</token></antipattern>
   <pattern>
    <token inflected="yes" regexp="yes">&aux;<exception postag="N.*" postag_regexp="yes"/></token>
    <marker><token postag="V inf"/></marker>
   </pattern>
   <message>Écrivez <suggestion><match no="2" postag="V inf" postag_replace="V ppa m s"/></suggestion> après \1.</message>
   <short>Participe</short>
   <example correction="mangé">Il a <marker>manger</marker>.</example>
  </rule>
  <rulegroup id="G" name="groupe" default="off">
   <rule><pattern><or><token>le</token><token>la</token></or><token skip="2" min="0">x</token><token>y</token></pattern><message>m</message></rule>
   <rule><regexp mark="1">qu (elle|il)</regexp><message>Élision : <suggestion>qu'\1</suggestion></message></rule>
  </rulegroup>
 </category>
</rules>"#;

    #[test]
    fn grammar_file_structure() {
        let mut u = UnifierConfiguration::default();
        let rules = load_grammar(GRAMMAR, "grammar.xml", false, &no_filters, &mut u);
        assert!(!u.is_empty());
        // R_INF, two <or> alternatives, one regexp rule
        assert_eq!(rules.len(), 4);
        let GrammarRule::Pattern(r) = &rules[0] else { panic!("pattern rule") };
        assert_eq!(r.info.id, "R_INF");
        assert_eq!(r.info.full_id(), "R_INF[1]");
        assert_eq!(r.info.category_id, "CAT_A");
        assert_eq!(r.info.issue_type, "grammar");
        assert_eq!(r.anti_patterns.len(), 1);
        let toks = &r.rule.pattern.tokens;
        assert_eq!(toks.len(), 2);
        assert!(toks[0].inflected && toks[0].text.is_regexp);
        assert!(!toks[0].inside_marker && toks[1].inside_marker);
        assert_eq!(toks[0].current_and_next_exceptions.len(), 1);
        assert_eq!(toks[1].pos_tag(), Some("V inf"));
        assert_eq!(r.rule.start_position_correction, 1);
        assert_eq!(r.rule.end_position_correction, 0);
        assert_eq!(r.rule.message, "Écrivez <suggestion>\\2</suggestion> après \\1.");
        assert_eq!(r.rule.suggestion_matches.len(), 2);
        assert_eq!(r.rule.suggestion_matches[0].pos_tag_replace.as_deref(), Some("V ppa m s"));
        assert!(r.rule.suggestion_matches[1].in_message_only);
        assert_eq!(r.rule.short_message, "Participe");
        // the hint comes from the lemma alternation
        assert!(r.rule.pattern.token_hints.iter().any(|h| h.inflected && h.lower_case_values.len() == 2));

        let GrammarRule::Pattern(a) = &rules[1] else { panic!() };
        let GrammarRule::Pattern(b) = &rules[2] else { panic!() };
        assert_eq!(a.info.full_id(), "G[1]");
        assert!(a.info.default_off && b.info.default_off);
        // As in LanguageTool, the <or> alternatives come first.
        assert_eq!(a.rule.pattern.tokens[0].string(), "la");
        assert_eq!(b.rule.pattern.tokens[0].string(), "le");
        assert!(a.rule.pattern.tokens[1].may_be_omitted);
        assert_eq!(a.rule.pattern.tokens[1].skip, 2);
        let GrammarRule::Regex(x) = &rules[3] else { panic!("regex rule") };
        assert_eq!(x.info.full_id(), "G[2]");
        assert_eq!(x.mark_group, 1);
    }

    #[test]
    fn disambiguation_file_structure() {
        let xml = r#"<rules lang="fr">
 <rule id="D1" name="x"><pattern><token>la</token><marker><token postag="N.*" postag_regexp="yes"/></marker></pattern><disambig action="filter" postag="N.*"/></rule>
 <rulegroup id="D2" name="y">
  <rule><pattern><token>a</token></pattern><disambig action="remove"><wd pos="N m sp"/></disambig></rule>
  <rule><pattern><token>est</token></pattern><disambig><wd lemma="être" pos="V etre ind pres 3 s">est</wd></disambig></rule>
 </rulegroup>
</rules>"#;
        let mut u = UnifierConfiguration::default();
        let rules = load_disambiguation(xml, &no_filters, &mut u);
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].id, "D1[1]");
        assert_eq!(rules[0].action, DisambigAction::Filter);
        assert_eq!(rules[0].disambiguated_pos.as_deref(), Some("N.*"));
        assert_eq!(rules[0].rule.start_position_correction, 1);
        assert_eq!(rules[1].id, "D2[1]");
        assert_eq!(rules[1].action, DisambigAction::Remove);
        assert_eq!(rules[1].new_token_readings.len(), 1);
        assert_eq!(rules[1].new_token_readings[0].pos(), Some("N m sp"));
        assert_eq!(rules[2].action, DisambigAction::Replace);
        assert_eq!(rules[2].new_token_readings[0].lemma.as_deref(), Some("être"));
        assert_eq!(&*rules[2].new_token_readings[0].token, "est");
    }
}
