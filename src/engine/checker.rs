//! Running the rules over analysed sentences and keeping the matches worth
//! showing: what `JLanguageTool.check` does after the analysis.
//!
//! * rules that are off by default, in a category that is off, tagged `picky`
//!   or reserved to a tone are not reported at the default level;
//! * rules with `min_prev_matches` only report from their n-th occurrence
//!   (`RepeatedPatternRuleTransformer`);
//! * of two overlapping matches of one rule group only the first is kept
//!   (`SameRuleGroupFilter`), and of two overlapping matches of different
//!   rules the one with the higher priority (`CleanOverlappingFilter`).

use std::collections::HashMap;

use super::french;
use super::fx::FxMap;
use super::rule::{PatternRule, RegexRule, RuleContext, RuleInfo, RuleMatch};
use super::token::Sentence;

/// A rule written in code rather than in the XML rule files.
pub trait NativeRule: Send + Sync {
    fn info(&self) -> &RuleInfo;

    /// Matches in one sentence, with byte positions relative to the sentence.
    fn match_sentence(&self, _rule: usize, _sentence: &Sentence, _ctx: &RuleContext) -> Vec<RuleMatch> {
        Vec::new()
    }

    /// A `TextLevelRule` sees all the sentences at once and returns byte
    /// positions relative to the text (the sentences' texts joined).
    fn is_text_level(&self) -> bool {
        false
    }

    fn match_text(&self, _rule: usize, _sentences: &[Sentence], _ctx: &RuleContext) -> Vec<RuleMatch> {
        Vec::new()
    }

    /// A rule that only lends its id to the matches of another rule
    /// (`SpecificIdRule`): it is never run.
    fn never_matches(&self) -> bool {
        false
    }
}

pub enum CheckRule {
    Pattern(PatternRule),
    Regex(RegexRule),
    Native(Box<dyn NativeRule>),
}

impl CheckRule {
    pub fn info(&self) -> &RuleInfo {
        match self {
            CheckRule::Pattern(r) => &r.info,
            CheckRule::Regex(r) => &r.info,
            CheckRule::Native(r) => r.info(),
        }
    }

    fn match_sentence(&self, index: usize, sentence: &Sentence, ctx: &RuleContext) -> Vec<RuleMatch> {
        match self {
            CheckRule::Pattern(r) => r.match_sentence(index, sentence, ctx),
            CheckRule::Regex(r) => r.match_sentence(index, sentence),
            CheckRule::Native(r) => r.match_sentence(index, sentence, ctx),
        }
    }
}

/// A match in the checked text. `from`/`to` are UTF-16 offsets, as in
/// LanguageTool (its filters compare lengths in these units).
#[derive(Clone, Debug)]
pub struct Found {
    pub rule: usize,
    pub from: usize,
    pub to: usize,
    pub from_byte: usize,
    pub to_byte: usize,
    pub message: String,
    pub short_message: String,
    pub suggestions: Vec<String>,
    pub original: String,
}

/// How to walk the text-level rules, in LanguageTool's rule order.
enum TextRule {
    Native(usize),
    /// Rules of one group that share `min_prev_matches`.
    Repeated(Vec<usize>),
}

pub struct Checker {
    pub rules: Vec<CheckRule>,
    pub ctx: RuleContext,
    /// Reported at the default level.
    active: Vec<bool>,
    priority: Vec<i32>,
    picky: Vec<bool>,
    text_rules: Vec<TextRule>,
    sentence_rules: Vec<usize>,
    index: RuleIndex,
}

/// Which sentence rules can match a sentence at all (`RuleSet`): a rule
/// whose pattern requires one of a few words or lemmas is only tried on the
/// sentences that contain one of them.
#[derive(Default)]
struct RuleIndex {
    /// Positions in `sentence_rules` of the rules without such a requirement.
    unhinted: Vec<u32>,
    by_token: FxMap<Box<str>, Vec<u32>>,
    by_lemma: FxMap<Box<str>, Vec<u32>>,
}

impl RuleIndex {
    fn build(rules: &[CheckRule], sentence_rules: &[usize]) -> RuleIndex {
        let mut index = RuleIndex::default();
        for (position, &i) in sentence_rules.iter().enumerate() {
            let position = position as u32;
            let hint = match &rules[i] {
                // Any hint is a necessary condition: take the most selective.
                CheckRule::Pattern(r) => r.rule.pattern.token_hints.iter().min_by_key(|h| h.lower_case_values.len()),
                _ => None,
            };
            match hint {
                Some(h) => {
                    let map = if h.inflected { &mut index.by_lemma } else { &mut index.by_token };
                    for value in &h.lower_case_values {
                        map.entry(Box::from(value.as_str())).or_default().push(position);
                    }
                }
                None => index.unhinted.push(position),
            }
        }
        index
    }

