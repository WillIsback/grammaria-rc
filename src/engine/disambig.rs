//! Disambiguation rules: token patterns whose match edits the readings of the
//! matched tokens (keep, remove, replace, add, unify, immunize...).
//!
//! Port of `DisambiguationPatternRule` and `DisambiguationPatternRuleReplacer`
//! from `org.languagetool.tagging.disambiguation.rules` (LanguageTool,
//! LGPL-2.1-or-later). Grammar-rule antipatterns are disambiguation rules
//! with the `Immunize` action.

use std::sync::Arc;

use super::pattern::{CaseConversion, IncludeRange, Match, MatchState};
use super::performer::{PatternMatch, Performer};
use super::rule::{resolve_filter_arguments, FilterRef, RuleContext, RuleMatch, TokenRule};
use super::token::{Reading, Readings, Sentence};
use crate::re::{self, Flags};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisambigAction {
    Add,
    Filter,
    Remove,
    Replace,
    Unify,
    Immunize,
    IgnoreSpelling,
    FilterAll,
    AddChunk,
}

impl DisambigAction {
    pub fn parse(s: &str) -> Option<DisambigAction> {
        Some(match s.to_ascii_uppercase().as_str() {
            "ADD" => DisambigAction::Add,
            "FILTER" => DisambigAction::Filter,
            "REMOVE" => DisambigAction::Remove,
            "REPLACE" => DisambigAction::Replace,
            "UNIFY" => DisambigAction::Unify,
            "IMMUNIZE" => DisambigAction::Immunize,
            "IGNORE_SPELLING" => DisambigAction::IgnoreSpelling,
            "FILTERALL" => DisambigAction::FilterAll,
            "ADDCHUNK" => DisambigAction::AddChunk,
            _ => return None,
        })
    }
}

/// `DisambiguationPatternRule`.
pub struct DisambigRule {
    pub id: String,
    pub rule: TokenRule,
    pub disambiguated_pos: Option<String>,
    pub match_element: Option<Match>,
    pub action: DisambigAction,
    /// `<wd>` elements: token text (may be empty), lemma (may be absent), tag.
    pub new_token_readings: Vec<Reading>,
    pub anti_patterns: Vec<Arc<DisambigRule>>,
}

/// The working copy of a sentence during one rule application. Matching runs
/// on `view` (the non-whitespace tokens as the rule saw them at the start,
/// plus the edits made *in place*); `all` receives every edit. LanguageTool
/// has the same split: tokens that an action *replaces* keep their old
/// readings for the rest of the pass, tokens edited in place do not.
struct Work {
    all: Vec<Readings>,
    view: Vec<Readings>,
    /// For each non-whitespace index: does `view` still mirror `all`?
    attached: Vec<bool>,
}

impl Work {
    fn edit_in_place(&mut self, sentence: &Sentence, non_ws_index: usize, f: &dyn Fn(&mut Readings)) {
        let Some(&position) = sentence.non_ws.get(non_ws_index) else { return };
        f(&mut self.all[position]);
        if self.attached[non_ws_index] {
            f(&mut self.view[non_ws_index]);
        }
    }

    fn replace(&mut self, sentence: &Sentence, non_ws_index: usize, new: Readings) {
        let Some(&position) = sentence.non_ws.get(non_ws_index) else { return };
        self.all[position] = new;
        self.attached[non_ws_index] = false;
    }
}

