//! Rule filters that need the spell checker.
//!
//! Port of `org.languagetool.rules.AbstractFindSuggestionsFilter` with
//! `org.languagetool.rules.fr.FindSuggestionsFilter`, of
//! `org.languagetool.rules.AbstractSuppressMisspelledSuggestionsFilter` with
//! `org.languagetool.rules.fr.FrenchSuppressMisspelledSuggestionsFilter`, and
//! of the Damerau-Levenshtein distance of
//! `org.languagetool.rules.spelling.symspell.implementation.EditDistance`
//! (LanguageTool, LGPL-2.1-or-later).
//!
//! * `FindSuggestionsFilter` replaces the `{suggestion}` placeholder of a
//!   rule's suggestions by the spelling suggestions for one of the matched
//!   words that carry a wanted part-of-speech tag (`desiredPostag`); when the
//!   speller has none, forms synthesized from the lemmas of its suggestions
//!   are used, closest first.
//! * `FrenchSuppressMisspelledSuggestionsFilter` drops the suggestions of a
//!   match that are misspelled, and the match itself when none is left
//!   (unless `suppressMatch:false`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::super::pattern::WordTagger;
use super::super::rule::{RuleContext, RuleFilter, RuleMatch};
use super::super::strtools;
use super::super::token::{Reading, Readings, Sentence};
use super::speller_rule::FrenchSpellerRule;
use super::strings::{java_split, make_wrong, remove_diacritics};
use super::Tokenize;
use crate::re::{self, Flags, Regex};

/// `AbstractFindSuggestionsFilter.MAX_SUGGESTIONS`.
const MAX_SUGGESTIONS: usize = 10;

/// `EditDistance` with `DistanceAlgorithm.Damerau`: the distance from a base
/// string, bounded by a maximum (`-1` beyond it).
pub struct EditDistance {
    base: Option<Vec<u16>>,
}

impl EditDistance {
    pub fn new(base_string: &str) -> EditDistance {
        let base: Vec<u16> = base_string.encode_utf16().collect();
        EditDistance {
            base: if base.is_empty() { None } else { Some(base) },
        }
    }

    /// `compare` (`DamerauLevenshteinDistance`).
    pub fn compare(&self, string2: &str, max_distance: i32) -> i32 {
        let other: Vec<u16> = string2.encode_utf16().collect();
        let Some(base) = &self.base else { return other.len() as i32 };
        if other.is_empty() {
            return base.len() as i32;
        }
        if max_distance == 0 {
            return if *base == other { 0 } else { -1 };
        }
        // string1 is the shorter of the two.
        let (string1, mut string2): (&[u16], &[u16]) = if base.len() > other.len() { (&other, base) } else { (base, &other) };
        let mut s_len = string1.len();
        let mut t_len = string2.len();
        // suffix common to both strings can be ignored
        while s_len > 0 && string1[s_len - 1] == string2[t_len - 1] {
            s_len -= 1;
            t_len -= 1;
        }
        let mut start = 0;
        if string1[0] == string2[0] || s_len == 0 {
            // prefix common to both strings can be ignored
            while start < s_len && string1[start] == string2[start] {
                start += 1;
            }
            s_len -= start;
            t_len -= start;
            // if all of the shorter string matches prefix and/or suffix of
            // the longer string, the distance is the number of extra chars
            if s_len == 0 {
                return t_len as i32;
            }
            string2 = &string2[start..start + t_len];
        }
        let len_diff = t_len - s_len;
        let mut max_distance = max_distance;
        if max_distance < 0 || max_distance as usize > t_len {
            max_distance = t_len as i32;
        } else if len_diff as i32 > max_distance {
            return -1;
        }
        let max = max_distance as usize;
        let mut v0: Vec<i32> = vec![0; t_len];
        let mut v2: Vec<i32> = vec![0; t_len];
        for (j, slot) in v0.iter_mut().enumerate() {
            *slot = if j < max { j as i32 + 1 } else { max_distance + 1 };
        }
        let j_start_offset = max as i64 - (t_len - s_len) as i64;
        let have_max = max < t_len;
        let mut j_start = 0usize;
        let mut j_end = max;
        let mut s_char = string1[0];
        let mut current = 0i32;
        for i in 0..s_len {
            let prev_s_char = s_char;
            s_char = string1[start + i];
            let mut t_char = string2[0];
            let mut left = i as i32;
            current = left + 1;
            let mut next_trans_cost = 0;
            // no need to look beyond window of lower right diagonal -
            // maxDistance cells
            if i as i64 > j_start_offset {
                j_start += 1;
            }
            if j_end < t_len {
                j_end += 1;
            }
            for j in j_start..j_end {
                let above = current;
                let mut this_trans_cost = next_trans_cost;
                next_trans_cost = v2[j];
                current = left; // cost of diagonal (substitution)
                v2[j] = current;
                left = v0[j]; // left now equals current cost
                let prev_t_char = t_char;
                t_char = string2[j];
                if s_char != t_char {
                    if left < current {
                        current = left; // insertion
                    }
                    if above < current {
                        current = above; // deletion
                    }
                    current += 1;
                    if i != 0 && j != 0 && s_char == prev_t_char && prev_s_char == t_char {
                        this_trans_cost += 1;
                        if this_trans_cost < current {
                            current = this_trans_cost; // transposition
                        }
                    }
                }
                v0[j] = current;
            }
            if have_max && v0[i + len_diff] > max_distance {
                return -1;
            }
        }
        if current <= max_distance {
            current
        } else {
            -1
        }
    }
}