    /// Positions in `sentence_rules` of the rules worth trying, in order.
    fn candidates(&self, sentence: &Sentence, n_rules: usize) -> Vec<u32> {
        let mut wanted = vec![false; n_rules];
        for &p in &self.unhinted {
            wanted[p as usize] = true;
        }
        for key in sentence.token_offsets.keys() {
            if let Some(list) = self.by_token.get(key.as_str()) {
                for &p in list {
                    wanted[p as usize] = true;
                }
            }
        }
        for key in sentence.lemma_offsets.keys() {
            if let Some(list) = self.by_lemma.get(key.as_str()) {
                for &p in list {
                    wanted[p as usize] = true;
                }
            }
        }
        (0..n_rules as u32).filter(|&p| wanted[p as usize]).collect()
    }
}

/// `JLanguageTool.ignoreRule` with no user choice, then
/// `isRuleActiveForLevelAndToneTags` at the default level.
pub fn is_active_by_default(info: &RuleInfo) -> bool {
    if info.category_default_off || info.default_off || info.default_temp_off {
        return false;
    }
    if info.tags.iter().any(|t| t == "picky") {
        return false;
    }
    !(info.goal_specific && !info.tone_tags.is_empty())
}

impl Checker {
    /// `rules` in LanguageTool's order: native rules, then the rule files.
    /// The rules LanguageTool runs at its default level are active.
    pub fn new(rules: Vec<CheckRule>, ctx: RuleContext) -> Checker {
        let priority: Vec<i32> = rules.iter().map(|r| french::rule_priority(r.info())).collect();
        let picky: Vec<bool> = rules.iter().map(|r| r.info().tags.iter().any(|t| t == "picky")).collect();
        let mut checker =
            Checker { rules, ctx, active: Vec::new(), priority, picky, text_rules: Vec::new(), sentence_rules: Vec::new(), index: RuleIndex::default() };
        checker.select(|_, by_default| by_default);
        checker
    }

    /// Choose the rules that report: `keep(rule, active by default)`.
    pub fn select(&mut self, keep: impl Fn(&RuleInfo, bool) -> bool) {
        let rules = &self.rules;
        let active: Vec<bool> = rules.iter().map(|r| keep(r.info(), is_active_by_default(r.info()))).collect();
        let mut text_rules: Vec<TextRule> = Vec::new();
        let mut repeated: Vec<(String, Vec<usize>)> = Vec::new();
        let mut group_of: HashMap<String, usize> = HashMap::new();
        let mut sentence_rules: Vec<usize> = Vec::new();
        for (i, rule) in rules.iter().enumerate() {
            match rule {
                CheckRule::Native(r) => {
                    if !active[i] || r.never_matches() {
                        continue;
                    }
                    if r.is_text_level() {
                        text_rules.push(TextRule::Native(i));
                    } else {
                        sentence_rules.push(i);
                    }
                }
                _ => {
                    if rule.info().min_prev_matches > 0 {
                        // A repeated rule runs even when off: its matches
                        // count, they are only not reported.
                        let id = rule.info().id.clone();
                        let g = *group_of.entry(id.clone()).or_insert_with(|| {
                            repeated.push((id, Vec::new()));
                            repeated.len() - 1
                        });
                        repeated[g].1.push(i);
                    } else if active[i] {
                        sentence_rules.push(i);
                    }
                }
            }
        }
        text_rules.extend(repeated.into_iter().map(|(_, members)| TextRule::Repeated(members)));
        self.index = RuleIndex::build(rules, &sentence_rules);
        self.active = active;
        self.text_rules = text_rules;
        self.sentence_rules = sentence_rules;
    }

    pub fn info(&self, rule: usize) -> &RuleInfo {
        self.rules[rule].info()
    }

