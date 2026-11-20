//! Matching a token pattern against a sentence.
//!
//! Port of `org.languagetool.rules.patterns.AbstractPatternRulePerformer`,
//! `PatternTokenMatcher` and the token hints of `AbstractTokenBasedRule`
//! (LanguageTool, LGPL-2.1-or-later). The same algorithm drives grammar rules
//! and disambiguation rules; the control flow is kept as in the original,
//! including its handling of optional elements, `skip`, scoped exceptions,
//! AND groups and unification.

use std::collections::HashMap;
use std::sync::Arc;

use super::pattern::{Lang, PatternToken};
use super::token::{Reading, Readings, Sentence};
use super::unifier::UnifierConfiguration;

/// `AbstractTokenBasedRule.TokenHint`: strings one of which must occur in the
/// sentence (as a surface form, or as a lemma when `inflected`) at the
/// position of pattern element `token_index`.
#[derive(Clone, Debug)]
pub struct TokenHint {
    pub inflected: bool,
    pub lower_case_values: Vec<String>,
    pub token_index: usize,
}

impl TokenHint {
    fn indices<'s>(&self, sentence: &'s Sentence, hint: &str) -> Option<&'s Vec<usize>> {
        if self.inflected {
            sentence.lemma_offsets.get(hint)
        } else {
            sentence.token_offsets.get(hint)
        }
    }

    pub fn can_be_ignored_for(&self, sentence: &Sentence) -> bool {
        !self.lower_case_values.iter().any(|h| self.indices(sentence, h).is_some())
    }

    /// Positions (among non-whitespace tokens) where a hinted string occurs.
    pub fn possible_indices(&self, sentence: &Sentence) -> Vec<usize> {
        let mut result: Vec<usize> = Vec::new();
        let mut lists = 0;
        for h in &self.lower_case_values {
            if let Some(v) = self.indices(sentence, h) {
                result.extend_from_slice(v);
                lists += 1;
            }
        }
        if lists > 1 {
            result.sort_unstable();
            result.dedup();
        }
        result
    }
}

/// The matching-relevant part of a rule (`AbstractTokenBasedRule`).
#[derive(Clone, Debug)]
pub struct TokenPattern {
    pub tokens: Vec<Arc<PatternToken>>,
    pub test_unification: bool,
    pub sent_start: bool,
    pub groups_or_unification: bool,
    pub get_unified: bool,
    pub token_hints: Vec<TokenHint>,
    pub anchor_hint: Option<TokenHint>,
    pub min_token_count: usize,
}

impl TokenPattern {
    pub fn new(tokens: Vec<Arc<PatternToken>>, get_unified: bool) -> TokenPattern {
        let test_unification = tokens.iter().any(|t| t.is_unified());
        let sent_start = tokens.first().is_some_and(|t| t.is_sentence_start());
        let groups_or_unification = test_unification || tokens.iter().any(|t| t.has_and_group());

        // AbstractTokenBasedRule: hints and the minimal sentence length.
        let can_match_sentence_start =
            |t: &PatternToken| t.is_sentence_start() || t.negation || !t.has_string_that_must_match();
        let mut hints: Vec<TokenHint> = Vec::new();
        let mut anchor: Option<TokenHint> = None;
        let mut fixed_offset = true;
        let mut min_token_count = if tokens.is_empty() || can_match_sentence_start(&tokens[0]) { 0 } else { 1 };
        for (i, token) in tokens.iter().enumerate() {
            if token.min_occurrence() > 0 {
                min_token_count += 1;
            }
            let mut inflected = false;
            let mut values = token.calc_string_hints(false);
            if values.is_none() {
                inflected = true;
                values = token.calc_string_hints(true);
            }
            if let Some(values) = values {
                let mut lower: Vec<String> = values.iter().map(|s| s.to_lowercase()).collect();
                lower.sort();
                lower.dedup();
                let hint = TokenHint { inflected, lower_case_values: lower, token_index: i };
                if !hints.iter().any(|h| {
                    h.inflected == hint.inflected && h.token_index == hint.token_index && h.lower_case_values == hint.lower_case_values
                }) {
                    hints.push(hint.clone());
                }
                if fixed_offset && anchor.is_none() {
                    anchor = Some(hint);
                }
            }
            if fixed_offset && (token.min_occurrence() != 1 || token.skip != 0 || token.max_occurrence != 1) {
                fixed_offset = false;
            }
        }
        // Most selective hints first.
        hints.sort_by_key(|h| {
            (h.lower_case_values.len(), -(h.lower_case_values.iter().map(|s| s.chars().count()).min().unwrap_or(0) as i64))
        });
        TokenPattern {
            tokens,
            test_unification,
            sent_start,
            groups_or_unification,
            get_unified,
            token_hints: hints,
            anchor_hint: anchor,
            min_token_count: min_token_count.min(127),
        }
    }

