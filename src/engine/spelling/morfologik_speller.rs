//! The spellers behind the Morfologik spelling rule.
//!
//! Port of `org.languagetool.rules.spelling.morfologik.MorfologikSpeller`,
//! `MorfologikMultiSpeller` and `WeightedSuggestion` (LanguageTool,
//! LGPL-2.1-or-later).
//!
//! A [`MorfologikSpeller`] wraps one dictionary and a maximum edit distance;
//! a [`MorfologikMultiSpeller`] asks the binary dictionary and the dictionary
//! made of the plain-text word lists, and merges their answers. No user
//! dictionary is ported (there is no user configuration here).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use super::super::strtools;
use super::strings::{java_is_all_uppercase, java_is_mixed_case, java_starts_with_uppercase};
use crate::morfologik::dictionary::apply_replacements;
use crate::morfologik::{unicode, Automaton, Dictionary, MatchKind, Speller};

/// `SpellingCheckRule.LANGUAGETOOL` / `LANGUAGETOOLER`: never misspelled.
const LANGUAGETOOL: &str = "LanguageTool";
const LANGUAGETOOLER: &str = "LanguageTooler";
/// `StringMatcher.MAX_MATCH_LENGTH`.
const MAX_MATCH_LENGTH: usize = 250;

/// `WeightedSuggestion`: a word and its weight (lower is better).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeightedSuggestion {
    pub word: String,
    pub weight: i32,
}

/// `MorfologikSpeller`: one dictionary, one maximum edit distance.
pub struct MorfologikSpeller {
    dictionary: Arc<Dictionary>,
    max_edit_distance: usize,
    /// The `containsSeparators` field of the long-lived `Speller` Java keeps
    /// for `isMisspelled`: true until a word without separator is found as a
    /// whole sequence of the automaton (a word list rather than a tagged
    /// dictionary); from then on only whole sequences count.
    contains_separators: AtomicBool,
}

impl MorfologikSpeller {
    pub fn new(dictionary: Arc<Dictionary>, max_edit_distance: usize) -> MorfologikSpeller {
        assert!(max_edit_distance > 0, "maxEditDistance must be > 0: {max_edit_distance}");
        MorfologikSpeller { dictionary, max_edit_distance, contains_separators: AtomicBool::new(true) }
    }

    /// `isMisspelled`.
    pub fn is_misspelled(&self, word: &str) -> bool {
        if word.is_empty() || word == LANGUAGETOOL || word == LANGUAGETOOLER {
            return false;
        }
        self.speller_is_misspelled(word)
    }

    /// `morfologik.speller.Speller.isInDictionary`, with the state of the
    /// speller object LanguageTool keeps between calls.
    fn is_in_dictionary(&self, word: &str) -> bool {
        let bytes = word.as_bytes();
        let separator = self.dictionary.metadata.separator;
        let m = self.dictionary.fsa.match_sequence(bytes);
        let mut contains_separators = self.contains_separators.load(Ordering::Relaxed);
        if contains_separators && m.kind == MatchKind::ExactMatch {
            contains_separators = bytes.contains(&separator);
            self.contains_separators.store(contains_separators, Ordering::Relaxed);
        }
        if m.kind == MatchKind::ExactMatch && !contains_separators {
            return true;
        }
        contains_separators && m.kind == MatchKind::SequenceIsAPrefix && !bytes.is_empty() && self.dictionary.fsa.arc(m.node, separator) != 0
    }

    /// `morfologik.speller.Speller.isMisspelled`.
    fn speller_is_misspelled(&self, word: &str) -> bool {
        let meta = &self.dictionary.metadata;
        let converted;
        let word_to_check: &str = if meta.input_conversion_pairs.is_empty() {
            word
        } else {
            converted = apply_replacements(word, &meta.input_conversion_pairs);
            &converted
        };
        let units: Vec<u16> = word_to_check.encode_utf16().collect();
        let is_alphabetic = units.len() != 1 || unicode::is_alphabetic(units[0]);
        !units.is_empty()
            && (!meta.ignore_punctuation || is_alphabetic)
            && (!meta.ignore_numbers || !units.iter().any(|&c| unicode::is_digit(c)))
            && !(meta.ignore_camel_case && is_camel_case(&units))
            && !(meta.ignore_all_uppercase && is_alphabetic && is_all_uppercase(&units))
            && !self.is_in_dictionary(word_to_check)
            && (!meta.convert_case
                || !(!is_mixed_case(&units)
                    && (self.is_in_dictionary(&unicode::lower_string(word_to_check))
                        || is_all_uppercase(&units) && self.is_in_dictionary(&initial_uppercase(&units)))))
    }

