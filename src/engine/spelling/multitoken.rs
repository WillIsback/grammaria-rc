//! Spelling of multi-word names ("Rocky Mountain National Park").
//!
//! Port of `org.languagetool.rules.spelling.multitoken.MultitokenSpeller`,
//! `MultitokenSpellerFilter` and
//! `org.languagetool.rules.fr.FrenchMultitokenSpeller` (LanguageTool,
//! LGPL-2.1-or-later).
//!
//! The speller knows the multi-word entries of `multiwords.txt` (proper
//! nouns, nouns and adverbs only), `spelling_global.txt` and
//! `hyphenated_words.txt`. For a run of unknown tokens it proposes the
//! entries that are the same up to case, diacritics, hyphens and spaces, or
//! failing that the entries within a small edit distance. The filter of the
//! `FR_MULTITOKEN_SPELLING_*` rules keeps a match only when the speller has
//! something to propose.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use super::super::rule::{is_punctuation_mark, RuleContext, RuleFilter, RuleMatch};
use super::super::strtools;
use super::super::token::{Readings, Sentence};
use super::speller_rule::FrenchSpellerRule;
use super::strings::{
    is_anagram, is_not_word_string, is_space_separator, java_hash_map_order, java_is_letter,
    java_split, java_trim, levenshtein, remove_diacritics, split_non_empty, substring_before,
};
use super::wordlist::{java_lines, prepare_line_for_speller, read_resource};

/// `MultitokenSpeller.MAX_LENGTH_DIFF`.
const MAX_LENGTH_DIFF: i64 = 3;
/// `MultitokenSpellerFilter.MAX_UNDERLINED_LENGTH`.
const MAX_UNDERLINED_LENGTH: usize = 50;

/// One key of `suggestionsMap`: a normalized entry and the entries as
/// written that share it.
struct Key {
    normalized: String,
    /// `normalized.length()`.
    length: i64,
    /// `candidatePartsByKey`: the key split at spaces.
    parts: Vec<String>,
    candidates: Vec<String>,
}

/// The entries of the multi-word files, as `initMultitokenSpeller` indexes
/// them.
pub struct MultitokenEntries {
    /// `suggestionsMap`: first character of the key -> keys, in the order a
    /// `java.util.HashMap` iterates over them (the search stops early and
    /// keeps ties in that order).
    suggestions_map: HashMap<char, Vec<Key>>,
    /// `suggestionsMapNoSpacesKey`.
    suggestions_map_no_spaces_key: HashMap<String, Vec<String>>,
}

/// `MultitokenSpeller`.
pub struct MultitokenSpeller {
    spelling_rule: Arc<FrenchSpellerRule>,
    suggestions_map: HashMap<char, Vec<Key>>,
    suggestions_map_no_spaces_key: HashMap<String, Vec<String>>,
    /// `suggestionsCache`.
    cache: Mutex<HashMap<String, Arc<Vec<String>>>>,
}

fn len16(s: &str) -> i64 {
    strtools::utf16_len(s) as i64
}

/// `getNormalizeKey`: lowercase, without diacritics, hyphens as spaces.
fn normalize_key(word: &str) -> String {
    remove_diacritics(&word.to_lowercase()).replace('-', " ")
}

impl MultitokenEntries {
    /// The files of `FrenchMultitokenSpeller`: `multiwords.txt`,
    /// `spelling_global.txt` and `hyphenated_words.txt`.
    pub fn french(assets: &Path) -> MultitokenEntries {
        let contents: Vec<String> = [
            "multiwords.txt",
            "spelling_global.txt",
            "hyphenated_words.txt",
        ]
        .iter()
        .filter_map(|name| read_resource(&assets.join(name)))
        .collect();
        let contents: Vec<&str> = contents.iter().map(String::as_str).collect();
        MultitokenEntries::new(&contents)
    }