impl DisambigRule {
    /// `replace`: apply the rule to the sentence. `None` when nothing changed.
    pub fn replace(&self, sentence: &Sentence, ctx: &RuleContext) -> Option<Sentence> {
        if self.rule.pattern.can_be_ignored_for(sentence) {
            return None;
        }
        let mut work = Work {
            all: sentence.tokens.clone(),
            view: sentence.non_ws.iter().map(|&i| sentence.tokens[i].clone()).collect(),
            attached: vec![true; sentence.non_ws.len()],
        };
        let mut changed = false;
        let mut performer = Performer::new(&self.rule.pattern, &ctx.lang, &ctx.disambig_unifier, false, None);
        let starts = performer.start_indices(sentence, work.view.len(), true);
        for start in starts {
            let pm = {
                let tokens: Vec<&Readings> = work.view.iter().collect();
                performer.match_from(start, &tokens)
            };
            let Some(pm) = pm else { continue };
            let rule_match_from = work.view[pm.first_match_token].start_pos;
            let rule_match_to = work.view[pm.last_match_token.min(work.view.len() - 1)].end_pos();
            let matching_tokens = pm.token_positions.iter().filter(|&&p| p != 0).count();
            if self.keep_despite_filter(&work.view, &pm, ctx) && self.keep_by_disambig(sentence, rule_match_from, rule_match_to, ctx) {
                self.execute_action(sentence, &mut work, &pm, matching_tokens, ctx);
                changed = true;
            }
        }
        if changed {
            Some(Sentence::new(sentence.text.clone(), work.all, sentence.pre_disambig.clone()))
        } else {
            None
        }
    }

    /// `keepByDisambig`: an antipattern of the rule overlapping the match
    /// cancels it.
    fn keep_by_disambig(&self, sentence: &Sentence, from: usize, to: usize, ctx: &RuleContext) -> bool {
        for ap in &self.anti_patterns {
            if ap.rule.pattern.can_be_ignored_for(sentence) {
                continue;
            }
            for d in ap.rule.match_sentence(usize::MAX, sentence, ctx, &ctx.disambig_unifier) {
                if (d.from_pos <= from && d.to_pos >= from) || (d.from_pos <= to && d.to_pos >= to) || (d.from_pos >= from && d.to_pos <= to) {
                    return false;
                }
            }
        }
        true
    }

    fn keep_despite_filter(&self, view: &[Readings], pm: &PatternMatch, ctx: &RuleContext) -> bool {
        match &self.rule.filter {
            None => true,
            Some(FilterRef::Unsupported(_)) => false,
            Some(FilterRef::Native(filter)) => {
                let tokens: Vec<&Readings> = view.iter().collect();
                let last = pm.last_match_token.min(tokens.len() - 1);
                let relevant = &tokens[pm.first_match_token..=last];
                // LanguageTool resolves `\N` arguments against the whole
                // sentence here (unlike grammar rules).
                let Some(args) = resolve_filter_arguments(self.rule.filter_args.as_deref().unwrap_or(""), &tokens, &pm.token_positions) else {
                    return false;
                };
                let fake = RuleMatch::new(usize::MAX, "", 0, 1, 0, 1, "(internal rule)".into(), String::new(), false, false, "");
                filter.accept(fake, &args, pm.first_match_token, relevant, &pm.token_positions, ctx).is_some()
            }
        }
    }