/// `StringComparator`: by Damerau distance to a word, up to 4 (beyond: 8).
fn comparator_distance(distance: &EditDistance, s: &str) -> i32 {
    const MAX_DISTANCE: i32 = 4;
    let d = distance.compare(s, MAX_DISTANCE);
    if d < 0 {
        2 * MAX_DISTANCE
    } else {
        d
    }
}

fn clean_suggestion_pattern() -> &'static Arc<Regex> {
    static PATTERN: OnceLock<Arc<Regex>> = OnceLock::new();
    PATTERN.get_or_init(|| re::compile("^[smntl]'|^(nous|vous|le|la|les|me|te|se|leur|en|y) ", Flags::default()).expect("valid pattern"))
}

/// `FindSuggestionsFilter` (French).
pub struct FindSuggestionsFilter {
    /// `French.getInstance().getDefaultSpellingRule()`.
    morfologik_rule: Arc<FrenchSpellerRule>,
    /// `FrenchTagger.INSTANCE`.
    tagger: Arc<dyn WordTagger>,
    /// `spellingSuggestionsCache`.
    cache: Mutex<HashMap<(String, bool), Arc<Vec<String>>>>,
}

impl FindSuggestionsFilter {
    pub fn new(morfologik_rule: Arc<FrenchSpellerRule>, tagger: Arc<dyn WordTagger>) -> FindSuggestionsFilter {
        FindSuggestionsFilter {
            morfologik_rule,
            tagger,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// `getSpellingSuggestions`: the speller's suggestions for the word (made
    /// wrong first when it is a known word), for the word without its final
    /// `s`, and for the word with an added `s`.
    fn spelling_suggestions(&self, token: &str, is_tagged: bool) -> Vec<String> {
        let w = if is_tagged { make_wrong(token) } else { token.to_string() };
        let mut words_to_check: Vec<String> = vec![w.clone()];
        if let Some(without_s) = w.strip_suffix('s') {
            words_to_check.push(without_s.to_string());
        }
        // `ENDS_IN_VOWEL` is `[aeioué]$` tested with `matches()`: only a word
        // that is a single vowel passes.
        if matches!(w.as_str(), "a" | "e" | "i" | "o" | "u" | "é") {
            words_to_check.push(format!("{w}s"));
        }
        let mut suggestions = Vec::new();
        for word in &words_to_check {
            suggestions.extend(self.morfologik_rule.spelling_suggestions(word));
        }
        suggestions
    }

    /// `getSpellingSuggestionsCached`.
    fn spelling_suggestions_cached(&self, token: &str, is_tagged: bool) -> Arc<Vec<String>> {
        let key = (token.to_string(), is_tagged);
        if let Some(found) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return found.clone();
        }
        let computed = Arc::new(self.spelling_suggestions(token, is_tagged));
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= 10_000 {
            cache.clear();
        }
        cache.insert(key, computed.clone());
        computed
    }
}

/// `cleanSuggestion`: without a leading elided or clitic pronoun, and only
/// the first word.
fn clean_suggestion(s: &str) -> String {
    let output = clean_suggestion_pattern().replace_all(s, "");
    java_split(&output, ' ').first().copied().unwrap_or("").to_string()
}

/// `RuleFilter.getPosition`: the index of a pattern token given as a number
/// (from 1) or relative to the marker (`marker`, `marker+1`...).
fn get_position(from_str: &str, pattern_tokens: &[&Readings], m: &RuleMatch) -> Option<usize> {
    let i: i64 = if let Some(rest) = from_str.strip_prefix("marker") {
        let mut i = 0usize;
        while i < pattern_tokens.len() && (pattern_tokens[i].start_pos < m.from_pos || pattern_tokens[i].is_sent_start) {
            i += 1;
        }
        let mut i = i as i64 + 1;
        if !rest.is_empty() {
            i += rest.trim_start_matches('+').parse::<i64>().ok()?;
        }
        i
    } else {
        from_str.parse().ok()?
    };
    if i < 1 || i as usize > pattern_tokens.len() {
        return None;
    }
    Some(i as usize - 1)
}

impl RuleFilter for FindSuggestionsFilter {
    fn accept(
        &self,
        m: RuleMatch,
        args: &HashMap<String, String>,
        _pattern_token_pos: usize,
        pattern_tokens: &[&Readings],
        _token_positions: &[usize],
        sentence: &Sentence,
        ctx: &RuleContext,
    ) -> Option<RuleMatch> {
        let mut replacements: Vec<String> = Vec::new();
        let mut replacements2: Vec<String> = Vec::new();
        let word_from = args.get("wordFrom")?;
        let desired_postag = args.get("desiredPostag")?;
        let priority_postag = args.get("priorityPostag");
        let remove_suggestions_regexp = args.get("removeSuggestionsRegexp");
        let suppress_match = args.get("suppressMatch").is_some_and(|v| v.eq_ignore_ascii_case("true"));
        let diacritics_mode = args.get("Mode").is_some_and(|mode| mode == "diacritics");
        let synth = ctx.lang.synthesizer.as_deref();
        let mut used_lemmas: Vec<Option<Arc<str>>> = Vec::new();

        let desired = re::compile(desired_postag, Flags::default()).ok()?;
        let priority = match priority_postag {
            Some(p) => Some(re::compile(p, Flags::default()).ok()?),
            None => None,
        };
        // `inmarker`: the text of the match without its spaces, taken as a
        // known word; otherwise one of the matched tokens.
        let (word, word_is_tagged): (String, bool) = if word_from == "inmarker" {
            (m.original_error_str.replace(' ', ""), true)
        } else {
            let token = pattern_tokens[get_position(word_from, pattern_tokens, &m)?];
            (token.token.to_string(), token.is_tagged())
        };
        let string_comparator = EditDistance::new(&word);
        let is_word_capitalized = strtools::is_capitalized_word(&word);
        let is_word_allupper = strtools::is_all_uppercase(&word);
        if diacritics_mode && self.tagger.tag_word(&word).matches_pos_tag_regex(&desired) {
            return None;
        }
        let regexp_pattern = match remove_suggestions_regexp {
            Some(r) => Some(re::compile(r, Flags::default()).ok()?),
            None => None,
        };
        let suggestions = self.spelling_suggestions_cached(&word, word_is_tagged);
        let mut used_priority_postag_pos = 0;
        for suggestion in suggestions.iter() {
            let analyzed_suggestion = self.tagger.tag_word(&clean_suggestion(suggestion));
            if replacements.len() >= 2 * MAX_SUGGESTIONS {
                continue;
            }
            let mut used = false;
            if *suggestion != word && analyzed_suggestion.matches_pos_tag_regex(&desired) {
                if !replacements.contains(suggestion)
                    && !replacements.contains(&suggestion.to_lowercase())
                    && (!diacritics_mode || strtools::equals_ignore_case(&remove_diacritics(suggestion), &remove_diacritics(&word)))
                    && !regexp_pattern.as_ref().is_some_and(|p| p.is_full_match(suggestion))
                {
                    let mut replacement = suggestion.clone();
                    if is_word_allupper {
                        replacement = replacement.to_uppercase();
                    }
                    if is_word_capitalized {
                        replacement = strtools::uppercase_first_char(&replacement);
                    }
                    if priority.as_ref().is_some_and(|p| analyzed_suggestion.matches_pos_tag_regex(p)) {
                        replacements.insert(used_priority_postag_pos, replacement);
                        used_priority_postag_pos += 1;
                    } else {
                        replacements.push(replacement);
                    }
                    used = true;
                }
            }
            if !used {
                if let Some(synth) = synth {
                    // try with the synthesizer
                    let mut synthesized_suggestions: Vec<String> = Vec::new();
                    for reading in &analyzed_suggestion.readings {
                        if used_lemmas.contains(&reading.lemma) {
                            continue;
                        }
                        let synthesized = synthesize(synth, reading, desired_postag);
                        used_lemmas.push(reading.lemma.clone());
                        for form in synthesized {
                            if !synthesized_suggestions.contains(&form) {
                                synthesized_suggestions.push(form);
                            }
                        }
                        // As in LanguageTool, the forms found so far for this
                        // suggestion are added again for each new lemma.
                        for form in &synthesized_suggestions {
                            let mut replacement = form.clone();
                            if is_word_allupper {
                                replacement = replacement.to_uppercase();
                            }
                            if is_word_capitalized {
                                replacement = strtools::uppercase_first_char(&replacement);
                            }
                            replacements2.push(replacement);
                        }
                    }
                }
            }
        }

        let match_contains_some_finished_suggestion = m.suggestions.iter().any(|k| !k.to_lowercase().contains("{suggestion}"));
        if diacritics_mode && replacements.is_empty() && !match_contains_some_finished_suggestion {
            return None;
        }
        if replacements.len() + replacements2.len() == 0 && suppress_match && !match_contains_some_finished_suggestion {
            return None;
        }
        // The new match reads its suggestions from the message again, without
        // adapting their case.
        let mut rule_match = RuleMatch::new(
            m.rule,
            &sentence.text,
            m.from_pos,
            m.to_pos,
            m.from_pos,
            m.to_pos,
            m.message.clone(),
            m.short_message.clone(),
            false,
            false,
            "",
        );
        let mut definitive_replacements: Vec<String> = Vec::new();
        let mut replacements_used = false;
        for s in &m.suggestions {
            if s.contains("{suggestion}") || s.contains("{Suggestion}") || s.contains("{SUGGESTION}") {
                replacements_used = true;
                for s2 in &replacements {
                    if definitive_replacements.len() >= MAX_SUGGESTIONS {
                        break;
                    }
                    if s.contains("{suggestion}") {
                        if !definitive_replacements.contains(s2) {
                            definitive_replacements.push(s.replace("{suggestion}", s2));
                        }
                    } else if s.contains("{Suggestion}") {
                        let capitalized = strtools::uppercase_first_char(s2);
                        if !definitive_replacements.contains(&capitalized) {
                            definitive_replacements.push(s.replace("{Suggestion}", &capitalized));
                        }
                    } else {
                        let upper = s2.to_uppercase();
                        if !definitive_replacements.contains(&upper) {
                            definitive_replacements.push(s.replace("{SUGGESTION}", &upper));
                        }
                    }
                }
            } else if !definitive_replacements.contains(s) {
                definitive_replacements.push(s.clone());
            }
        }
        if !replacements_used {
            if replacements.is_empty() {
                // `Collections.sort(replacements2, stringComparator)`: stable.
                let mut keyed: Vec<(i32, String)> = replacements2.into_iter().map(|r| (comparator_distance(&string_comparator, &r), r)).collect();
                keyed.sort_by_key(|(d, _)| *d);
                for (_, replacement) in keyed {
                    if !replacements.contains(&replacement) && !definitive_replacements.contains(&replacement) {
                        replacements.push(replacement);
                    }
                }
            }
            for replacement in &replacements {
                if definitive_replacements.len() >= MAX_SUGGESTIONS {
                    break;
                }
                if !definitive_replacements.contains(replacement) {
                    definitive_replacements.push(replacement.clone());
                }
            }
        }
        if let Some(at) = definitive_replacements.iter().position(|r| *r == m.original_error_str) {
            definitive_replacements.remove(at);
        }
        if !definitive_replacements.is_empty() {
            let mut distinct: Vec<String> = Vec::with_capacity(definitive_replacements.len());
            for r in definitive_replacements {
                if !distinct.contains(&r) {
                    distinct.push(r);
                }
            }
            rule_match.suggestions = distinct;
        }
        rule_match.original_error_str = m.original_error_str;
        Some(rule_match)
    }
}

/// `synth.synthesize(token, desiredPostag, true)`.
fn synthesize(synth: &dyn super::super::pattern::Synthesizer, reading: &Reading, desired_postag: &str) -> Vec<String> {
    synth.synthesize_regexp(reading, desired_postag, true)
}

/// `FrenchSuppressMisspelledSuggestionsFilter`.
pub struct SuppressMisspelledSuggestionsFilter {
    /// `language.getDefaultSpellingRule()`.
    spelling_rule: Arc<FrenchSpellerRule>,
    /// `language.getTagger()`.
    tagger: Arc<dyn WordTagger>,
    /// `language.getWordTokenizer()`.
    tokenize: Tokenize,
}

impl SuppressMisspelledSuggestionsFilter {
    pub fn new(spelling_rule: Arc<FrenchSpellerRule>, tagger: Arc<dyn WordTagger>, tokenize: Tokenize) -> SuppressMisspelledSuggestionsFilter {
        SuppressMisspelledSuggestionsFilter {
            spelling_rule,
            tagger,
            tokenize,
        }
    }

