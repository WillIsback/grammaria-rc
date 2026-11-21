//! Grammar rules and their matches.
//!
//! Port of `PatternRule`, `PatternRuleMatcher` (message and suggestion
//! formatting), `RegexPatternRule`, `RuleMatch`, `RuleFilterEvaluator` and
//! `RuleWithMaxFilter` from `org.languagetool.rules` (LanguageTool,
//! LGPL-2.1-or-later).

use std::collections::HashMap;
use std::sync::Arc;

use super::disambig::DisambigRule;
use super::pattern::{convert_case, Lang, Match, MatchState, MISTAKE};
use super::performer::{PatternMatch, Performer, TokenPattern};
use super::strtools;
use super::token::{Readings, Sentence};
use super::unifier::UnifierConfiguration;
use crate::re::Regex;

pub const SUGGESTION_START_TAG: &str = "<suggestion>";
pub const SUGGESTION_END_TAG: &str = "</suggestion>";
pub const PLEASE_SPELL_ME: &str = "<pleasespellme/>";

/// What the engine shares with every rule.
pub struct RuleContext {
    pub lang: Lang,
    /// `<unification>` definitions of the grammar files.
    pub unifier: UnifierConfiguration,
    /// `<unification>` definitions of the disambiguation file.
    pub disambig_unifier: UnifierConfiguration,
}

/// Rule metadata (`Rule` / `AbstractPatternRule`).
#[derive(Clone, Debug, Default)]
pub struct RuleInfo {
    pub id: String,
    pub sub_id: Option<String>,
    pub description: String,
    pub category_id: String,
    pub category_name: String,
    /// The category itself is off by default.
    pub category_default_off: bool,
    /// ITS issue type (`misspelling`, `typographical`, `style`...);
    /// `uncategorized` when the rule file gives none.
    pub issue_type: String,
    pub default_off: bool,
    pub default_temp_off: bool,
    pub tags: Vec<String>,
    /// Tone tags (`tone_tags`); a goal-specific rule that has some only runs
    /// when its tone is asked for.
    pub tone_tags: Vec<String>,
    pub goal_specific: bool,
    pub url: Option<String>,
    /// `prio` attribute of the rule, group or category (0 when absent).
    pub priority: i32,
    pub source_file: String,
    pub min_prev_matches: i32,
    pub distance_tokens: i32,
    pub is_style_file: bool,
}

impl RuleInfo {
    /// `getFullId`: `ID[subId]`.
    pub fn full_id(&self) -> String {
        match &self.sub_id {
            Some(s) => format!("{}[{}]", self.id, s),
            None => self.id.clone(),
        }
    }
}

/// A reported problem (`RuleMatch`). Positions are byte offsets, relative to
/// the sentence until the checker shifts them to the document.
#[derive(Clone, Debug)]
pub struct RuleMatch {
    /// Index of the rule in the rule set (`usize::MAX` for filter probes).
    pub rule: usize,
    pub from_pos: usize,
    pub to_pos: usize,
    pub pattern_from: usize,
    pub pattern_to: usize,
    pub message: String,
    pub short_message: String,
    pub suggestions: Vec<String>,
    pub original_error_str: String,
}

impl RuleMatch {
    /// The `RuleMatch` constructor: also extracts the `<suggestion>`s of the
    /// message and adjusts their case to the matched text.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rule: usize,
        sentence_text: &str,
        from_pos: usize,
        to_pos: usize,
        pattern_from: usize,
        pattern_to: usize,
        message: String,
        short_message: String,
        start_with_uppercase: bool,
        is_all_uppercase: bool,
        suggestions_out_msg: &str,
    ) -> RuleMatch {
        let original_error_str = sentence_text
            .get(from_pos..to_pos)
            .unwrap_or("")
            .to_string();
        let mut suggestions: Vec<String> = Vec::new();
        let suggestion = format!("{message}{suggestions_out_msg}");
        let mut pos = suggestion.find(SUGGESTION_START_TAG);
        while let Some(p) = pos {
            let Some(end) = suggestion[p..].find(SUGGESTION_END_TAG).map(|e| e + p) else {
                break;
            };
            let mut replacement = suggestion[p + SUGGESTION_START_TAG.len()..end].to_string();
            let next_from = end + SUGGESTION_END_TAG.len();
            pos = suggestion[next_from..]
                .find(SUGGESTION_START_TAG)
                .map(|x| x + next_from);
            if replacement.contains(MISTAKE) {
                continue;
            }
            if is_all_uppercase
                && !(strtools::is_mixed_case(&replacement) && !replacement.contains(' '))
            {
                if original_error_str != replacement.to_uppercase() {
                    replacement = replacement.to_uppercase();
                }
            } else if start_with_uppercase {
                replacement = strtools::uppercase_first_char(&replacement);
            }
            if !suggestions.contains(&replacement) {
                suggestions.push(replacement);
            }
        }
        RuleMatch {
            rule,
            from_pos,
            to_pos,
            pattern_from,
            pattern_to,
            message,
            short_message,
            suggestions,
            original_error_str,
        }
    }
}