    /// `canBeIgnoredFor`: the sentence cannot possibly match.
    pub fn can_be_ignored_for(&self, sentence: &Sentence) -> bool {
        if sentence.len() < self.min_token_count {
            return true;
        }
        self.token_hints.iter().any(|h| h.can_be_ignored_for(sentence))
    }
}

/// `PatternTokenMatcher`: a pattern element plus its per-attempt state.
struct TokMatcher<'r> {
    base: &'r PatternToken,
    /// Identity of the element in the rule (the same element can occupy
    /// several positions when `min` > 1).
    key: usize,
    compiled: Option<PatternToken>,
    and_group: Vec<TokMatcher<'r>>,
    and_group_check: Vec<bool>,
}

impl<'r> TokMatcher<'r> {
    fn new(base: &'r PatternToken, key: usize) -> TokMatcher<'r> {
        let and_group = base.and_group.iter().map(|t| TokMatcher::new(t, 0)).collect();
        TokMatcher { base, key, compiled: None, and_group, and_group_check: Vec::new() }
    }

    #[inline]
    fn token(&self) -> &PatternToken {
        self.compiled.as_ref().unwrap_or(self.base)
    }

    fn resolve_reference(&mut self, first_match_token: i64, tokens: &[&Readings], lang: &Lang) {
        if let Some(reference) = &self.base.token_reference {
            let ref_pos = first_match_token + reference.token_ref as i64;
            if ref_pos >= 0 && (ref_pos as usize) < tokens.len() {
                self.compiled = Some(self.base.compile(tokens[ref_pos as usize], lang));
            }
        }
    }

    fn is_matched(&mut self, token: &Reading) -> bool {
        let matched = self.token().is_matched(token);
        if self.base.has_and_group() {
            if let Some(first) = self.and_group_check.first_mut() {
                *first |= matched;
            }
        }
        matched
    }

    fn prepare_and_group(&mut self, first_match_token: i64, tokens: &[&Readings], lang: &Lang) {
        if self.base.has_and_group() {
            for m in &mut self.and_group {
                m.resolve_reference(first_match_token, tokens, lang);
            }
            self.and_group_check = vec![false; self.base.and_group.len() + 1];
        }
    }

    fn add_member_and_group(&mut self, token: &Reading) {
        if self.base.has_and_group() {
            for i in 0..self.and_group.len() {
                if !self.and_group_check[i + 1] && self.and_group[i].is_matched(token) {
                    self.and_group_check[i + 1] = true;
                }
            }
        }
    }

    fn check_and_group(&self, previous: bool) -> bool {
        if self.base.has_and_group() {
            self.and_group_check.iter().all(|&b| b)
        } else {
            previous
        }
    }
}

/// One successful match of the pattern.
#[derive(Clone, Debug)]
pub struct PatternMatch {
    /// For each pattern element, the number of sentence tokens it consumed
    /// (skipped tokens included); 0 for an omitted optional element.
    pub token_positions: Vec<usize>,
    pub first_match_token: usize,
    pub last_match_token: usize,
    /// Token range inside `<marker>`, when one of the matched elements is.
    pub first_marker_match_token: Option<usize>,
    pub last_marker_match_token: Option<usize>,
    /// Tokens reduced to their unified readings (`<unify>` in disambiguation).
    pub unified_tokens: Option<Vec<Readings>>,
}

/// Runs a [`TokenPattern`] over the tokens of a sentence.
pub struct Performer<'r> {
    pattern: &'r TokenPattern,
    lang: &'r Lang,
    unifier_config: &'r UnifierConfiguration,
    matchers: Vec<TokMatcher<'r>>,
    min_occur_correction: usize,
    prev_matched: bool,
    /// Grammar rules do not match on tokens an antipattern immunized.
    skip_immunized: bool,
    to_unify: HashMap<usize, Vec<Vec<Reading>>>,
    neutral_readings: HashMap<usize, Vec<Readings>>,
    unified_tokens: Option<Vec<Readings>>,
    /// `translateElementNo` for rules built from phrases.
    element_no: Option<&'r [usize]>,
}