    /// `isMisspelled(s, language)`: one of the tokens of `s` is misspelled.
    pub fn is_misspelled(&self, s: &str) -> bool {
        (self.tokenize)(s).iter().any(|token| self.spelling_rule.is_misspelled(token))
    }
}

impl RuleFilter for SuppressMisspelledSuggestionsFilter {
    fn accept(
        &self,
        mut m: RuleMatch,
        args: &HashMap<String, String>,
        _pattern_token_pos: usize,
        _pattern_tokens: &[&Readings],
        _token_positions: &[usize],
        _sentence: &Sentence,
        _ctx: &RuleContext,
    ) -> Option<RuleMatch> {
        let suppress_match = args.get("suppressMatch")?;
        let suppress_postag = match args.get("SuppressPostag") {
            Some(p) => Some(re::compile(p, Flags::default()).ok()?),
            None => None,
        };
        let filter_postag = match args.get("FilterPostag") {
            Some(p) => Some(re::compile(p, Flags::default()).ok()?),
            None => None,
        };
        let check_postag = suppress_postag.is_some() || filter_postag.is_some();
        let mut new_replacements: Vec<String> = Vec::new();
        for replacement in std::mem::take(&mut m.suggestions) {
            if self.is_misspelled(&replacement) {
                continue;
            }
            let mut add_replacement = true;
            if check_postag {
                let readings = self.tagger.tag_word(&replacement);
                if suppress_postag.as_ref().is_some_and(|p| readings.matches_pos_tag_regex(p)) {
                    add_replacement = false;
                }
                if filter_postag.as_ref().is_some_and(|p| !readings.matches_pos_tag_regex(p)) {
                    add_replacement = false;
                }
            }
            if add_replacement {
                new_replacements.push(replacement);
            }
        }
        let suppress = !suppress_match.eq_ignore_ascii_case("false");
        if new_replacements.is_empty() && suppress {
            None
        } else {
            m.suggestions = new_replacements;
            Some(m)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damerau_distance() {
        let d = EditDistance::new("maison");
        assert_eq!(d.compare("maison", 4), 0);
        assert_eq!(d.compare("maisno", 4), 1, "a transposition is one edit");
        assert_eq!(d.compare("maisons", 4), 1);
        assert_eq!(d.compare("raison", 4), 1);
        assert_eq!(d.compare("mason", 4), 1);
        assert_eq!(d.compare("saisie", 4), 3);
        assert_eq!(d.compare("anticonstitutionnel", 4), -1);
        assert_eq!(d.compare("", 4), 6);
        assert_eq!(EditDistance::new("").compare("abc", 4), 3);
        assert_eq!(d.compare("maisno", 0), -1);
        assert_eq!(comparator_distance(&d, "anticonstitutionnel"), 8);
    }

    #[test]
    fn suggestions_are_cleaned_before_tagging() {
        assert_eq!(clean_suggestion("s'allie"), "allie");
        assert_eq!(clean_suggestion("nous allons"), "allons");
        assert_eq!(clean_suggestion("la maison"), "maison");
        assert_eq!(clean_suggestion("a lot"), "a");
        assert_eq!(clean_suggestion("maison"), "maison");
    }

    fn context(tools: &super::super::testing::Tools) -> RuleContext {
        let mut lang = super::super::super::pattern::Lang::default();
        lang.synthesizer = super::super::super::synth::FrenchSynthesizer::load(std::path::Path::new("assets/fr/lt"))
            .ok()
            .map(|s| Arc::new(s) as _);
        lang.tagger = Some(tools.spelling.tagger.clone());
        lang.speller = Some(tools.spelling.spell_checker());
        RuleContext {
            lang,
            unifier: Default::default(),
            disambig_unifier: Default::default(),
        }
    }

    fn arguments(args: &str) -> HashMap<String, String> {
        args.split(' ')
            .filter_map(|a| a.split_once(':'))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// A match over the `marked` tokens of the sentence (indexes among its
    /// non-whitespace tokens) with the given suggestions in its message.
    fn rule_match(sentence: &Sentence, marked: (usize, usize), suggestions: &[&str]) -> RuleMatch {
        let (from, to) = (sentence.tok(marked.0).start_pos, sentence.tok(marked.1).end_pos());
        let message: String = suggestions.iter().map(|s| format!("<suggestion>{s}</suggestion>")).collect();
        RuleMatch::new(0, &sentence.text, from, to, from, to, message, String::new(), false, false, "")
    }

    #[test]
    fn find_suggestions_filter() {
        let Some(tools) = super::super::testing::tools() else { return };
        let ctx = context(tools);
        let filter = tools.spelling.filter("FindSuggestionsFilter").unwrap();
        let run = |text: &str, pattern: (usize, usize), marked: (usize, usize), suggestions: &[&str], args: &str| -> Option<Vec<String>> {
            let sentence = tools.analyse(text);
            let tokens = sentence.non_ws_tokens();
            let m = rule_match(&sentence, marked, suggestions);
            let positions = vec![1; pattern.1 - pattern.0 + 1];
            filter
                .accept(m, &arguments(args), pattern.0, &tokens[pattern.0..=pattern.1], &positions, &sentence, &ctx)
                .map(|m| m.suggestions.into_iter().take(5).collect())
        };
        // JE_VERBE[1]: a verb in the first person for the unknown word.
        assert_eq!(
            run(
                "Je connai ton frère.",
                (1, 2),
                (2, 2),
                &[],
                "wordFrom:2 desiredPostag:V.*1.s|R.* removeSuggestionsRegexp:^.*[éè]$"
            ),
            Some(s(&["donnai", "cannai", "sonnai", "tonnai", "monnaie"]))
        );
        // ILS_VERBE[2].
        assert_eq!(
            run(
                "ils le ferront crever à petit feu",
                (1, 3),
                (3, 3),
                &[],
                "wordFrom:3 desiredPostag:V.*3.p|R.pers.obj.*"
            ),
            Some(s(&["feront", "seront", "verront", "ferrent", "ferreront"]))
        );
        // LETTRE_UNIQUE[6]: a known word is first made wrong ("fïlle"); the
        // placeholder of the rule's suggestion takes each proposal.
        assert_eq!(
            run(
                "C'est s fille.",
                (2, 4),
                (3, 4),
                &["fille", "s'{suggestion}"],
                "wordFrom:3 desiredPostag:V.* RemoveSuggestionsRegexp:^.*[éè]$"
            ),
            Some(s(&["fille", "s'file", "s'filé", "s'faille", "s'fallu"]))
        );
        // Nothing to propose: the match goes away when the rule asks for it.
        assert_eq!(
            run("Je xyzzyqw", (1, 2), (2, 2), &[], "wordFrom:2 desiredPostag:V.*1.s suppressMatch:true"),
            None
        );
        assert_eq!(run("Je xyzzyqw", (1, 2), (2, 2), &[], "wordFrom:2 desiredPostag:V.*1.s"), Some(Vec::new()));
    }

    #[test]
    fn suppress_misspelled_suggestions_filter() {
        let Some(tools) = super::super::testing::tools() else { return };
        let ctx = context(tools);
        let filter = tools.spelling.filter("FrenchSuppressMisspelledSuggestionsFilter").unwrap();
        let run = |suggestions: &[&str], args: &str| -> Option<Vec<String>> {
            let sentence = tools.analyse("auto défense");
            let tokens = sentence.non_ws_tokens();
            let mut m = rule_match(&sentence, (1, 2), &[]);
            m.suggestions = s(suggestions);
            filter
                .accept(m, &arguments(args), 1, &tokens[1..=2], &[1, 1], &sentence, &ctx)
                .map(|m| m.suggestions)
        };
        // "auto-défense" is one token (the tagger knows the prefix) that the
        // dictionary does not have.
        assert_eq!(run(&["autodéfense", "auto-défense"], "suppressMatch:true"), Some(s(&["autodéfense"])));
        assert_eq!(run(&["auto-défense", "autodéfence"], "suppressMatch:true"), None);
        assert_eq!(run(&["auto-défense"], "suppressMatch:false"), Some(Vec::new()));
        // Every token of a suggestion is checked.
        assert_eq!(
            run(&["l'autodéfense", "l'autodéfence", "dit-il"], "suppressMatch:true"),
            Some(s(&["l'autodéfense", "dit-il"]))
        );
        assert_eq!(run(&["maison", "chat"], "suppressMatch:true FilterPostag:N.f.*"), Some(s(&["maison"])));
        assert_eq!(run(&["maison", "chat"], "suppressMatch:true SuppressPostag:N.f.*"), Some(s(&["chat"])));
    }

    fn s(list: &[&str]) -> Vec<String> {
        list.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn positions() {
        let a = Readings::untagged("le", 0);
        let b = Readings::untagged("chat", 3);
        let c = Readings::untagged("dort", 8);
        let tokens = vec![&a, &b, &c];
        let m = RuleMatch::new(0, "le chat dort", 3, 7, 0, 12, String::new(), String::new(), false, false, "");
        assert_eq!(get_position("2", &tokens, &m), Some(1));
        assert_eq!(get_position("marker", &tokens, &m), Some(1));
        assert_eq!(get_position("marker+1", &tokens, &m), Some(2));
        assert_eq!(get_position("4", &tokens, &m), None);
        assert_eq!(get_position("0", &tokens, &m), None);
    }
}