/// A Java filter attached to a rule (`<filter class=... args=...>`): accepts,
/// rewrites or rejects a match.
pub trait RuleFilter: Send + Sync {
    /// `acceptRuleMatch`: `None` rejects the match. `pattern_tokens` are the
    /// sentence tokens the pattern matched; `token_positions` as in
    /// [`PatternMatch`].
    fn accept(
        &self,
        m: RuleMatch,
        args: &HashMap<String, String>,
        pattern_token_pos: usize,
        pattern_tokens: &[&Readings],
        token_positions: &[usize],
        sentence: &Sentence,
        ctx: &RuleContext,
    ) -> Option<RuleMatch>;
}

/// `RuleFilter.getSkipCorrectedReference`.
pub fn skip_corrected_reference(token_positions: &[usize], ref_number: i64) -> i64 {
    if ref_number < 0 {
        return ref_number;
    }
    let mut corrected = 0i64;
    for (i, &p) in token_positions.iter().enumerate() {
        if i as i64 >= ref_number {
            break;
        }
        corrected += p as i64;
    }
    corrected - 1
}

/// `RuleFilterEvaluator.getResolvedArguments`: `key:value` pairs, where a
/// value `\N` stands for the text of the N-th matched pattern element.
pub fn resolve_filter_arguments(
    filter_args: &str,
    pattern_tokens: &[&Readings],
    token_positions: &[usize],
) -> Option<HashMap<String, String>> {
    let mut result = HashMap::new();
    for arg in filter_args.split_whitespace() {
        let delim = arg.find(':')?;
        let key = &arg[..delim];
        let val = &arg[delim + 1..];
        if let Some(num) = val.strip_prefix('\\') {
            let ref_number: usize = num.replace('\\', "").parse().ok()?;
            if ref_number > token_positions.len() {
                return None;
            }
            let corrected = skip_corrected_reference(token_positions, ref_number as i64);
            if corrected < 0 || corrected as usize >= pattern_tokens.len() {
                return None;
            }
            result.insert(
                key.to_string(),
                pattern_tokens[corrected as usize].token.to_string(),
            );
        } else {
            result.insert(key.to_string(), val.to_string());
        }
    }
    Some(result)
}

/// How a rule's filter is available.
#[derive(Clone)]
pub enum FilterRef {
    Native(Arc<dyn RuleFilter>),
    /// The Java class has no native counterpart yet: the rule cannot be
    /// evaluated faithfully and stays silent.
    Unsupported(String),
}

/// The parts shared by grammar rules and antipatterns/disambiguation rules
/// when they are matched as patterns (`AbstractTokenBasedRule` seen by
/// `PatternRuleMatcher`).
#[derive(Clone)]
pub struct TokenRule {
    pub pattern: TokenPattern,
    pub message: String,
    pub short_message: String,
    pub suggestions_out_msg: String,
    pub suggestion_matches: Vec<Match>,
    pub suggestion_matches_out_msg: Vec<Match>,
    pub start_position_correction: i32,
    pub end_position_correction: i32,
    pub filter: Option<FilterRef>,
    pub filter_args: Option<String>,
    /// `getElementNo` / `useList`: element counts of phrase references.
    pub element_no: Vec<usize>,
    pub use_list: bool,
    /// `raw_pos="yes"`: match the tags the tagger produced.
    pub raw_pos: bool,
    pub adjust_suggestion_case: bool,
}

impl TokenRule {
    pub fn new(pattern: TokenPattern) -> TokenRule {
        TokenRule {
            pattern,
            message: String::new(),
            short_message: String::new(),
            suggestions_out_msg: String::new(),
            suggestion_matches: Vec::new(),
            suggestion_matches_out_msg: Vec::new(),
            start_position_correction: 0,
            end_position_correction: 0,
            filter: None,
            filter_args: None,
            element_no: Vec::new(),
            use_list: false,
            raw_pos: false,
            adjust_suggestion_case: true,
        }
    }