    /// `getSuggestions`: replacement candidates, then run-on word splits,
    /// with the case of the misspelled word given back to the suggestions.
    pub fn suggestions(&self, word: &str) -> Vec<WeightedSuggestion> {
        let mut suggestions: Vec<WeightedSuggestion> = Vec::new();
        let length = strtools::utf16_len(word);
        if length > MAX_MATCH_LENGTH {
            return suggestions;
        }
        // A new speller for each word, as in LanguageTool: the search state
        // of Morfologik's speller is not reset between two words.
        let mut speller = Speller::with_edit_distance(&self.dictionary, self.max_edit_distance);
        if length < 50 {
            // slow for long words (the limit is arbitrary)
            for candidate in speller.find_replacement_candidates(word) {
                suggestions.push(WeightedSuggestion { word: candidate.word, weight: candidate.distance });
            }
        }
        for candidate in speller.replace_run_on_word_candidates(word) {
            suggestions.push(WeightedSuggestion { word: candidate.word, weight: candidate.distance });
        }
        let converting_case = self.dictionary.metadata.convert_case;
        if converting_case && java_is_all_uppercase(word) {
            recase(&mut suggestions, word, |s| s.to_uppercase());
        } else if converting_case && java_starts_with_uppercase(word) {
            recase(&mut suggestions, word, strtools::uppercase_first_char);
        }
        suggestions
    }

    /// `convertsCase`.
    pub fn converts_case(&self) -> bool {
        self.dictionary.metadata.convert_case
    }

    /// `getFrequency`: the frequency class of the word, or of its lowercase
    /// form.
    pub fn frequency(&self, word: &str) -> i32 {
        let mut freq = self.dictionary.frequency(word);
        if freq == 0 {
            let lower = word.to_lowercase();
            if lower != word {
                freq = self.dictionary.frequency(&lower);
            }
        }
        freq
    }
}

/// The case loops of `MorfologikSpeller.getSuggestions`: every suggestion is
/// rewritten with `convert` (unless that gives back the misspelled word, or
/// the suggestion is in mixed case); a rewritten form already in the list is
/// not repeated.
fn recase(suggestions: &mut Vec<WeightedSuggestion>, word: &str, convert: impl Fn(&str) -> String) {
    let mut i: isize = 0;
    while (i as usize) < suggestions.len() {
        let at = i as usize;
        let mut converted = convert(&suggestions[at].word);
        if converted == word || java_is_mixed_case(&suggestions[at].word) {
            converted = suggestions[at].word.clone();
        }
        let mut aux_index = suggestions.iter().position(|s| s.word == converted);
        if let Some(aux) = aux_index.filter(|&aux| aux > at) {
            suggestions.remove(aux);
            // Java keeps testing the stale index below.
            aux_index = Some(aux);
        }
        match aux_index {
            Some(aux) if aux < at => {
                suggestions.remove(at);
                i -= 1;
            }
            _ => {
                let weight = suggestions[at].weight;
                suggestions[at] = WeightedSuggestion { word: converted, weight };
            }
        }
        i += 1;
    }
}

// The character tests of `morfologik.speller.Speller`, on UTF-16 units.

fn is_all_uppercase(s: &[u16]) -> bool {
    !s.iter().any(|&c| unicode::is_letter(c) && unicode::is_lower_case(c))
}

fn is_not_all_lowercase(s: &[u16]) -> bool {
    s.iter().any(|&c| unicode::is_letter(c) && !unicode::is_lower_case(c))
}

fn is_not_capitalized_word(s: &[u16]) -> bool {
    if !s.is_empty() && unicode::is_upper_case(s[0]) {
        return s[1..].iter().any(|&c| unicode::is_letter(c) && !unicode::is_lower_case(c));
    }
    true
}

fn is_mixed_case(s: &[u16]) -> bool {
    !is_all_uppercase(s) && is_not_capitalized_word(s) && is_not_all_lowercase(s)
}

fn is_camel_case(s: &[u16]) -> bool {
    !s.is_empty()
        && !is_all_uppercase(s)
        && is_not_capitalized_word(s)
        && unicode::is_upper_case(s[0])
        && (s.len() <= 1 || unicode::is_lower_case(s[1]))
        && is_not_all_lowercase(s)
}

/// `initialUppercase`: the first unit unchanged, the rest lowercased.
fn initial_uppercase(word: &[u16]) -> String {
    let (first, rest) = word.split_at(1.min(word.len()));
    let mut s = String::from_utf16_lossy(first);
    s.push_str(&unicode::lower_string(&String::from_utf16_lossy(rest)));
    s
}

/// Size of the suggestion cache (`defaultDictSuggestionsCache` holds 2 000
/// entries in LanguageTool; this one is simply emptied when full).
const SUGGESTION_CACHE_SIZE: usize = 20_000;