    /// `initMultitokenSpeller` on the texts of the word files.
    pub fn new(contents: &[&str]) -> MultitokenEntries {
        // Keys per first character, in insertion order.
        let mut by_char: HashMap<char, (Vec<Key>, HashMap<String, usize>)> = HashMap::new();
        let mut no_spaces: HashMap<String, Vec<String>> = HashMap::new();
        for content in contents {
            for line_original in java_lines(content) {
                if line_original.is_empty() || line_original.starts_with('#') {
                    continue; // ignore comments
                }
                let line =
                    prepare_line_for_speller(java_trim(substring_before(line_original, '#')));
                if line.is_empty() {
                    continue;
                }
                let normalized_key = normalize_key(&line);
                if !normalized_key.contains(' ') {
                    continue;
                }
                let Some(first_char) = normalized_key.chars().next() else {
                    continue;
                };
                let (keys, index) = by_char.entry(first_char).or_default();
                let at = *index.entry(normalized_key.clone()).or_insert_with(|| {
                    keys.push(Key {
                        length: len16(&normalized_key),
                        parts: split_non_empty(&normalized_key, ' ')
                            .into_iter()
                            .map(str::to_string)
                            .collect(),
                        normalized: normalized_key.clone(),
                        candidates: Vec::new(),
                    });
                    keys.len() - 1
                });
                if !keys[at].candidates.contains(&line) {
                    keys[at].candidates.push(line.clone());
                }
                let list = no_spaces
                    .entry(normalized_key.replace(' ', ""))
                    .or_default();
                if !list.contains(&line) {
                    list.push(line);
                }
            }
        }
        let suggestions_map = by_char
            .into_iter()
            .map(|(c, (keys, _))| {
                let names: Vec<&str> = keys.iter().map(|k| k.normalized.as_str()).collect();
                let order = java_hash_map_order(&names);
                let mut slots: Vec<Option<Key>> = keys.into_iter().map(Some).collect();
                (
                    c,
                    order.into_iter().filter_map(|i| slots[i].take()).collect(),
                )
            })
            .collect();
        MultitokenEntries {
            suggestions_map,
            suggestions_map_no_spaces_key: no_spaces,
        }
    }
}

impl MultitokenSpeller {
    /// `FrenchMultitokenSpeller`.
    pub fn french(assets: &Path, spelling_rule: Arc<FrenchSpellerRule>) -> MultitokenSpeller {
        MultitokenSpeller::new(spelling_rule, MultitokenEntries::french(assets))
    }