    /// The `PatternRule` constructor's phrase bookkeeping (`elementNo`).
    pub fn compute_element_no(&mut self) {
        let mut element_no = Vec::new();
        let mut prev_name = String::new();
        let mut cnt = 0usize;
        let mut loop_cnt = 0usize;
        let mut use_list = false;
        let total = self.pattern.tokens.len();
        for t in &self.pattern.tokens {
            if let Some(name) = &t.phrase_name {
                let mut cur_name = name.to_string();
                if prev_name.is_empty() || prev_name == cur_name {
                    cnt += 1;
                    use_list = true;
                } else {
                    element_no.push(cnt);
                    cur_name = String::new();
                    cnt = 0;
                }
                prev_name = cur_name;
                loop_cnt += 1;
                if loop_cnt == total && !prev_name.is_empty() {
                    element_no.push(cnt);
                }
            } else {
                if cnt > 0 {
                    element_no.push(cnt);
                }
                element_no.push(1);
                loop_cnt += 1;
            }
        }
        self.element_no = element_no;
        self.use_list = use_list;
    }

    /// `PatternRuleMatcher.match`: all matches of the rule in the sentence,
    /// after the per-rule inclusion filter.
    pub fn match_sentence(
        &self,
        rule_index: usize,
        sentence: &Sentence,
        ctx: &RuleContext,
        unifier: &UnifierConfiguration,
    ) -> Vec<RuleMatch> {
        let tokens: Vec<&Readings> = if self.raw_pos {
            sentence.pre_disambig_non_ws_tokens()
        } else {
            sentence.non_ws_tokens()
        };
        let element_no = if self.use_list {
            Some(self.element_no.as_slice())
        } else {
            None
        };
        let mut performer = Performer::new(&self.pattern, &ctx.lang, unifier, true, element_no);
        let mut matches: Vec<RuleMatch> = Vec::new();
        performer.do_match(sentence, &tokens, !self.raw_pos, &mut |pm| {
            if let Some(m) = self.create_rule_match(rule_index, &pm, &tokens, sentence, ctx) {
                matches.push(m);
            }
        });
        rule_with_max_filter(matches)
    }

    fn match_preserves_case(&self, suggestion_matches: &[Match], msg: &str) -> bool {
        if !suggestion_matches.is_empty() {
            let mut sug_start = msg
                .find(SUGGESTION_START_TAG)
                .map(|p| p as i64)
                .unwrap_or(-1)
                + SUGGESTION_START_TAG.len() as i64;
            if msg.contains(PLEASE_SPELL_ME) {
                sug_start += PLEASE_SPELL_ME.len() as i64;
            }
            let at = msg.as_bytes().get(sug_start as usize).copied();
            for m in suggestion_matches {
                if !m.in_message_only && m.converts_case() && at == Some(b'\\') {
                    return false;
                }
            }
        }
        true
    }

    fn phrase_len(&self, i: usize) -> usize {
        if !self.use_list || i >= self.element_no.len() {
            1
        } else {
            self.element_no[i]
        }
    }

    fn concat_matches(
        &self,
        m: &Match,
        index: usize,
        token_index: usize,
        tokens: &[&Readings],
        next_token_pos: usize,
        ctx: &RuleContext,
    ) -> Vec<String> {
        let len = self.phrase_len(index);
        if len == 1 {
            let skipped = next_token_pos as i64 - token_index as i64;
            let mut st = MatchState::new(m, &ctx.lang);
            st.set_token_at(tokens, token_index.saturating_sub(1), skipped);
            st.to_final_string(true)
        } else {
            let mut lists: Vec<Vec<String>> = Vec::new();
            for i in 0..len {
                let skipped = next_token_pos as i64 - (token_index + i) as i64;
                let mut st = MatchState::new(m, &ctx.lang);
                st.set_token_at(tokens, (token_index + i).saturating_sub(1), skipped);
                lists.push(st.to_final_string(true));
            }
            combine_lists(&lists)
        }
    }