/// `MorfologikMultiSpeller`: the binary dictionary plus the plain-text word
/// lists, at one edit distance.
pub struct MorfologikMultiSpeller {
    spellers: Vec<MorfologikSpeller>,
    converts_case: bool,
    /// `isMisspelled` verdicts. The answer for a word never changes once the
    /// spellers are warm, so it is safe to keep.
    misspelled: RwLock<HashMap<String, bool>>,
    default_dict_suggestions: Mutex<HashMap<String, Arc<Vec<WeightedSuggestion>>>>,
}

impl MorfologikMultiSpeller {
    /// `binary` is the `.dict` dictionary, `plain_text` the dictionary built
    /// from the word lists (`None` when they are empty).
    pub fn new(binary: Arc<Dictionary>, plain_text: Option<Arc<Dictionary>>, max_edit_distance: usize) -> MorfologikMultiSpeller {
        let speller = MorfologikSpeller::new(binary, max_edit_distance);
        let converts_case = speller.converts_case();
        let mut spellers = vec![speller];
        if let Some(dictionary) = plain_text {
            spellers.push(MorfologikSpeller::new(dictionary, max_edit_distance));
        }
        MorfologikMultiSpeller { spellers, converts_case, misspelled: RwLock::new(HashMap::new()), default_dict_suggestions: Mutex::new(HashMap::new()) }
    }

    /// `isMisspelled`: no dictionary knows the word.
    pub fn is_misspelled(&self, word: &str) -> bool {
        if let Some(&known) = self.misspelled.read().unwrap_or_else(|e| e.into_inner()).get(word) {
            return known;
        }
        let verdict = self.spellers.iter().all(|s| s.is_misspelled(word));
        let mut cache = self.misspelled.write().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= 200_000 {
            cache.clear();
        }
        cache.insert(word.to_string(), verdict);
        verdict
    }

    /// `getFrequency`: the first non-zero frequency.
    pub fn frequency(&self, word: &str) -> i32 {
        self.spellers.iter().map(|s| s.frequency(word)).find(|&f| f > 0).unwrap_or(0)
    }

    /// `getSuggestionsFromSpellers`: the suggestions of every speller, without
    /// repeats and without the word itself, best first (stable).
    fn suggestions_from_spellers(&self, word: &str) -> Vec<WeightedSuggestion> {
        let mut result: Vec<WeightedSuggestion> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for speller in &self.spellers {
            for suggestion in speller.suggestions(word) {
                if !seen.contains(&suggestion.word) && suggestion.word != word {
                    result.push(suggestion.clone());
                }
                seen.insert(suggestion.word);
            }
        }
        result.sort_by_key(|s| s.weight);
        result
    }

    /// `getSuggestions`: all spellers, words only.
    pub fn suggestions(&self, word: &str) -> Vec<String> {
        self.suggestions_from_spellers(word).into_iter().map(|s| s.word).collect()
    }

    /// `getWeightedSuggestionsFromDefaultDicts` (cached). Without a user
    /// dictionary the default dictionaries are all the dictionaries.
    pub fn weighted_suggestions_from_default_dicts(&self, word: &str) -> Arc<Vec<WeightedSuggestion>> {
        if let Some(found) = self.default_dict_suggestions.lock().unwrap_or_else(|e| e.into_inner()).get(word) {
            return found.clone();
        }
        let computed = Arc::new(self.suggestions_from_spellers(word));
        let mut cache = self.default_dict_suggestions.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= SUGGESTION_CACHE_SIZE {
            cache.clear();
        }
        cache.insert(word.to_string(), computed.clone());
        computed
    }

    pub fn converts_case(&self) -> bool {
        self.converts_case
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(word: &str, weight: i32) -> WeightedSuggestion {
        WeightedSuggestion { word: word.to_string(), weight }
    }

    #[test]
    fn suggestions_take_the_case_of_the_word() {
        let mut list = vec![ws("paris", 1), ws("Paris", 2), ws("pari", 3), ws("iPad", 4)];
        recase(&mut list, "Pariss", strtools::uppercase_first_char);
        assert_eq!(list, [ws("Paris", 1), ws("Pari", 3), ws("iPad", 4)]);

        let mut list = vec![ws("Paris", 1), ws("paris", 2), ws("pari", 3)];
        recase(&mut list, "PARISS", |s| s.to_uppercase());
        assert_eq!(list, [ws("PARIS", 1), ws("PARI", 3)]);

        // A suggestion equal to the word once converted keeps its own case.
        let mut list = vec![ws("école", 1), ws("écoles", 2)];
        recase(&mut list, "École", strtools::uppercase_first_char);
        assert_eq!(list, [ws("école", 1), ws("Écoles", 2)]);
    }

    #[test]
    fn speller_character_classes() {
        let u = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert!(is_mixed_case(&u("iPhone")) && !is_mixed_case(&u("Paris")) && !is_mixed_case(&u("PARIS")));
        assert!(is_camel_case(&u("HelloWorld")) && !is_camel_case(&u("Hello")));
        assert_eq!(initial_uppercase(&u("PARIS")), "Paris");
    }
}