    /// All the matches of a text given as analysed sentences, filtered as
    /// LanguageTool does at the default level.
    pub fn check(&self, sentences: &[Sentence]) -> Vec<Found> {
        // Sentence start offsets, in bytes and in UTF-16 units.
        let mut starts: Vec<(usize, usize)> = Vec::with_capacity(sentences.len() + 1);
        let (mut b, mut u) = (0usize, 0usize);
        for s in sentences {
            starts.push((b, u));
            b += s.text.len();
            u += s.text.encode_utf16().count();
        }
        starts.push((b, u));

        let found_in = |m: RuleMatch, si: usize| -> Found {
            let text: &str = &sentences[si].text;
            let (sb, su) = starts[si];
            let from16 = text[..m.from_pos].encode_utf16().count();
            let len16 = text[m.from_pos..m.to_pos].encode_utf16().count();
            Found {
                rule: m.rule,
                from: su + from16,
                to: su + from16 + len16,
                from_byte: sb + m.from_pos,
                to_byte: sb + m.to_pos,
                message: m.message,
                short_message: m.short_message,
                suggestions: m.suggestions,
                original: m.original_error_str,
            }
        };
        // A text-relative byte position -> (sentence, sentence-relative).
        let locate = |pos: usize| -> (usize, usize) {
            let si = match starts.binary_search_by(|(sb, _)| sb.cmp(&pos)) {
                Ok(i) => i.min(sentences.len().saturating_sub(1)),
                Err(i) => i.saturating_sub(1),
            };
            (si, pos - starts[si].0)
        };

        let mut all: Vec<Found> = Vec::new();
        for text_rule in &self.text_rules {
            match text_rule {
                TextRule::Native(i) => {
                    let CheckRule::Native(rule) = &self.rules[*i] else { continue };
                    for m in rule.match_text(*i, sentences, &self.ctx) {
                        if sentences.is_empty() {
                            continue;
                        }
                        let (si, from) = locate(m.from_pos);
                        let (sb, su) = starts[si];
                        let text: &str = &sentences[si].text;
                        // A text-level match may run over several sentences.
                        let from16 = su + text[..from.min(text.len())].encode_utf16().count();
                        let mut to16 = from16;
                        let mut pos = m.from_pos;
                        let mut sj = si;
                        while pos < m.to_pos && sj < sentences.len() {
                            let t: &str = &sentences[sj].text;
                            let a = pos - starts[sj].0;
                            let z = (m.to_pos - starts[sj].0).min(t.len());
                            to16 += t[a..z].encode_utf16().count();
                            pos = starts[sj].0 + z;
                            sj += 1;
                        }
                        let _ = sb;
                        all.push(Found {
                            rule: m.rule,
                            from: from16,
                            to: to16,
                            from_byte: m.from_pos,
                            to_byte: m.to_pos,
                            message: m.message,
                            short_message: m.short_message,
                            suggestions: m.suggestions,
                            original: m.original_error_str,
                        });
                    }
                }
                TextRule::Repeated(members) => self.repeated(members, sentences, &found_in, &mut all),
            }
        }
        for (si, sentence) in sentences.iter().enumerate() {
            let mut matches: Vec<RuleMatch> = Vec::new();
            for p in self.index.candidates(sentence, self.sentence_rules.len()) {
                let i = self.sentence_rules[p as usize];
                matches.extend(self.rules[i].match_sentence(i, sentence, &self.ctx));
            }
            if matches.is_empty() {
                continue;
            }
            let found: Vec<Found> = matches.into_iter().map(|m| found_in(m, si)).collect();
            all.extend(self.same_rule_group_filter(found));
        }
        if all.is_empty() {
            return all;
        }
        all.retain(|m| self.active[m.rule]);
        let all = self.same_rule_group_filter(all);
        self.clean_overlapping(all)
    }