    /// `formatMatches`: replace the `\N` references of a message by the
    /// matched tokens, rewritten by the corresponding `<match>` elements.
    fn format_matches(
        &self,
        tokens: &[&Readings],
        positions: &[usize],
        first_match_tok: usize,
        error_msg: &str,
        suggestion_matches: &[Match],
        ctx: &RuleContext,
    ) -> String {
        let mut suggestion_matches: Vec<&Match> = suggestion_matches.iter().collect();
        let mut error_message = error_msg.to_string();
        let mut match_counter = 0usize;
        let mut numbers_to_matches = vec![0usize; error_msg.len() + 1];
        let mut new_way = false;
        let mut processed = 0usize;
        let is_positive = |b: Option<u8>| matches!(b, Some(b'1'..=b'9'));
        let find_from =
            |s: &str, from: usize| s.get(from..).and_then(|t| t.find('\\')).map(|p| p + from);
        let mut err_marker = find_from(&error_message, processed);
        let mut number_follows = err_marker.is_some_and(|m| {
            m + 1 < error_message.len() && is_positive(error_message.as_bytes().get(m + 1).copied())
        });
        let mut guard = 0;
        while err_marker.is_some() && number_follows {
            guard += 1;
            if guard > 10_000 {
                break;
            }
            let backslash_pos = find_from(&error_message, processed);
            if let Some(bp) = backslash_pos
                .filter(|&bp| is_positive(error_message.as_bytes().get(bp + 1).copied()))
            {
                let bytes = error_message.as_bytes();
                let mut num_len = 1;
                while bp + num_len < bytes.len() && bytes[bp + num_len].is_ascii_digit() {
                    num_len += 1;
                }
                let j: usize = error_message[bp + 1..bp + num_len]
                    .parse::<usize>()
                    .unwrap_or(1)
                    - 1;
                let mut rep_token_pos = 0usize;
                let mut next_token_pos = 0usize;
                for l in 0..=j.min(positions.len().saturating_sub(1)) {
                    if let Some(p) = positions.get(l) {
                        rep_token_pos += p;
                    }
                }
                if j + 1 < positions.len() {
                    next_token_pos = first_match_tok + rep_token_pos + positions[j + 1];
                }
                if !suggestion_matches.is_empty() {
                    if match_counter < suggestion_matches.len() {
                        if j < numbers_to_matches.len() {
                            numbers_to_matches[j] = match_counter;
                        }
                        let matches: Vec<String> = if j >= positions.len() || positions[j] != 0 {
                            self.concat_matches(
                                suggestion_matches[match_counter],
                                j,
                                first_match_tok + rep_token_pos,
                                tokens,
                                next_token_pos,
                                ctx,
                            )
                        } else {
                            vec![String::new()]
                        };
                        let left = error_message[..bp].to_string();
                        let right = error_message[bp + num_len..].to_string();
                        if matches.len() == 1 {
                            if matches[0].is_empty() {
                                error_message = concat_without_extra_space(&left, &right);
                                processed = left.len().min(error_message.len());
                            } else {
                                processed = left.len() + matches[0].len();
                                error_message = format!("{left}{}{right}", matches[0]);
                            }
                        } else {
                            error_message = format_multiple_synthesis(&matches, &left, &right);
                        }
                        match_counter += 1;
                        new_way = true;
                    } else {
                        let idx = numbers_to_matches.get(j).copied().unwrap_or(0);
                        let again = suggestion_matches[idx.min(suggestion_matches.len() - 1)];
                        suggestion_matches.push(again);
                    }
                }
                if !new_way {
                    let needle = format!("\\{}", j + 1);
                    let tok_index = (first_match_tok + rep_token_pos)
                        .saturating_sub(1)
                        .min(tokens.len() - 1);
                    let tok_text = tokens[tok_index].token.to_string();
                    let new_processed = error_message.rfind(&needle).unwrap_or(0) + tok_text.len();
                    let head = error_message[..processed].to_string();
                    let tail = error_message[processed..].replace(&needle, &tok_text);
                    error_message = format!("{head}{tail}");
                    processed = new_processed.min(error_message.len());
                }
            }
            while processed < error_message.len() && !error_message.is_char_boundary(processed) {
                processed += 1;
            }
            err_marker = find_from(&error_message, processed);
            number_follows = err_marker.is_some_and(|m| {
                m + 1 < error_message.len()
                    && is_positive(error_message.as_bytes().get(m + 1).copied())
            });
        }
        remove_suppress_misspelled(&error_message)
    }