    /// `executeAction`.
    fn execute_action(&self, sentence: &Sentence, work: &mut Work, pm: &PatternMatch, matching_tokens: usize, ctx: &RuleContext) {
        let token_positions = &pm.token_positions;
        let first_match_token = pm.first_match_token as i64;
        // The original passes the last token inside <marker> here.
        let last_match_token: i64 = pm.last_marker_match_token.map(|x| x as i64).unwrap_or(-1);
        let mut corrected_st_pos: i64 = 0;
        let mut start_position_correction = self.rule.start_position_correction as i64;
        let mut end_position_correction = self.rule.end_position_correction as i64;
        let mut matching_tokens_with_correction = matching_tokens as i64;
        if start_position_correction > 0 {
            corrected_st_pos -= 1;
            let mut l = 0;
            while l <= start_position_correction && (l as usize) < token_positions.len() {
                corrected_st_pos += token_positions[l as usize] as i64;
                l += 1;
            }
            let w = start_position_correction;
            for j in 0..=w {
                if (j as usize) < token_positions.len() && token_positions[j as usize] == 0 {
                    start_position_correction -= 1;
                }
            }
        }
        if end_position_correction < 0 {
            let mut d = start_position_correction.max(0) as usize;
            while d < token_positions.len() {
                if token_positions[d] == 0 {
                    end_position_correction += 1;
                }
                d += 1;
            }
        }
        if last_match_token != -1 {
            let max_pos_correction = ((last_match_token + 1 - (first_match_token + corrected_st_pos)) - matching_tokens as i64).max(0);
            matching_tokens_with_correction += max_pos_correction;
        }
        let base = first_match_token + corrected_st_pos;
        if base < 0 || base as usize >= sentence.non_ws.len() {
            return;
        }
        let base = base as usize;
        let span = matching_tokens_with_correction - start_position_correction + end_position_correction;
        let from_pos = sentence.non_ws[base];
        let in_range = |i: usize| base + i < sentence.non_ws.len();
        let new_readings = &self.new_token_readings;
        let pos = self.disambiguated_pos.as_deref();

        let replace_fallthrough = |work: &mut Work| {
            if !new_readings.is_empty() {
                if new_readings.len() as i64 == span {
                    let start_pos = work.all[from_pos].start_pos;
                    for (i, nr) in new_readings.iter().enumerate() {
                        if !in_range(i) {
                            break;
                        }
                        let position = sentence.non_ws[base + i];
                        let token: Arc<str> = if nr.token.is_empty() { work.all[position].token.clone() } else { nr.token.clone() };
                        let lemma: Arc<str> = nr.lemma.clone().unwrap_or_else(|| token.clone());
                        let analyzed = Reading::new(token, nr.pos_tag, Some(lemma));
                        let to_replace = Readings::new(vec![analyzed], start_pos);
                        let new = Readings::with_readings(&work.all[position], to_replace.readings);
                        work.replace(sentence, base + i, new);
                    }
                }
            } else if self.match_element.is_none() {
                let old = &work.all[from_pos];
                let mut lemma: Option<Arc<str>> = None;
                for r in &old.readings {
                    if r.pos().is_some() && r.pos() == pos && r.lemma.is_some() {
                        lemma = r.lemma.clone();
                    }
                }
                if lemma.as_deref().map(str::is_empty).unwrap_or(true) {
                    lemma = old.readings[0].lemma.clone();
                }
                let analyzed = Reading::new(old.token.clone(), pos.map(super::token::Tag::new), lemma);
                let new = Readings::with_readings(old, vec![analyzed]);
                work.replace(sentence, base, new);
            } else if let Some(me) = &self.match_element {
                let old = work.all[from_pos].clone();
                let mut st = MatchState::new(me, &ctx.lang);
                st.set_token(&old);
                if let Some(filtered) = st.filter_readings() {
                    let new = Readings::with_readings(&old, filtered.readings);
                    work.replace(sentence, base, new);
                }
            }
        };

        match self.action {
            DisambigAction::Unify => {
                if let Some(unified) = &pm.unified_tokens {
                    if unified.len() as i64 == span && !unified.is_empty() && in_range(unified.len() - 1) {
                        let mut unified = unified.clone();
                        let last_position = sentence.non_ws[base + unified.len() - 1];
                        if work.all[last_position].is_sent_end {
                            let n = unified.len();
                            unified[n - 1].set_sent_end();
                        }
                        for (i, u) in unified.into_iter().enumerate() {
                            let position = sentence.non_ws[base + i];
                            let new = Readings::with_readings(&work.all[position], u.readings);
                            work.replace(sentence, base + i, new);
                        }
                    }
                }
            }
            DisambigAction::Remove => {
                if !new_readings.is_empty() {
                    if new_readings.len() as i64 == span {
                        for (i, nr) in new_readings.iter().enumerate() {
                            if in_range(i) {
                                work.edit_in_place(sentence, base + i, &|t| t.remove_reading(nr));
                            }
                        }
                    }
                } else if let Some(pos) = pos.filter(|p| !p.is_empty()) {
                    if let Ok(p) = re::compile(pos, Flags::default()) {
                        let snapshot = work.all[from_pos].readings.clone();
                        for r in snapshot {
                            if r.pos().is_some_and(|t| p.is_full_match(t)) {
                                work.edit_in_place(sentence, base, &|t| t.remove_reading(&r));
                            }
                        }
                    }
                }
            }
            DisambigAction::Add => {
                if !new_readings.is_empty() && new_readings.len() as i64 == span {
                    for (i, nr) in new_readings.iter().enumerate() {
                        if !in_range(i) {
                            break;
                        }
                        let position = sentence.non_ws[base + i];
                        let token: Arc<str> = if nr.token.is_empty() { work.all[position].token.clone() } else { nr.token.clone() };
                        let lemma: Arc<str> = nr.lemma.clone().unwrap_or_else(|| token.clone());
                        let new_tok = Reading::new(token, nr.pos_tag, Some(lemma));
                        work.edit_in_place(sentence, base + i, &|t| t.add_reading(new_tok.clone()));
                    }
                }
            }
            DisambigAction::AddChunk => {
                if !new_readings.is_empty() && new_readings.len() as i64 == span {
                    for (i, nr) in new_readings.iter().enumerate() {
                        if !in_range(i) {
                            break;
                        }
                        if let Some(tag) = nr.pos() {
                            let tag: Arc<str> = Arc::from(tag);
                            work.edit_in_place(sentence, base + i, &|t| {
                                if !t.chunk_tags.iter().any(|c| **c == *tag) {
                                    t.chunk_tags.push(tag.clone());
                                }
                            });
                        }
                    }
                }
            }
            DisambigAction::FilterAll => {
                let pattern_tokens = &self.rule.pattern.tokens;
                for i in 0..span.max(0) as usize {
                    if !in_range(i) {
                        break;
                    }
                    let position = sentence.non_ws[base + i];
                    let idx = i as i64 + start_position_correction;
                    if idx < 0 || idx as usize >= token_positions.len() {
                        break;
                    }
                    let idx = idx as usize;
                    let p_token = if token_positions[idx] > 0 {
                        pattern_tokens.get(idx)
                    } else {
                        let mut k = 1usize;
                        while ((idx + k) as i64) < pattern_tokens.len() as i64 + end_position_correction
                            && idx + k < token_positions.len()
                            && token_positions[idx + k] == 0
                        {
                            k += 1;
                        }
                        pattern_tokens.get(idx + k)
                    };
                    let Some(p_token) = p_token else { break };
                    let tag = p_token.pos_tag();
                    let tmp = Match::new(tag, None, true, tag, None, CaseConversion::None, false, false, IncludeRange::None);
                    let old = work.all[position].clone();
                    let mut st = MatchState::new(&tmp, &ctx.lang);
                    st.set_token(&old);
                    if let Some(filtered) = st.filter_readings() {
                        let new = Readings::with_readings(&old, filtered.readings);
                        work.replace(sentence, base + i, new);
                    }
                }
            }
            DisambigAction::Immunize => {
                for i in 0..span.max(0) as usize {
                    if in_range(i) {
                        work.edit_in_place(sentence, base + i, &|t| t.immunized = true);
                    }
                }
            }
            DisambigAction::IgnoreSpelling => {
                for i in 0..span.max(0) as usize {
                    if in_range(i) {
                        work.edit_in_place(sentence, base + i, &|t| t.ignored_by_speller = true);
                    }
                }
            }
            DisambigAction::Filter if self.match_element.is_none() => {
                if let Some(pos) = pos {
                    let tmp = Match::new(Some(pos), None, true, Some(pos), None, CaseConversion::None, false, false, IncludeRange::None);
                    let old = work.all[from_pos].clone();
                    let new_pos_matches = match re::compile(pos, Flags::default()) {
                        Ok(p) => old.readings.iter().any(|r| !r.has_no_tag && r.pos().is_some_and(|t| p.is_full_match(t))),
                        Err(_) => false,
                    };
                    if new_pos_matches {
                        let mut st = MatchState::new(&tmp, &ctx.lang);
                        st.set_token(&old);
                        if let Some(filtered) = st.filter_readings() {
                            let new = Readings::with_readings(&old, filtered.readings);
                            work.replace(sentence, base, new);
                        }
                    }
                }
            }
            DisambigAction::Filter | DisambigAction::Replace => replace_fallthrough(work),
        }
    }
}