    /// `RepeatedPatternRule.match`.
    fn repeated(&self, members: &[usize], sentences: &[Sentence], found_in: &dyn Fn(RuleMatch, usize) -> Found, out: &mut Vec<Found>) {
        const DEFAULT_MAX_DISTANCE_TOKENS: i32 = 60;
        let mut offset_tokens = 0usize;
        let mut prev_from_token = 0usize;
        let mut prev_matches = 0i32;
        let mut distances: Vec<i64> = Vec::new();
        for (si, s) in sentences.iter().enumerate() {
            let mut sentence_matches: Vec<RuleMatch> = Vec::new();
            for &i in members {
                sentence_matches.extend(self.rules[i].match_sentence(i, s, &self.ctx));
            }
            // SameRuleGroupFilter, on sentence positions.
            sentence_matches.sort_by_key(|m| m.from_pos);
            let mut kept: Vec<RuleMatch> = Vec::new();
            let mut i = 0;
            while i < sentence_matches.len() {
                let m = sentence_matches[i].clone();
                while i + 1 < sentence_matches.len() && {
                    let next = &sentence_matches[i + 1];
                    m.from_pos <= next.to_pos && m.to_pos >= next.from_pos && self.info(m.rule).id == self.info(next.rule).id
                } {
                    i += 1;
                }
                kept.push(m);
                i += 1;
            }
            let n_tokens = s.len();
            for m in kept {
                let mut from_token = 0usize;
                while from_token < n_tokens && s.tok(from_token).start_pos < m.from_pos {
                    from_token += 1;
                }
                from_token += offset_tokens;
                let info = self.info(m.rule);
                let mut max_distance = info.distance_tokens;
                if max_distance < 1 {
                    max_distance = DEFAULT_MAX_DISTANCE_TOKENS * info.min_prev_matches;
                }
                distances.push(from_token as i64 - prev_from_token as i64);
                let min_prev = info.min_prev_matches;
                let distance_ok = {
                    let size = distances.len();
                    let mut distance = 0i64;
                    let mut k = 0usize;
                    while (k as i32) < min_prev && k < size {
                        distance += distances[size - 1 - k];
                        k += 1;
                    }
                    distance < max_distance as i64
                };
                if prev_matches >= min_prev && distance_ok {
                    out.push(found_in(m, si));
                }
                prev_from_token = from_token;
                prev_matches += 1;
            }
            offset_tokens += n_tokens.saturating_sub(1);
        }
    }

    /// `SameRuleGroupFilter`: sorted by start, a match swallows the following
    /// ones of its rule group that touch it.
    fn same_rule_group_filter(&self, mut matches: Vec<Found>) -> Vec<Found> {
        matches.sort_by_key(|m| m.from);
        let mut out: Vec<Found> = Vec::with_capacity(matches.len());
        let mut i = 0;
        while i < matches.len() {
            let m = &matches[i];
            let mut j = i;
            while j + 1 < matches.len() && {
                let next = &matches[j + 1];
                m.from <= next.to && m.to >= next.from && self.info(m.rule).id == self.info(next.rule).id
            } {
                j += 1;
            }
            out.push(matches[i].clone());
            i = j + 1;
        }
        out
    }

    /// `CleanOverlappingFilter`.
    fn clean_overlapping(&self, matches: Vec<Found>) -> Vec<Found> {
        const NEGATIVE_CONSTANT: i64 = i32::MIN as i64 + 10000;
        let mut clean: Vec<Found> = Vec::with_capacity(matches.len());
        let mut prev: Option<Found> = None;
        for m in matches {
            let Some(p) = prev.as_ref() else {
                prev = Some(m);
                continue;
            };
            let mut duplicate_suggestion = false;
            if let (Some(suggestion), Some(prev_suggestion)) = (m.suggestions.first(), p.suggestions.first()) {
                if m.from == p.to && prev_suggestion.ends_with(',') && suggestion.starts_with(", ") {
                    duplicate_suggestion = true;
                }
                let inner_space = |s: &str| s.find(' ').is_some_and(|i| i > 0);
                if inner_space(suggestion) && inner_space(prev_suggestion) && m.from == p.to + 1 {
                    let parts: Vec<&str> = java_split_space(suggestion);
                    let parts_prev: Vec<&str> = java_split_space(prev_suggestion);
                    if parts_prev.len() > 1 && parts.len() > 1 && parts_prev[1] == parts[0] {
                        duplicate_suggestion = true;
                    }
                }
            }
            if m.from >= p.to && !duplicate_suggestion {
                clean.push(prev.take().unwrap());
                prev = Some(m);
                continue;
            }
            let mut current = self.priority[m.rule] as i64;
            if self.picky[m.rule] {
                current += NEGATIVE_CONSTANT;
            }
            let mut previous = self.priority[p.rule] as i64;
            if self.picky[p.rule] {
                previous += NEGATIVE_CONSTANT;
            }
            if current == previous {
                current = (m.to - m.from) as i64;
                previous = (p.to - p.from) as i64;
            }
            if current == previous {
                current += 1; // the later one wins
            }
            if current > previous {
                prev = Some(m);
            }
        }
        if let Some(p) = prev {
            clean.push(p);
        }
        clean
    }
}

/// Java's `String.split(" ")`: trailing empty strings are dropped.
fn java_split_space(s: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = s.split(' ').collect();
    while parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    parts
}