    /// `createRuleMatch`.
    fn create_rule_match(
        &self,
        rule_index: usize,
        pm: &PatternMatch,
        tokens: &[&Readings],
        sentence: &Sentence,
        ctx: &RuleContext,
    ) -> Option<RuleMatch> {
        let positions = &pm.token_positions;
        let first = pm.first_match_token;
        let last = pm.last_match_token;
        let err_message = self.format_matches(
            tokens,
            positions,
            first,
            &self.message,
            &self.suggestion_matches,
            ctx,
        );
        let short_err_message = self.format_matches(
            tokens,
            positions,
            first,
            &self.short_message,
            &self.suggestion_matches,
            ctx,
        );
        let suggestions_out_msg = self.format_matches(
            tokens,
            positions,
            first,
            &self.suggestions_out_msg,
            &self.suggestion_matches_out_msg,
            ctx,
        );
        let mut corrected_st_pos: i64 = 0;
        if self.start_position_correction > 0 {
            for l in
                0..=(self.start_position_correction as usize).min(positions.len().saturating_sub(1))
            {
                corrected_st_pos += positions[l] as i64;
            }
            corrected_st_pos -= 1;
        }
        let mut idx = (first as i64 + corrected_st_pos).max(0) as usize;
        if idx >= tokens.len() {
            idx = tokens.len() - 1;
        }
        let mut first_obj = tokens[idx];
        let input_tokens: Vec<&str> = (idx..=last.min(tokens.len() - 1))
            .map(|i| &*tokens[i].token)
            .collect();
        let is_input_all_uppercase = is_all_uppercase_list(&input_tokens);
        let preserves = self.match_preserves_case(&self.suggestion_matches, &self.message)
            && self
                .match_preserves_case(&self.suggestion_matches_out_msg, &self.suggestions_out_msg);
        let mut is_all_uppercase = is_input_all_uppercase
            && (strtools::utf16_len(&first_obj.token.replace('\'', "")) > 1 || last > idx)
            && preserves;
        is_all_uppercase = is_all_uppercase && self.adjust_suggestion_case;
        let mut starts_with_uppercase =
            strtools::starts_with_uppercase(&first_obj.token) && preserves;
        let after = (first as i64 + corrected_st_pos + 1) as usize;
        if first_obj.is_sent_start && tokens.len() > after {
            first_obj = tokens[after];
            starts_with_uppercase = strtools::starts_with_uppercase(&first_obj.token);
        }
        starts_with_uppercase = starts_with_uppercase && self.adjust_suggestion_case;
        let first_marker = pm.first_marker_match_token.unwrap_or(first);
        let mut from_pos = tokens[first_marker].start_pos;
        let comma_suggestion = format!("{SUGGESTION_START_TAG},");
        if first_marker >= 1
            && (err_message.contains(&comma_suggestion)
                || suggestions_out_msg.contains(&comma_suggestion))
        {
            from_pos = tokens[first_marker - 1].end_pos();
        }
        let last_marker = pm.last_marker_match_token.unwrap_or(last);
        let token = tokens[last_marker.min(tokens.len() - 1)];
        let to_pos = token.end_pos();
        if from_pos < to_pos
            && !(err_message.contains(PLEASE_SPELL_ME)
                && !err_message.contains(SUGGESTION_START_TAG)
                && !suggestions_out_msg.contains(SUGGESTION_START_TAG))
        {
            let clear_msg = err_message
                .replace(PLEASE_SPELL_ME, "")
                .replace(MISTAKE, "");
            let rule_match = RuleMatch::new(
                rule_index,
                &sentence.text,
                from_pos,
                to_pos,
                tokens[first].start_pos,
                tokens[last.min(tokens.len() - 1)].end_pos(),
                clear_msg,
                short_err_message,
                starts_with_uppercase,
                is_all_uppercase,
                &suggestions_out_msg,
            );
            return match &self.filter {
                None => Some(rule_match),
                Some(FilterRef::Unsupported(_)) => None,
                Some(FilterRef::Native(filter)) => {
                    let pattern_tokens = &tokens[first..=last.min(tokens.len() - 1)];
                    let args = resolve_filter_arguments(
                        self.filter_args.as_deref().unwrap_or(""),
                        pattern_tokens,
                        positions,
                    )?;
                    filter.accept(
                        rule_match,
                        &args,
                        first,
                        pattern_tokens,
                        positions,
                        sentence,
                        ctx,
                    )
                }
            };
        }
        None
    }
}

/// `StringTools.isAllUppercase(List<String>)`.
fn is_all_uppercase_list(list: &[&str]) -> bool {
    let mut all_upper = true;
    let mut all_not_letters = true;
    for s in list {
        all_upper = all_upper && strtools::is_all_uppercase(s);
        let not_word = !s.is_empty() && s.chars().all(|c| !c.is_alphabetic());
        all_not_letters = all_not_letters && (not_word || is_punctuation_mark(s));
    }
    all_upper && !all_not_letters
}