    /// `spelling_rule` is the language's default spelling rule.
    pub fn new(
        spelling_rule: Arc<FrenchSpellerRule>,
        entries: MultitokenEntries,
    ) -> MultitokenSpeller {
        MultitokenSpeller {
            spelling_rule,
            suggestions_map: entries.suggestions_map,
            suggestions_map_no_spaces_key: entries.suggestions_map_no_spaces_key,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// `getSuggestions(originalWord, areTokensAcceptedBySpeller)`.
    pub fn suggestions(
        &self,
        original_word: &str,
        are_tokens_accepted_by_speller: bool,
    ) -> Arc<Vec<String>> {
        // `WHITESPACE_AND_SEP`: runs of space separators become one space.
        let mut normalized_word = String::with_capacity(original_word.len());
        let mut in_space = false;
        for c in original_word.chars() {
            if is_space_separator(c) {
                if !in_space {
                    normalized_word.push(' ');
                }
                in_space = true;
            } else {
                normalized_word.push(c);
                in_space = false;
            }
        }
        if let Some(found) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&normalized_word)
        {
            return found.clone();
        }
        let computed =
            Arc::new(self.compute_suggestions(&normalized_word, are_tokens_accepted_by_speller));
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= 20_000 {
            cache.clear();
        }
        cache.insert(normalized_word, computed.clone());
        computed
    }

    fn compute_suggestions(
        &self,
        original_word: &str,
        are_tokens_accepted_by_speller: bool,
    ) -> Vec<String> {
        let word = original_word.replace("- ", "-").replace(" -", "-");
        if self.discard_run_on_words(&word) {
            return Vec::new();
        }
        let normalized_word = normalize_key(&word);
        let mut weighted_candidates: Vec<(&str, i32)> = Vec::new();
        let normalized_word_no_spaces = normalized_word.replace(' ', "");
        if let Some(candidates) = self
            .suggestions_map_no_spaces_key
            .get(&normalized_word_no_spaces)
        {
            if stop_searching(candidates, original_word) {
                return Vec::new();
            }
            weighted_candidates.extend(candidates.iter().map(|c| (c.as_str(), 0)));
        }
        let Some(first_char) = normalized_word.chars().next() else {
            return Vec::new();
        };
        if let Some(keys) = self
            .suggestions_map
            .get(&first_char)
            .filter(|_| weighted_candidates.is_empty())
        {
            let word_parts = split_non_empty(&normalized_word, ' ');
            let min_length = len16(&word) - MAX_LENGTH_DIFF;
            let max_length = len16(&word) + MAX_LENGTH_DIFF;
            for key in keys {
                if key.length < min_length || key.length > max_length {
                    continue;
                }
                if stop_searching(&key.candidates, original_word) {
                    return Vec::new();
                }
                let distances =
                    distances_per_word(&key.parts, &word_parts, &key.normalized, &normalized_word);
                let total_distance: i32 = distances.iter().sum();
                if total_distance < 1 {
                    weighted_candidates.extend(key.candidates.iter().map(|c| (c.as_str(), 0)));
                    // "Not all candidates are needed"
                    if weighted_candidates.len() == 2 {
                        break;
                    }
                    continue;
                }
                // for very short candidates, allow only distance = 0
                if key.length < 7 {
                    continue;
                }
                let exceeds_max_distance_per_token =
                    distances.iter().enumerate().any(|(i, &distance)| {
                        let word_part = word_parts.get(i).copied().unwrap_or("");
                        let candidate_part = key.parts.get(i).map(String::as_str).unwrap_or("");
                        let max_distance = if len16(word_part) > 5 && len16(candidate_part) > 4 {
                            2
                        } else {
                            1
                        };
                        distance > max_distance
                    });
                if exceeds_max_distance_per_token {
                    continue;
                }
                if total_distance <= max_edit_distance(&key.normalized, &normalized_word) {
                    weighted_candidates
                        .extend(key.candidates.iter().map(|c| (c.as_str(), total_distance)));
                }
            }
        }
        if weighted_candidates.is_empty() {
            return Vec::new();
        }
        weighted_candidates.sort_by_key(|c| c.1);
        let weight_first_candidate = weighted_candidates[0].1;
        if are_tokens_accepted_by_speller
            && weighted_candidates[0].0.to_uppercase() == original_word
        {
            // don't correct all-uppercase words
            return Vec::new();
        }
        if are_tokens_accepted_by_speller && weight_first_candidate > 1 {
            return Vec::new();
        }
        let mut results: Vec<String> = Vec::new();
        for (candidate, weight) in weighted_candidates {
            if weight - weight_first_candidate < 1 && !results.iter().any(|r| r == candidate) {
                results.push(candidate.to_string());
            }
        }
        results
    }

    /// `discardRunOnWords`: two tokens that only need their space moved by
    /// one character are the single-word speller's business.
    fn discard_run_on_words(&self, underlined_error: &str) -> bool {
        let parts = split_non_empty(underlined_error, ' ');
        if parts.len() != 2 {
            return false;
        }
        if strtools::is_capitalized_word(parts[1]) {
            return false;
        }
        let misspelled = |w: &str| self.spelling_rule.is_misspelled(w);
        let cut = parts[0]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        let sugg1a = &parts[0][..cut];
        let sugg1b = format!("{}{}", &parts[0][cut..], parts[1]);
        if !misspelled(sugg1a) && !misspelled(&sugg1b) {
            return true;
        }
        let first = parts[1].chars().next().map(char::len_utf8).unwrap_or(0);
        let sugg2a = format!("{}{}", parts[0], &parts[1][..first]);
        let sugg2b = &parts[1][first..];
        !misspelled(&sugg2a) && !misspelled(sugg2b)
    }
}

/// `stopSearching`: the text is already an entry, or an entry in title case.
fn stop_searching(candidates: &[String], original_word: &str) -> bool {
    if candidates.iter().any(|c| c == original_word) {
        return true;
    }
    candidates.iter().any(|c| {
        *c == c.to_lowercase()
            && strtools::convert_to_title_case_iterating_chars(c) == original_word
    })
}

/// `maxEditDistance`: the longer the part that differs, the more edits are
/// tolerated; a wrong first letter costs most of the allowance.
fn max_edit_distance(normalized_candidate: &str, normalized_word: &str) -> i32 {
    let total_length = len16(normalized_word);
    let correct_length =
        total_length - number_of_correct_chars(normalized_candidate, normalized_word);
    let first_char_wrong: f32 = first_character_distances(normalized_candidate, normalized_word)
        .iter()
        .sum();
    if correct_length <= 7 {
        return (2.0f32 - first_char_wrong) as i32;
    }
    (2.0f64 + 0.25 * (correct_length - 7) as f64 - 0.6 * f64::from(first_char_wrong)) as i32
}

/// `distancesPerWord`: one distance per token when both sides have the same
/// number of tokens (more than one), one for the whole text otherwise.
fn distances_per_word(parts1: &[String], parts2: &[&str], s1: &str, s2: &str) -> Vec<i32> {
    if parts1.len() == parts2.len() && parts1.len() > 1 {
        parts1
            .iter()
            .zip(parts2)
            .map(|(a, b)| levenshtein_distance(a, b))
            .collect()
    } else {
        vec![levenshtein_distance(s1, s2)]
    }
}

fn first_character_distances(s1: &str, s2: &str) -> Vec<f32> {
    let parts1 = split_non_empty(s1, ' ');
    let parts2 = split_non_empty(s2, ' ');
    if parts1.len() == parts2.len() && parts1.len() == 2 {
        parts1
            .iter()
            .zip(&parts2)
            .map(|(a, b)| {
                char_distance(
                    a.chars().next().unwrap_or(' '),
                    b.chars().next().unwrap_or(' '),
                )
            })
            .collect()
    } else {
        vec![0.0]
    }
}

fn char_distance(a: char, b: char) -> f32 {
    if a == b {
        return 0.0;
    }
    match (a, b) {
        ('s', 'z') | ('z', 's') | ('b', 'v') | ('v', 'b') => 0.2,
        ('i', 'y') | ('y', 'i') => 0.0,
        _ => 1.0,
    }
}

/// `levenshteinDistance`: plain Levenshtein, made cheaper for look-alike
/// spellings (`y`/`i`, `k`/`c`) and anagrams.
fn levenshtein_distance(s1: &str, s2: &str) -> i32 {
    if s1.replace(' ', "") == s2.replace(' ', "") {
        return 0;
    }
    let mut distance = levenshtein(s1, s2) as i32;
    let ns1 = normalize_similar_chars(s1);
    let ns2 = normalize_similar_chars(s2);
    if s1 != ns1 || s2 != ns2 {
        distance = distance.min(levenshtein(&ns1, &ns2) as i32);
    }
    let anagram = is_anagram(s1, s2);
    if distance > 1 && anagram {
        distance -= 1;
    }
    if distance > 0 && len16(s1) == len16(s2) && anagram {
        distance = 1;
    }
    distance
}

fn normalize_similar_chars(s: &str) -> String {
    s.replace('y', "i").replace("ko", "co").replace("ka", "ca")
}

/// `numberOfCorrectChars`: the length of the tokens that are right.
fn number_of_correct_chars(s1: &str, s2: &str) -> i64 {
    let parts1 = java_split(s1, ' ');
    let parts2 = java_split(s2, ' ');
    let mut correct_tokens = 0;
    if parts1.len() == parts2.len() && parts1.len() > 1 {
        for (a, b) in parts1.iter().zip(&parts2) {
            if a == b {
                correct_tokens += len16(a);
            }
        }
    }
    correct_tokens
}

/// `MultitokenSpellerFilter`.
pub struct MultitokenSpellerFilter {
    speller: Arc<MultitokenSpeller>,
}

impl MultitokenSpellerFilter {
    pub fn new(speller: Arc<MultitokenSpeller>) -> MultitokenSpellerFilter {
        MultitokenSpellerFilter { speller }
    }
}

/// `isClearlyUnsuitable`: too long, or fewer than two letters.
fn is_clearly_unsuitable(s: &str) -> bool {
    if strtools::utf16_len(s) > MAX_UNDERLINED_LENGTH {
        return true;
    }
    s.chars().filter(|c| java_is_letter(*c)).count() < 2
}

impl RuleFilter for MultitokenSpellerFilter {
    fn accept(
        &self,
        mut m: RuleMatch,
        _args: &HashMap<String, String>,
        pattern_token_pos: usize,
        pattern_tokens: &[&Readings],
        _token_positions: &[usize],
        sentence: &Sentence,
        _ctx: &RuleContext,
    ) -> Option<RuleMatch> {
        if pattern_tokens.iter().all(|t| t.ignored_by_speller) {
            return None;
        }
        let underlined_error = m.original_error_str.clone();
        if is_clearly_unsuitable(&underlined_error) {
            return None;
        }
        // Only English, German, Portuguese and Dutch ask the speller whether
        // it accepts the tokens.
        let are_tokens_accepted_by_speller = false;
        let suggestions = self
            .speller
            .suggestions(&underlined_error, are_tokens_accepted_by_speller);
        if suggestions.is_empty() {
            return None;
        }
        let mut replacements: Vec<String> = Vec::new();
        if strtools::utf16_len(&underlined_error) > 4
            && strtools::is_all_uppercase(&underlined_error)
        {
            for replacement in suggestions.iter() {
                let new_replacement = replacement.to_uppercase();
                if !replacements.contains(&new_replacement) && underlined_error != new_replacement {
                    replacements.push(new_replacement);
                }
            }
        } else {
            // capitalize at sentence start
            let mut words_start_pos = 1;
            while words_start_pos < sentence.len() && {
                let token: &str = &sentence.tok(words_start_pos).token;
                is_punctuation_mark(token) || is_not_word_string(token)
            } {
                words_start_pos += 1;
            }
            if pattern_token_pos == words_start_pos {
                for replacement in suggestions.iter() {
                    let new_replacement = if *replacement == replacement.to_lowercase() {
                        strtools::uppercase_first_char(replacement)
                    } else {
                        replacement.clone()
                    };
                    if !replacements.contains(&new_replacement)
                        && underlined_error != new_replacement
                    {
                        replacements.push(new_replacement);
                    }
                }
            } else {
                replacements = suggestions.to_vec();
            }
        }
        if replacements.is_empty() {
            return None;
        }
        m.suggestions = replacements;
        Some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distances() {
        assert_eq!(levenshtein_distance("new york", "newyork"), 0);
        assert_eq!(
            levenshtein_distance("tokyo", "tokio"),
            0,
            "y and i are the same letter"
        );
        assert_eq!(
            levenshtein_distance("york", "yrok"),
            1,
            "an anagram of the same length"
        );
        assert_eq!(levenshtein_distance("knight", "night"), 1);
        assert_eq!(
            distances_per_word(
                &["dark".to_string(), "knight".to_string()],
                &["derk", "knight"],
                "dark knight",
                "derk knight"
            ),
            [1, 0]
        );
        assert_eq!(
            distances_per_word(
                &["dark".to_string(), "knight".to_string()],
                &["darkknight"],
                "dark knight",
                "darkknight"
            ),
            [0]
        );
        assert_eq!(
            number_of_correct_chars("dark knight rises", "derk knight rises"),
            11
        );
        // 17 characters, 11 of them in correct tokens: 6 <= 7.
        assert_eq!(
            max_edit_distance("dark knight rises", "derk knight rises"),
            2
        );
        // Two tokens, the first letter of one is wrong.
        assert_eq!(max_edit_distance("dark knight", "bark knight"), 1);
        // Nothing right over 11 characters: 2 + 0.25 * 4 - 0.6 * 2 = 1.8.
        assert_eq!(max_edit_distance("dark knight", "sark znight"), 1);
    }

    #[test]
    fn suggestions_for_names() {
        let Some(tools) = super::super::testing::tools() else {
            return;
        };
        let speller = MultitokenSpeller::french(
            std::path::Path::new("assets/fr/lt"),
            tools.spelling.rule.clone(),
        );
        let suggest = |text: &str| speller.suggestions(text, false).to_vec();
        assert_eq!(suggest("The Derk Knight Rises"), ["The Dark Knight Rises"]);
        assert_eq!(
            suggest("Rocky Mountains Naional Park"),
            ["Rocky Mountain National Park"]
        );
        assert_eq!(suggest("Christina Aguillera"), ["Christina Aguilera"]);
        assert_eq!(suggest("Alfred Hitchkock"), ["Alfred Hitchcock"]);
        assert_eq!(suggest("General Motor"), ["General Motors"]);
        assert_eq!(suggest("Etats-Unis"), ["États-Unis"]);
        assert_eq!(suggest("Alpes Maritimæ"), ["Alpes-Maritimes"]);
        // An entry as it is written, or in title case, needs nothing.
        assert!(suggest("The Dark Knight Rises").is_empty());
        assert!(suggest("New York Times").is_empty());
        // Two words that only need their space moved are left to the
        // single-word speller.
        assert!(suggest("ordi nateur").is_empty());
    }

    #[test]
    fn stops_on_known_entries() {
        let candidates = vec!["home page".to_string(), "New York".to_string()];
        assert!(stop_searching(&candidates, "New York"));
        assert!(stop_searching(&candidates, "Home Page"));
        assert!(!stop_searching(&candidates, "new york"));
        assert!(is_clearly_unsuitable("+ ="));
        assert!(!is_clearly_unsuitable("New Yrok"));
    }
}