impl<'r> Performer<'r> {
    pub fn new(
        pattern: &'r TokenPattern,
        lang: &'r Lang,
        unifier_config: &'r UnifierConfiguration,
        skip_immunized: bool,
        element_no: Option<&'r [usize]>,
    ) -> Performer<'r> {
        let matchers = pattern.tokens.iter().map(|t| TokMatcher::new(t, Arc::as_ptr(t) as usize)).collect();
        let min_occur_correction = pattern.tokens.iter().filter(|t| t.min_occurrence() == 0).count();
        Performer {
            pattern,
            lang,
            unifier_config,
            matchers,
            min_occur_correction,
            prev_matched: false,
            skip_immunized,
            to_unify: HashMap::new(),
            neutral_readings: HashMap::new(),
            unified_tokens: None,
            element_no,
        }
    }

    fn translate_element_no(&self, i: i32) -> i32 {
        match self.element_no {
            Some(list) if i >= 0 => list.iter().take(i as usize).sum::<usize>() as i32,
            _ => i,
        }
    }

    /// The start positions `doMatch` tries: all of them, or only those the
    /// anchor hint allows. `use_anchor` is false when the tokens are not those
    /// the sentence indexes describe (pre-disambiguation tags).
    pub fn start_indices(&self, sentence: &Sentence, token_count: usize, use_anchor: bool) -> Vec<usize> {
        let pattern_size = self.matchers.len();
        let limit = if self.pattern.sent_start {
            1
        } else {
            token_count.saturating_sub(pattern_size).saturating_add(1) + self.min_occur_correction
        };
        match (&self.pattern.anchor_hint, use_anchor) {
            (Some(anchor), true) => anchor
                .possible_indices(sentence)
                .into_iter()
                .filter(|&index| index >= anchor.token_index && index - anchor.token_index < limit)
                .map(|index| index - anchor.token_index)
                .collect(),
            _ => (0..limit).collect(),
        }
    }

    /// `doMatch`: report every match of the pattern in `tokens`.
    pub fn do_match(&mut self, sentence: &Sentence, tokens: &[&Readings], use_anchor: bool, consumer: &mut dyn FnMut(PatternMatch)) {
        for i in self.start_indices(sentence, tokens.len(), use_anchor) {
            if let Some(m) = self.match_from(i, tokens) {
                consumer(m);
            }
        }
    }

    /// `matchFrom`: the match starting at `start_index`, if any.
    pub fn match_from(&mut self, start_index: usize, tokens: &[&Readings]) -> Option<PatternMatch> {
        let pattern_size = self.matchers.len();
        let mut token_positions = vec![0usize; pattern_size];
        let n = tokens.len() as i64;
        let mut skip_shift_total: i64 = 0;
        let mut all_elements_match = false;
        self.unified_tokens = None;
        let mut matching_tokens = 0usize;
        let mut first_match_token: i64 = -1;
        let mut last_match_token: i64 = -1;
        let mut first_marker_match_token: i64 = -1;
        let mut last_marker_match_token: i64 = -1;
        let mut prev_skip_next: i64 = 0;
        if self.pattern.test_unification {
            self.to_unify.clear();
            self.neutral_readings.clear();
        }
        let mut min_occur_skip: i64 = 0;
        for k in 0..pattern_size {
            self.matchers[k].resolve_reference(first_match_token, tokens, self.lang);
            let next_pos = start_index as i64 + k as i64 + skip_shift_total - min_occur_skip;
            self.prev_matched = false;
            if prev_skip_next + next_pos >= n || prev_skip_next < 0 {
                // up to the end of the sentence
                prev_skip_next = n - (next_pos + 1);
            }
            let max_tok = (next_pos + prev_skip_next).min(n - (pattern_size as i64 - k as i64) + self.min_occur_correction as i64);
            let mut m = next_pos;
            while m <= max_tok {
                if m < 0 || m >= n {
                    all_elements_match = false;
                    break;
                }
                all_elements_match = self.test_all_readings(tokens, k, k.checked_sub(1), m as usize, first_match_token, prev_skip_next);
                if self.matchers[k].base.min_occurrence() == 0 {
                    let mut found_next = false;
                    for k2 in k + 1..pattern_size {
                        let next_element_match = self.test_all_readings(tokens, k2, Some(k), m as usize, first_match_token, prev_skip_next);
                        if next_element_match {
                            all_elements_match = true;
                            min_occur_skip += 1;
                            if matching_tokens < token_positions.len() {
                                token_positions[matching_tokens] = 0;
                            }
                            matching_tokens += 1;
                            found_next = true;
                            break;
                        } else if self.matchers[k2].base.min_occurrence() > 0 {
                            break;
                        }
                    }
                    if found_next {
                        break;
                    }
                }
                if all_elements_match {
                    let skip_for_max = self.skip_max_tokens(tokens, k, k.checked_sub(1), first_match_token, prev_skip_next, m as usize, pattern_size - k - 1) as i64;
                    last_match_token = m + skip_for_max;
                    let skip_shift = last_match_token - next_pos;
                    if matching_tokens < token_positions.len() {
                        token_positions[matching_tokens] = (skip_shift + 1) as usize;
                    }
                    matching_tokens += 1;
                    prev_skip_next = self.translate_element_no(self.matchers[k].base.skip) as i64;
                    skip_shift_total += skip_shift;
                    if first_match_token == -1 {
                        first_match_token = last_match_token - skip_for_max;
                    }
                    if first_marker_match_token == -1 && self.matchers[k].base.inside_marker {
                        first_marker_match_token = last_match_token - skip_for_max;
                    }
                    if self.matchers[k].base.inside_marker {
                        last_marker_match_token = last_match_token;
                    }
                    break;
                }
                m += 1;
            }
            if !all_elements_match {
                break;
            }
        }
        if all_elements_match && matching_tokens == pattern_size && self.test_unification() && first_match_token >= 0 {
            return Some(PatternMatch {
                token_positions,
                first_match_token: first_match_token as usize,
                last_match_token: last_match_token as usize,
                first_marker_match_token: (first_marker_match_token >= 0).then_some(first_marker_match_token as usize),
                last_marker_match_token: (last_marker_match_token >= 0).then_some(last_marker_match_token as usize),
                unified_tokens: self.unified_tokens.take(),
            });
        }
        None
    }

    /// `testAllReadings`: does pattern element `k` accept `tokens[token_no]`?
    fn test_all_readings(&mut self, tokens: &[&Readings], k: usize, prev: Option<usize>, token_no: usize, first_match_token: i64, prev_skip_next: i64) -> bool {
        let token = tokens[token_no];
        if self.skip_immunized && token.immunized {
            return false;
        }
        let mut any_matched = false;
        let number_of_readings = token.readings.len();
        self.matchers[k].prepare_and_group(first_match_token, tokens, self.lang);
        let collect = self.pattern.test_unification;
        let mut readings_to_unify: Vec<Reading> = Vec::new();
        for i in 0..number_of_readings {
            let match_token = &token.readings[i];
            if !self.prev_matched && prev_skip_next > 0 {
                if let Some(p) = prev {
                    self.prev_matched = self.matchers[p].token().is_matched_by_scope_next_exception(match_token);
                }
            }
            if !self.prev_matched && prev_skip_next == 0 && token_no + 2 <= tokens.len() {
                self.prev_matched = self.matchers[k].token().is_matched_by_scope_next_exception(&tokens[token_no + 1].readings[0]);
            }
            if self.prev_matched {
                return false;
            }
            let mut reading_tested = false;
            let mut reading_matches = false;
            if !any_matched {
                reading_matches = self.matchers[k].is_matched(match_token);
                any_matched = reading_matches;
                reading_tested = true;
            }
            if !any_matched && prev.is_none_or(|p| !self.matchers[p].base.has_current_or_next_exceptions()) {
                let elem = self.matchers[k].base;
                if elem.pos_tag().is_none() {
                    if elem.inflected {
                        if token.has_same_lemmas {
                            return false;
                        }
                    } else {
                        return false;
                    }
                } else if !elem.pos_negation() && !token.is_tagged() {
                    return false;
                }
            }
            if self.pattern.groups_or_unification {
                if !reading_tested {
                    reading_matches = self.matchers[k].is_matched(match_token);
                }
                let is_last_reading = i + 1 == number_of_readings;
                let elem = self.matchers[k].base;
                if reading_matches && collect && elem.is_unified() && !elem.unification_neutral {
                    readings_to_unify.push(match_token.clone());
                }
                // testAndGroup
                self.matchers[k].add_member_and_group(match_token);
                let and_ok = !is_last_reading || self.matchers[k].check_and_group(true);
                any_matched &= and_ok;
            }
        }
        if any_matched {
            for r in &token.readings {
                if self.matchers[k].token().is_exception_matched_completely(r) {
                    return false;
                }
            }
            if token_no > 0 && self.matchers[k].token().has_previous_exception() && self.matchers[k].token().is_matched_by_previous_exception(tokens[token_no - 1]) {
                return false;
            }
            if self.matchers[k].base.unification_neutral && collect {
                self.neutral_readings.entry(self.matchers[k].key).or_default().push(token.clone());
            }
        }
        let elem = self.matchers[k].base;
        if let Some(chunk) = &elem.chunk_tag {
            any_matched &= chunk.matches(token) != elem.negation;
        }
        for e in &elem.and_group {
            if let Some(chunk) = &e.chunk_tag {
                any_matched &= token.chunk_tags.iter().any(|c| **c == *chunk.tag) != e.negation;
            }
        }
        if any_matched && collect && !readings_to_unify.is_empty() {
            self.to_unify.entry(self.matchers[k].key).or_default().push(readings_to_unify);
        }
        any_matched
    }

    fn test_unification(&mut self) -> bool {
        if !self.pattern.test_unification {
            return true;
        }
        let mut unifier = self.unifier_config.create_unifier();
        for mi in 0..self.matchers.len() {
            let token = self.matchers[mi].base;
            let key = self.matchers[mi].key;
            if let Some(neutral) = self.neutral_readings.get(&key) {
                for atr in neutral {
                    unifier.add_neutral_element(atr);
                }
                continue;
            }
            let Some(reading_sets) = self.to_unify.get(&key) else { continue };
            let Some(features) = &token.unification_features else { continue };
            let last = reading_sets.len() - 1;
            for (si, readings) in reading_sets.iter().enumerate() {
                let mut any_matched = false;
                for (i, r) in readings.iter().enumerate() {
                    any_matched |= unifier.is_unified(r, features, i == readings.len() - 1);
                }
                if token.uni_negation && any_matched {
                    return false;
                }
                if token.last_in_unification && si == last {
                    if !any_matched && !token.uni_negation {
                        return false;
                    }
                    if self.pattern.get_unified {
                        self.unified_tokens = unifier.final_unified();
                    }
                    unifier.reset();
                }
            }
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn skip_max_tokens(&mut self, tokens: &[&Readings], k: usize, prev: Option<usize>, first_match_token: i64, prev_skip_next: i64, m: usize, remaining_elems: usize) -> usize {
        let mut max_skip = 0;
        let max = self.matchers[k].base.max_occurrence;
        let max_occurrences: usize = if max == -1 { usize::MAX } else { max.max(0) as usize };
        let mut j = 1;
        while j < max_occurrences && m + j + remaining_elems < tokens.len() {
            if self.test_all_readings(tokens, k, prev, m + j, first_match_token, prev_skip_next) {
                max_skip += 1;
            } else {
                break;
            }
            j += 1;
        }
        max_skip
    }
}

#[cfg(test)]
mod tests {
    use super::super::pattern::PosToken;
    use super::*;

    fn word(token: &str, readings: &[(&str, &str)], start: usize, space_before: bool) -> Readings {
        let rs: Vec<Reading> = if readings.is_empty() {
            vec![Reading::from_strs(token, None, None)]
        } else {
            readings.iter().map(|(lemma, tag)| Reading::from_strs(token, Some(tag), Some(lemma))).collect()
        };
        let mut t = Readings::new(rs, start);
        if space_before {
            t.set_whitespace_before(&Arc::from(" "));
        }
        t
    }

    /// "Il a manger une pomme." with LanguageTool-like readings.
    fn sentence() -> Sentence {
        let text = "Il a manger une pomme.";
        let mut tokens = vec![Readings::new(vec![Reading::from_strs("", Some("SENT_START"), None)], 0)];
        let specs: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("Il", vec![("il", "R pers suj 3 m s")]),
            ("a", vec![("avoir", "V avoir ind pres 3 s")]),
            ("manger", vec![("manger", "V inf")]),
            ("une", vec![("un", "D f s")]),
            ("pomme", vec![("pomme", "N f s")]),
            (".", vec![(".", "M fin")]),
        ];
        let mut pos = 0;
        for (i, (tok, readings)) in specs.iter().enumerate() {
            let space = i > 0 && *tok != ".";
            if space {
                tokens.push(Readings::untagged(" ", pos));
                pos += 1;
            }
            tokens.push(word(tok, readings, pos, space));
            pos += tok.len();
        }
        tokens.last_mut().unwrap().set_sent_end();
        Sentence::new(Arc::from(text), tokens, Vec::new())
    }

    fn tok(text: &str, regexp: bool, inflected: bool) -> PatternToken {
        PatternToken::from_text(text, false, regexp, inflected)
    }

    fn pos(tag: &str, regexp: bool) -> PatternToken {
        let mut t = tok("", false, false);
        t.pos = Some(Arc::new(PosToken::new(Some(tag), regexp, false)));
        t
    }

    fn run(tokens: Vec<PatternToken>) -> Vec<PatternMatch> {
        let pattern = TokenPattern::new(tokens.into_iter().map(Arc::new).collect(), false);
        let s = sentence();
        let lang = Lang::default();
        let cfg = UnifierConfiguration::default();
        let mut out = Vec::new();
        if pattern.can_be_ignored_for(&s) {
            return out;
        }
        let toks = s.non_ws_tokens();
        Performer::new(&pattern, &lang, &cfg, true, None).do_match(&s, &toks, true, &mut |m| out.push(m));
        out
    }

    #[test]
    fn inflected_auxiliary_followed_by_infinitive() {
        // <token inflected="yes">avoir</token><marker><token postag="V inf"/></marker>
        let mut aux = tok("avoir", false, true);
        aux.inside_marker = false;
        let m = run(vec![aux, pos("V inf", false)]);
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].first_match_token, m[0].last_match_token), (2, 3));
        assert_eq!((m[0].first_marker_match_token, m[0].last_marker_match_token), (Some(3), Some(3)));
        assert_eq!(m[0].token_positions, vec![1, 1]);
    }

    #[test]
    fn skip_and_optional_elements() {
        // il ... pomme, with up to 3 tokens skipped
        let mut il = tok("il", false, false);
        il.skip = 3;
        let m = run(vec![il.clone(), tok("pomme", false, false)]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].token_positions, vec![1, 4]);
        il.skip = 2;
        assert!(run(vec![il, tok("pomme", false, false)]).is_empty());
        // an optional adverb between "a" and "manger"
        let mut adv = pos("A", false);
        adv.may_be_omitted = true;
        let m = run(vec![tok("a", false, false), adv, tok("manger", false, false)]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].token_positions, vec![1, 0, 1]);
    }

    #[test]
    fn sentence_boundaries_are_readings() {
        // SENT_START is a token; SENT_END is a reading of the last token.
        let m = run(vec![pos("SENT_START", false), tok("il", false, false)]);
        assert_eq!(m.len(), 1);
        let m = run(vec![tok("pomme", false, false), pos("SENT_END", false)]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].last_match_token, 6, "the final period carries SENT_END");
    }

    #[test]
    fn exceptions_veto_a_match() {
        let mut det = pos("D .*", true);
        det.add_exception(false, false, tok("une", false, false));
        assert!(run(vec![det, pos("N .*", true)]).is_empty());
        // scope="next": the token after must not be "pomme"
        let mut det = pos("D .*", true);
        det.add_exception(true, false, tok("pomme", false, false));
        assert!(run(vec![det]).is_empty());
        // scope="previous"
        let mut noun = pos("N .*", true);
        noun.add_exception(false, true, tok("une", false, false));
        assert!(run(vec![noun]).is_empty());
    }

    #[test]
    fn hints_rule_out_sentences_and_anchor_start_positions() {
        assert!(run(vec![tok("poire", false, false)]).is_empty());
        let pattern = TokenPattern::new(vec![Arc::new(tok("poire|banane", true, false))], false);
        assert!(pattern.can_be_ignored_for(&sentence()));
        let pattern = TokenPattern::new(vec![Arc::new(tok("être", false, true))], false);
        assert!(pattern.can_be_ignored_for(&sentence()), "no form of être in the sentence");
        let pattern = TokenPattern::new(vec![Arc::new(tok("avoir", false, true))], false);
        assert!(!pattern.can_be_ignored_for(&sentence()));
    }

    #[test]
    fn repeated_elements() {
        // a noun phrase: determiner, then one to three tokens, then the period
        let mut any = tok(".*", true, false);
        any.max_occurrence = 3;
        let m = run(vec![tok("manger", false, false), any, tok(".", false, false)]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].token_positions, vec![1, 2, 1]);
    }
}