/// `StringTools.isPunctuationMark`: one punctuation character (or apostrophe).
pub fn is_punctuation_mark(s: &str) -> bool {
    let mut it = s.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => {
            c == '\''
                || c.is_ascii_punctuation() && !"$+<=>^`|~".contains(c)
                || "«»“”‘’…–—¿¡·„‚‹›".contains(c)
        }
        _ => false,
    }
}

fn concat_without_extra_space(left: &str, right: &str) -> String {
    let ws_or_punct = right
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_whitespace() || ",:;.!?".contains(c));
    if (left.ends_with(' ') && right.starts_with(SUGGESTION_END_TAG))
        || (left.ends_with(' ') && ws_or_punct)
    {
        return format!("{}{}", &left[..left.len() - 1], right);
    }
    if left.ends_with("suggestion>") && right.starts_with(' ') {
        return format!("{left}{}", &right[1..]);
    }
    format!("{left}{right}")
}

/// `removeSuppressMisspelled`: drop the suggestions whose synthesized form is
/// not a word, then the `<pleasespellme/>` markers.
fn remove_suppress_misspelled(s: &str) -> String {
    if !s.contains(PLEASE_SPELL_ME) {
        return s.to_string();
    }
    static SUPPRESS: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = SUPPRESS.get_or_init(|| {
        Regex::new(
            "<suggestion><pleasespellme/>[^<>()]*?(\\([^<>()]*?\\)|<mistake/>)[^<>()]*?</suggestion>",
            crate::re::Flags::default(),
        )
        .expect("valid regex")
    });
    let result = re.replace_all(s, "");
    result.replace(
        &format!("{SUGGESTION_START_TAG}{PLEASE_SPELL_ME}"),
        SUGGESTION_START_TAG,
    )
}

/// `formatMultipleSynthesis`: several synthesized forms become several
/// suggestions.
fn format_multiple_synthesis(matches: &[String], left: &str, right: &str) -> String {
    let mut suggestion_left = "";
    let mut suggestion_right = "";
    let mut right_new = right;
    if let Some(s_pos) = left.rfind(SUGGESTION_START_TAG) {
        suggestion_left = &left[s_pos + SUGGESTION_START_TAG.len()..];
    }
    let error_message: String = if suggestion_left.is_empty() {
        left.to_string()
    } else {
        format!(
            "{}{}",
            &left[..left.rfind(SUGGESTION_START_TAG).unwrap()],
            SUGGESTION_START_TAG
        )
    };
    if let Some(r_pos) = right.find(SUGGESTION_END_TAG) {
        suggestion_right = &right[..r_pos];
    }
    if !suggestion_right.is_empty() {
        right_new = &right[right.find(SUGGESTION_END_TAG).unwrap()..];
    }
    let last_left_sug_end = left
        .find(SUGGESTION_END_TAG)
        .map(|p| p as i64)
        .unwrap_or(-1);
    let last_left_sug_start = left
        .rfind(SUGGESTION_START_TAG)
        .map(|p| p as i64)
        .unwrap_or(-1);
    let mut sb = error_message;
    for (z, m) in matches.iter().enumerate() {
        sb.push_str(suggestion_left);
        sb.push_str(m);
        sb.push_str(suggestion_right);
        if z < matches.len() - 1 && last_left_sug_end < last_left_sug_start {
            sb.push_str(SUGGESTION_END_TAG);
            sb.push_str(", ");
            sb.push_str(SUGGESTION_START_TAG);
        }
    }
    sb.push_str(right_new);
    sb
}

/// `StringTools.addSpace` for French: no space before `.` and `,`.
fn add_space(word: &str) -> &'static str {
    if word == "." || word == "," {
        ""
    } else {
        " "
    }
}

/// `combineLists`: every combination of the alternatives, joined by spaces.
fn combine_lists(input: &[Vec<String>]) -> Vec<String> {
    fn rec(input: &[Vec<String>], output: &mut Vec<String>, r: usize, out: &mut Vec<String>) {
        if r == input.len() {
            let mut sb = String::new();
            for k in 0..output.len() {
                sb.push_str(&output[k]);
                if k < output.len() - 1 {
                    sb.push_str(add_space(&output[k + 1]));
                }
            }
            out.push(sb);
        } else {
            for c in &input[r] {
                output[r] = c.clone();
                rec(input, output, r + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    let mut output = vec![String::new(); input.len()];
    rec(input, &mut output, 0, &mut out);
    out
}

/// `RuleWithMaxFilter`: among matches of the same rule, drop those included
/// in an earlier one.
pub fn rule_with_max_filter(mut matches: Vec<RuleMatch>) -> Vec<RuleMatch> {
    matches.sort_by_key(|m| m.from_pos);
    let includes = |a: &RuleMatch, b: &RuleMatch| a.from_pos <= b.from_pos && a.to_pos >= b.to_pos;
    let n = matches.len();
    let mut keep = vec![false; n];
    let mut i = 0;
    while i < n {
        let cur = i;
        if i + 1 < n {
            let mut next = i + 1;
            while includes(&matches[cur], &matches[next]) && i < n {
                i += 1;
                if i + 1 < n {
                    next = i + 1;
                }
            }
        }
        keep[cur] = true;
        i += 1;
    }
    let mut it = keep.into_iter();
    matches.retain(|_| it.next().unwrap_or(false));
    matches
}

/// A grammar rule made of token patterns (`PatternRule`).
pub struct PatternRule {
    pub info: RuleInfo,
    pub rule: TokenRule,
    pub anti_patterns: Vec<Arc<DisambigRule>>,
}

impl PatternRule {
    /// `PatternRule.match`: matches, re-evaluated on an immunized copy of the
    /// sentence when an antipattern applies.
    pub fn match_sentence(
        &self,
        rule_index: usize,
        sentence: &Sentence,
        ctx: &RuleContext,
    ) -> Vec<RuleMatch> {
        if self.rule.pattern.can_be_ignored_for(sentence) {
            return Vec::new();
        }
        let matches = self
            .rule
            .match_sentence(rule_index, sentence, ctx, &ctx.unifier);
        if !matches.is_empty() && !self.anti_patterns.is_empty() {
            let mut immunized: Option<Sentence> = None;
            for ap in &self.anti_patterns {
                let current = immunized.as_ref().unwrap_or(sentence);
                if let Some(next) = ap.replace(current, ctx) {
                    immunized = Some(next);
                }
            }
            if let Some(s) = immunized {
                if s.tokens.iter().any(|t| t.immunized) {
                    return self.rule.match_sentence(rule_index, &s, ctx, &ctx.unifier);
                }
            }
        }
        matches
    }
}

/// A rule matching a regular expression on the sentence text
/// (`RegexPatternRule`).
pub struct RegexRule {
    pub info: RuleInfo,
    pub pattern: Arc<Regex>,
    pub mark_group: usize,
    pub message: String,
    pub short_message: String,
    pub suggestions_out_msg: String,
    pub suggestion_matches: Vec<Match>,
    pub suggestion_matches_out_msg: Vec<Match>,
    pub filter: Option<FilterRef>,
}

/// `RegexPatternRule.MAX_SENT_LENGTH`.
const MAX_SENT_LENGTH: usize = 2000;

impl RegexRule {
    pub fn match_sentence(&self, rule_index: usize, sentence: &Sentence) -> Vec<RuleMatch> {
        let mut out = Vec::new();
        if self.filter.is_some() {
            // Regex filters are not ported: stay silent rather than over-report.
            return out;
        }
        let text: &str = &sentence.text;
        let chars: Vec<char> = text.chars().collect();
        if chars.len() > MAX_SENT_LENGTH {
            return out;
        }
        // char index -> byte offset
        let mut byte_at: Vec<usize> = Vec::with_capacity(chars.len() + 1);
        let mut b = 0;
        for c in &chars {
            byte_at.push(b);
            b += c.len_utf8();
        }
        byte_at.push(b);
        let mut start = 0usize;
        while start <= chars.len() {
            let Some(caps) = self.pattern.find_chars(&chars, start) else {
                break;
            };
            let (m_start, m_end) = caps.spans[0].unwrap();
            let group = |n: usize| caps.spans.get(n).copied().flatten();
            let Some((mark_start, mark_end)) = group(self.mark_group) else {
                start = if m_end > m_start { m_end } else { m_end + 1 };
                continue;
            };
            let text_of = |n: usize| {
                group(n)
                    .map(|(a, b)| chars[a..b].iter().collect::<String>())
                    .unwrap_or_default()
            };
            let message = process_regex_message(&self.message, &self.suggestion_matches, &text_of);
            let out_msg = process_regex_message(
                &self.suggestions_out_msg,
                &self.suggestion_matches_out_msg,
                &text_of,
            );
            let starts_with_upper = m_start == 0 && chars.first().is_some_and(|c| c.is_uppercase());
            if mark_end > mark_start {
                out.push(RuleMatch::new(
                    rule_index,
                    text,
                    byte_at[mark_start],
                    byte_at[mark_end],
                    byte_at[m_start],
                    byte_at[m_end],
                    message,
                    self.short_message.clone(),
                    starts_with_upper,
                    false,
                    &out_msg,
                ));
            }
            start = if m_end > m_start { m_end } else { m_end + 1 };
        }
        out
    }
}

/// `RegexPatternRule.processMessage`: `\N` is capture group N of the regex.
fn process_regex_message(
    message: &str,
    matches: &[Match],
    group: &dyn Fn(usize) -> String,
) -> String {
    let bytes = message.as_bytes();
    let mut out = String::with_capacity(message.len());
    let mut i = 0;
    let mut ref_index = 0;
    let mut last = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'\\' && bytes[i + 1].is_ascii_digit() {
            let n = (bytes[i + 1] - b'0') as usize;
            let value = group(n);
            let suggestion = match matches.get(ref_index) {
                Some(m) => match (&m.regex_replace, &m.regex_match) {
                    (Some(repl), Some(rm)) => {
                        convert_case(m.case_conversion, &rm.replace_first(&value, repl), &value)
                    }
                    _ => value,
                },
                None => value,
            };
            out.push_str(&message[last..i]);
            out.push_str(&suggestion);
            ref_index += 1;
            i += 2;
            last = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&message[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(from: usize, to: usize) -> RuleMatch {
        RuleMatch::new(
            0,
            "0123456789",
            from,
            to,
            from,
            to,
            String::new(),
            String::new(),
            false,
            false,
            "",
        )
    }

    #[test]
    fn suggestions_are_extracted_and_cased() {
        let r = RuleMatch::new(
            0,
            "un pomme",
            0,
            8,
            0,
            8,
            "Écrivez <suggestion>une pomme</suggestion> ou <suggestion>une pomme</suggestion>."
                .into(),
            String::new(),
            true,
            false,
            "<suggestion>un pommier</suggestion>",
        );
        assert_eq!(
            r.suggestions,
            vec!["Une pomme".to_string(), "Un pommier".to_string()]
        );
        let r = RuleMatch::new(
            0,
            "UN POMME",
            0,
            8,
            0,
            8,
            "<suggestion>une pomme</suggestion><suggestion><mistake/></suggestion>".into(),
            String::new(),
            true,
            true,
            "",
        );
        assert_eq!(r.suggestions, vec!["UNE POMME".to_string()]);
    }

    #[test]
    fn max_filter_drops_included_matches_of_the_same_rule() {
        let kept = rule_with_max_filter(vec![m(2, 4), m(0, 5), m(6, 8), m(6, 7)]);
        let spans: Vec<(usize, usize)> = kept.iter().map(|x| (x.from_pos, x.to_pos)).collect();
        assert_eq!(spans, vec![(0, 5), (6, 8)]);
    }

    #[test]
    fn multiple_synthesis_and_spacing() {
        assert_eq!(
            format_multiple_synthesis(
                &["a".into(), "b".into()],
                "Use <suggestion>le ",
                "</suggestion>."
            ),
            "Use <suggestion>le a</suggestion>, <suggestion>le b</suggestion>."
        );
        assert_eq!(
            concat_without_extra_space("<suggestion>le ", "</suggestion>"),
            "<suggestion>le</suggestion>"
        );
        assert_eq!(
            combine_lists(&[vec!["a".into(), "b".into()], vec![".".into()]]),
            vec!["a.".to_string(), "b.".to_string()]
        );
        assert_eq!(
            remove_suppress_misspelled("x <suggestion><pleasespellme/>(abc)</suggestion> <suggestion><pleasespellme/>ok</suggestion>"),
            "x  <suggestion>ok</suggestion>"
        );
    }

    #[test]
    fn filter_arguments() {
        let a = Readings::untagged("le", 0);
        let b = Readings::untagged("chat", 3);
        let toks = vec![&a, &b];
        let args =
            resolve_filter_arguments("wordFrom:2 desiredPostag:N.* form:\\2", &toks, &[1, 1])
                .unwrap();
        assert_eq!(args["wordFrom"], "2");
        assert_eq!(args["desiredPostag"], "N.*");
        assert_eq!(args["form"], "chat");
    }
}
