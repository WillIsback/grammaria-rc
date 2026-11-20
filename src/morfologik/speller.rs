//! Spelling suggestions over a Morfologik dictionary.
//!
//! Port of `morfologik.speller.Speller` and `HMatrix` (Morfologik 2.1.9,
//! BSD-3-Clause, © Dawid Weiss, Marcin Miłkowski, Jaume Ortolà). The search is
//! K. Oflazer's error-tolerant recognition (1996) as implemented in Jan
//! Daciuk's `s_fsa`, extended with replacement pairs, equivalent characters,
//! diacritics- and case-insensitive comparison, and frequency ranking.
//!
//! The port is literal on purpose: the candidate lists and their order are
//! compared with the Java implementation. In particular
//! * strings are handled as UTF-16 code units inside the search, as Java
//!   `char`s are (lengths, indexes and the 120-unit limit are the same);
//! * the distance matrix is **not** cleared between two searches, exactly as
//!   in Java, where the result of a search can depend on the previous one.
//!   LanguageTool creates a new `Speller` for each word it wants suggestions
//!   for; [`Speller::reset`] gives the same fresh state without rebuilding
//!   the replacement tables;
//! * the sort by `(edit distance, frequency)` is stable, so candidates of
//!   equal weight stay in discovery order (automaton order).
//!
//! A [`Speller`] borrows a shared [`Dictionary`] and holds mutable search
//! state: it is cheap to create, `Send`, and meant to be used by one thread.
//!
//! Lowercasing and uppercasing use Rust's locale-independent mappings, where
//! Java uses the dictionary's locale (`fsa.dict.speller.locale`, by default
//! the JVM's): results differ only for Turkish, Azeri and Lithuanian.

use std::collections::HashMap;
use std::collections::HashSet;

use super::dictionary::{apply_replacements, index_of, Dictionary, Lookup};
use super::fsa::{Automaton, Fsa, MatchKind};
use super::metadata::DictionaryMetadata;
use super::unicode;

/// Maximum length (in UTF-16 units) of a word to find replacements for.
pub const MAX_WORD_LENGTH: usize = 120;
/// Number of frequency classes (`'A'..='Z'`).
const FREQ_RANGES: i32 = 26;
/// Upper limit on the number of rewritten words searched (performance).
const UPPER_SEARCH_LIMIT: usize = 15;
/// Rewritten words shorter than this are not searched.
const MIN_WORD_LENGTH: i32 = 4;
/// Depth limit of [`Speller::get_all_replacements`].
const MAX_RECURSION_LEVEL: usize = 6;

/// A suggestion with its weight: port of `Speller.CandidateData`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateData {
    /// The suggested word.
    pub word: String,
    /// The edit distance the search found (`0` for a replacement-pair hit).
    pub orig_distance: i32,
    /// The sort key: `orig_distance * 26 + 25 - frequency`, lower is better.
    pub distance: i32,
}

/// Port of `morfologik.speller.HMatrix`: the band of the edit distance matrix
/// around its diagonal, addressed with (possibly negative) Java indexes.
struct HMatrix {
    p: Vec<i32>,
    row_length: i32,
    edit_distance: i32,
}

impl HMatrix {
    fn new(distance: i32, max_length: i32) -> HMatrix {
        let row_length = max_length + 2;
        let column_height = 2 * distance + 3;
        let size = (row_length * column_height) as usize;
        let mut p = vec![0i32; size];
        // Initialize edges of the diagonal band to distance + 1 (i.e.
        // distance too big).
        for i in 0..(row_length - distance - 1).max(0) as usize {
            p[i] = distance + 1; // H(distance + j, j) = distance + 1
            p[size - i - 1] = distance + 1; // H(i, distance + i) = distance + 1
        }
        // Initialize items H(i,j) with at least one index equal to zero to
        // |i - j|.
        for j in 0..(2 * distance + 1) {
            p[(j * row_length) as usize] = distance + 1 - j; // H(i=0..distance+1,0)=i
            let k = ((j + distance + 1) * row_length + j) as usize;
            p[k.min(size - 1)] = j; // H(0,j=0..distance+1)=j
        }
        HMatrix {
            p,
            row_length,
            edit_distance: distance,
        }
    }

    #[inline]
    fn index(&self, i: i32, j: i32) -> Option<usize> {
        let k = (j - i + self.edit_distance + 1) * self.row_length + j;
        if k >= 0 && (k as usize) < self.p.len() {
            Some(k as usize)
        } else {
            None
        }
    }

    /// `get(i, j)`. Out of the band (where Java throws), "too far".
    #[inline]
    fn get(&self, i: i32, j: i32) -> i32 {
        match self.index(i, j) {
            Some(k) => self.p[k],
            None => self.edit_distance + 1,
        }
    }

    #[inline]
    fn set(&mut self, i: i32, j: i32, val: i32) {
        if let Some(k) = self.index(i, j) {
            self.p[k] = val;
        }
    }
}

/// Finds spelling suggestions: port of `morfologik.speller.Speller`.
pub struct Speller<'a> {
    dict: &'a Dictionary,
    fsa: &'a Fsa,
    meta: &'a DictionaryMetadata,
    root_node: usize,
    /// Reads frequencies (`getFrequency`).
    lookup: Lookup<'a>,

    edit_distance: i32,
    /// Effective edit distance for the word being processed.
    effect_edit_distance: i32,
    h_matrix: HMatrix,

    /// Current replacement, as UTF-16 units.
    candidate: Vec<u16>,
    /// Length of the word being processed.
    word_len: i32,
    /// Word being processed, as UTF-16 units.
    word_processed: Vec<u16>,

    /// Target character → source strings it can stand for.
    replacements_any_to_one: HashMap<u16, Vec<Vec<u16>>>,
    /// Target pair of characters → source strings it can stand for.
    replacements_any_to_two: HashMap<[u16; 2], Vec<Vec<u16>>>,
    /// Source string → longer targets, applied by rewriting the word.
    replacements_the_rest: Vec<(Vec<u16>, Vec<Vec<u16>>)>,
    /// Equivalent characters as UTF-16 units.
    equivalent_chars: HashMap<u16, Vec<u16>>,

    /// True until the dictionary is known to store bare words (entries
    /// without `separator + data`), see [`Speller::is_in_dictionary`].
    contains_separators: bool,
}

impl<'a> Speller<'a> {
    /// A speller with a maximum edit distance of 1 (`new Speller(dictionary)`).
    pub fn new(dict: &'a Dictionary) -> Speller<'a> {
        Self::with_edit_distance(dict, 1)
    }

    /// A speller with the given maximum edit distance.
    pub fn with_edit_distance(dict: &'a Dictionary, edit_distance: usize) -> Speller<'a> {
        let edit_distance = edit_distance.min(MAX_WORD_LENGTH) as i32;
        let meta = &dict.metadata;
        let utf16 = |s: &str| s.encode_utf16().collect::<Vec<u16>>();

        // createReplacementsMaps: the pairs are indexed by the length of
        // their *target*.
        let mut any_to_one: HashMap<u16, Vec<Vec<u16>>> = HashMap::new();
        let mut any_to_two: HashMap<[u16; 2], Vec<Vec<u16>>> = HashMap::new();
        let mut the_rest: Vec<(Vec<u16>, Vec<Vec<u16>>)> = Vec::new();
        for (key, values) in &meta.replacement_pairs {
            let key16 = utf16(key);
            for s in values {
                let s16 = utf16(s);
                match s16.len() {
                    1 => any_to_one.entry(s16[0]).or_default().push(key16.clone()),
                    2 => any_to_two
                        .entry([s16[0], s16[1]])
                        .or_default()
                        .push(key16.clone()),
                    _ => match the_rest.iter_mut().find(|(k, _)| *k == key16) {
                        Some((_, list)) => list.push(s16),
                        None => the_rest.push((key16.clone(), vec![s16])),
                    },
                }
            }
        }

        let mut equivalent_chars: HashMap<u16, Vec<u16>> = HashMap::new();
        for (from, to) in &meta.equivalent_chars {
            equivalent_chars
                .entry(*from as u16)
                .or_default()
                .extend(to.iter().map(|&c| c as u16));
        }

        Speller {
            dict,
            fsa: &dict.fsa,
            meta,
            root_node: dict.root_node(),
            lookup: Lookup::new(dict),
            edit_distance,
            effect_edit_distance: edit_distance,
            h_matrix: HMatrix::new(edit_distance, MAX_WORD_LENGTH as i32),
            candidate: vec![0; MAX_WORD_LENGTH],
            word_len: 0,
            word_processed: Vec::new(),
            replacements_any_to_one: any_to_one,
            replacements_any_to_two: any_to_two,
            replacements_the_rest: the_rest,
            equivalent_chars,
            contains_separators: true,
        }
    }

    /// Puts the speller back in the state of a newly created one (distance
    /// matrix and separator detection), keeping its tables.
    pub fn reset(&mut self) {
        self.h_matrix = HMatrix::new(self.edit_distance, MAX_WORD_LENGTH as i32);
        self.contains_separators = true;
        self.candidate.iter_mut().for_each(|c| *c = 0);
    }

    /// The dictionary this speller reads.
    pub fn dictionary(&self) -> &'a Dictionary {
        self.dict
    }

    /// `isMisspelled`: checks the word according to the dictionary's flags.
    ///
    /// * `ignore-punctuation`: a single non-alphabetic character is correct;
    /// * `ignore-numbers`: a word containing a digit is correct;
    /// * `ignore-camel-case`: a CamelCase word is correct;
    /// * `ignore-all-uppercase`: an all-uppercase word is correct;
    /// * otherwise the word must be in the dictionary; with `convert-case`, a
    ///   word that is not in mixed case is also accepted if its lowercase form
    ///   is known, or, when all uppercase, its capitalized form.
    pub fn is_misspelled(&mut self, word: &str) -> bool {
        // Dictionaries usually do not contain punctuation.
        let converted;
        let word_to_check: &str = if self.meta.input_conversion_pairs.is_empty() {
            word
        } else {
            converted = apply_replacements(word, &self.meta.input_conversion_pairs);
            &converted
        };
        let units: Vec<u16> = word_to_check.encode_utf16().collect();
        let meta = self.meta;
        let is_alphabetic = units.len() != 1 || unicode::is_alphabetic(units[0]);
        !units.is_empty()
            && (!meta.ignore_punctuation || is_alphabetic)
            && (!meta.ignore_numbers || contains_no_digit(&units))
            && !(meta.ignore_camel_case && is_camel_case(&units))
            && !(meta.ignore_all_uppercase && is_alphabetic && is_all_uppercase(&units))
            && !self.is_in_dictionary(word_to_check)
            && (!meta.convert_case
                || !(!is_mixed_case(&units)
                    && (self.is_in_dictionary(&unicode::lower_string(word_to_check))
                        || is_all_uppercase(&units)
                            && self.is_in_dictionary(&initial_uppercase(&units)))))
    }

    /// `isInDictionary`: true if the word is stored in the dictionary, either
    /// as a bare sequence or as the part before the separator of an entry.
    ///
    /// As in Java, the first exact match of a sequence without separator
    /// switches the speller, for good, to "this dictionary has no separators"
    /// (a word list rather than a tagged dictionary).
    pub fn is_in_dictionary(&mut self, word: &str) -> bool {
        let bytes = word.as_bytes();
        let separator = self.meta.separator;
        // Try to find a partial match in the dictionary.
        let m = self.dict.match_sequence(bytes);

        // Make sure the word doesn't contain a separator if there is an exact
        // match.
        if self.contains_separators && m.kind == MatchKind::ExactMatch {
            self.contains_separators = bytes.contains(&separator);
        }
        if m.kind == MatchKind::ExactMatch && !self.contains_separators {
            return true;
        }
        self.contains_separators
            && m.kind == MatchKind::SequenceIsAPrefix
            && !bytes.is_empty()
            && self.fsa.arc(m.node, separator) != 0
    }

    fn is_in_dictionary_utf16(&mut self, word: &[u16]) -> bool {
        match String::from_utf16(word) {
            Ok(s) => self.is_in_dictionary(&s),
            // An unpaired surrogate cannot be encoded: not in the dictionary.
            Err(_) => false,
        }
    }

    /// `getFrequency`: the frequency class of a word form, `0` (rare) to `25`,
    /// from the first entry of that form; `0` if unknown.
    pub fn frequency(&mut self, word: &str) -> i32 {
        self.lookup.frequency(word)
    }

    fn candidate_data(&mut self, word: String, distance: i32) -> CandidateData {
        let frequency = self.lookup.frequency(&word);
        CandidateData {
            word,
            orig_distance: distance,
            distance: distance * FREQ_RANGES + FREQ_RANGES - frequency - 1,
        }
    }

    /// `replaceRunOnWordCandidates`: suggestions that split a run-on word in
    /// two known words ("alot" → "a lot"), in split order.
    pub fn replace_run_on_word_candidates(&mut self, original: &str) -> Vec<CandidateData> {
        let mut candidates = Vec::new();
        let converted;
        let word_to_check: &str = if self.meta.input_conversion_pairs.is_empty() {
            original
        } else {
            converted = apply_replacements(original, &self.meta.input_conversion_pairs);
            &converted
        };
        if !self.is_in_dictionary(word_to_check) && self.meta.run_on_words {
            // Chop from left to right. A split inside a surrogate pair cannot
            // be encoded in Java and yields nothing, so only character
            // boundaries are tried.
            for (i, _) in word_to_check.char_indices().skip(1) {
                let prefix = &word_to_check[..i];
                let suffix = &word_to_check[i..];
                let suffix16: Vec<u16> = suffix.encode_utf16().collect();
                if self.is_in_dictionary(suffix)
                    // camel case words: e.g. GreatElephant
                    || (!is_not_capitalized_word(&suffix16)
                        && self.is_in_dictionary(&unicode::lower_string(suffix)))
                {
                    let first = prefix.encode_utf16().next().unwrap_or(0);
                    if self.is_in_dictionary(prefix)
                        // a word that's uppercase just because used at
                        // sentence start
                        || (unicode::is_upper_case(first)
                            && self.is_in_dictionary(&unicode::lower_string(prefix)))
                    {
                        let mut replacement = format!("{prefix} {suffix}");
                        if !self.meta.output_conversion_pairs.is_empty() {
                            replacement = apply_replacements(
                                &replacement,
                                &self.meta.output_conversion_pairs,
                            );
                        }
                        let cd = self.candidate_data(replacement, 1);
                        candidates.push(cd);
                    }
                }
            }
        }
        candidates
    }

    /// `replaceRunOnWords`: [`Speller::replace_run_on_word_candidates`] as
    /// plain strings.
    pub fn replace_run_on_words(&mut self, original: &str) -> Vec<String> {
        self.replace_run_on_word_candidates(original)
            .into_iter()
            .map(|c| c.word)
            .collect()
    }

    /// `findSimilarWordCandidates`: like
    /// [`Speller::find_replacement_candidates`], but also for a word that is
    /// in the dictionary.
    pub fn find_similar_word_candidates(&mut self, word: &str) -> Vec<CandidateData> {
        self.find_replacement_candidates_impl(word, true)
    }

    /// `findSimilarWords`: the words of
    /// [`Speller::find_similar_word_candidates`].
    pub fn find_similar_words(&mut self, word: &str) -> Vec<String> {
        self.find_similar_word_candidates(word)
            .into_iter()
            .map(|c| c.word)
            .collect()
    }

    /// `findReplacements`: suggestions for a misspelled word, best first
    /// (nothing for a word that is in the dictionary).
    pub fn find_replacements(&mut self, word: &str) -> Vec<String> {
        self.find_replacement_candidates(word)
            .into_iter()
            .map(|c| c.word)
            .collect()
    }

    /// `findReplacementCandidates`: [`Speller::find_replacements`] with the
    /// weight of each suggestion.
    pub fn find_replacement_candidates(&mut self, word: &str) -> Vec<CandidateData> {
        self.find_replacement_candidates_impl(word, false)
    }

    fn find_replacement_candidates_impl(
        &mut self,
        word: &str,
        even_if_word_in_dictionary: bool,
    ) -> Vec<CandidateData> {
        let word: String = if self.meta.input_conversion_pairs.is_empty() {
            word.to_string()
        } else {
            apply_replacements(word, &self.meta.input_conversion_pairs)
        };
        let word16: Vec<u16> = word.encode_utf16().collect();

        // Candidate strings, with additional data such as the edit distance
        // from the original word.
        let mut candidates: Vec<CandidateData> = Vec::new();

        if !word16.is_empty()
            && word16.len() < MAX_WORD_LENGTH
            && (!self.is_in_dictionary(&word) || even_if_word_in_dictionary)
        {
            let mut words_to_check: Vec<Vec<u16>> = Vec::new();
            if word16.len() > 1 {
                for word_checked in self.all_replacements_utf16(&word16, 0, 0) {
                    let as_string = String::from_utf16_lossy(&word_checked);
                    if self.is_in_dictionary_utf16(&word_checked) {
                        let cd = self.candidate_data(as_string, 0);
                        candidates.push(cd);
                    } else {
                        let lower_word = unicode::lower_string(&as_string);
                        let upper_word = unicode::upper_string(&as_string);
                        if self.is_in_dictionary(&lower_word) {
                            // Add the word as it is in the dictionary, not
                            // mixed-case versions of it.
                            let cd = self.candidate_data(lower_word.clone(), 0);
                            candidates.push(cd);
                        }
                        if self.is_in_dictionary(&upper_word) {
                            let cd = self.candidate_data(upper_word, 0);
                            candidates.push(cd);
                        }
                        let mut lower16: Vec<u16> = lower_word.encode_utf16().collect();
                        if lower16.len() > 1 {
                            lower16[0] = unicode::to_upper_case(lower16[0]);
                            if self.is_in_dictionary_utf16(&lower16) {
                                let first_upper = String::from_utf16_lossy(&lower16);
                                let cd = self.candidate_data(first_upper, 0);
                                candidates.push(cd);
                            }
                        }
                    }
                    words_to_check.push(word_checked);
                }
            } else {
                words_to_check.push(word16.clone());
            }

            // Even if a candidate was found with the replacement pairs (which
            // are usual errors), there might be more good candidates.
            let mut i = 1;
            for word_checked in words_to_check {
                i += 1;
                if i > UPPER_SEARCH_LIMIT {
                    // For performance reasons, do not search too deeply.
                    break;
                }
                self.word_len = word_checked.len() as i32;
                self.word_processed = word_checked;
                if self.word_len < MIN_WORD_LENGTH && i > 2 {
                    // Three-letter replacements make little sense anyway.
                    break;
                }
                self.candidate.clear();
                self.candidate.resize(MAX_WORD_LENGTH, 0);
                self.effect_edit_distance = if self.word_len <= self.edit_distance {
                    self.word_len - 1
                } else {
                    self.edit_distance
                };
                self.find_repl(&mut candidates, 0, self.root_node, PrevBytes::EMPTY, 0, 0);
            }
        }

        // Stable sort: equal weights keep their discovery order.
        candidates.sort_by_key(|c| c.distance);

        // Apply replacements, prune duplicates while preserving the candidate
        // order.
        let mut words: HashSet<String> = HashSet::new();
        let mut result = Vec::with_capacity(candidates.len());
        for cd in candidates {
            let replaced = if self.meta.output_conversion_pairs.is_empty() {
                cd.word
            } else {
                apply_replacements(&cd.word, &self.meta.output_conversion_pairs)
            };
            // Add only the first occurrence of a given word.
            if words.insert(replaced.clone()) && replaced != word {
                result.push(CandidateData {
                    word: replaced,
                    orig_distance: cd.orig_distance,
                    distance: cd.distance,
                });
            }
        }
        result
    }

    fn push_candidate(
        &mut self,
        candidates: &mut Vec<CandidateData>,
        cand_index: usize,
        dist: i32,
    ) {
        let word = String::from_utf16_lossy(&self.candidate[..=cand_index]);
        let cd = self.candidate_data(word, dist);
        candidates.push(cd);
    }

    /// `findRepl`: explores the arcs of `node`, extending the candidate by one
    /// character per (possibly multi-byte) step and pruning with the cut-off
    /// edit distance. `prev_bytes` holds the bytes of an unfinished UTF-8
    /// character.
    fn find_repl(
        &mut self,
        candidates: &mut Vec<CandidateData>,
        depth: i32,
        node: usize,
        prev_bytes: PrevBytes,
        word_index: i32,
        cand_index: usize,
    ) {
        let fsa = self.fsa;
        let mut arc = fsa.first_arc(node);
        while arc != 0 {
            // What Java recomputes at each use (target node, end of candidate)
            // is computed at most once per arc; the automaton is immutable.
            let mut this_arc = ArcState::new(arc);
            arc = fsa.next_arc(arc);

            let bytes = prev_bytes.with(fsa.arc_label(this_arc.arc));
            let Some(unit) = bytes.decode_first_unit() else {
                // Incomplete (or invalid) character: keep the bytes and go
                // on. Note: depth is not incremented.
                let end = self.target(&mut this_arc);
                if end != 0 {
                    self.find_repl(candidates, depth, end, bytes, word_index, cand_index);
                }
                continue;
            };
            if cand_index >= self.candidate.len() {
                // Java would overflow its candidate buffer here.
                continue;
            }
            self.candidate[cand_index] = unit;

            // Replacement "any to two".
            let length_replacement = self.match_any_to_two(word_index, cand_index);
            if length_replacement > 0 {
                // The replacement takes place at the end of the candidate.
                if self.is_end_of_candidate(&mut this_arc, word_index) {
                    let mut dist = self.h_matrix.get(depth - 1, depth - 1);
                    if dist <= self.effect_edit_distance {
                        let extra =
                            (self.word_len - 1 - (word_index + length_replacement - 2)).abs();
                        if extra > 0 {
                            // There are extra letters in the word after the
                            // replacement.
                            dist += extra;
                        }
                        if dist <= self.effect_edit_distance {
                            self.push_candidate(candidates, cand_index, dist);
                        }
                    }
                }
                if self.is_arc_not_terminal(&mut this_arc, cand_index) {
                    let x = self.h_matrix.get(depth, depth);
                    let diag = self.h_matrix.get(depth - 1, depth - 1);
                    self.h_matrix.set(depth, depth, diag);
                    let end = self.target(&mut this_arc);
                    self.find_repl(
                        candidates,
                        depth.max(0),
                        end,
                        PrevBytes::EMPTY,
                        word_index + length_replacement - 1,
                        cand_index + 1,
                    );
                    self.h_matrix.set(depth, depth, x);
                }
            }
            // Replacement "any to one".
            let length_replacement = self.match_any_to_one(word_index, cand_index);
            if length_replacement > 0 {
                // The replacement takes place at the end of the candidate.
                if self.is_end_of_candidate(&mut this_arc, word_index) {
                    let mut dist = self.h_matrix.get(depth, depth);
                    if dist <= self.effect_edit_distance {
                        let extra =
                            (self.word_len - 1 - (word_index + length_replacement - 1)).abs();
                        if extra > 0 {
                            // There are extra letters in the word after the
                            // replacement.
                            dist += extra;
                        }
                        if dist <= self.effect_edit_distance {
                            self.push_candidate(candidates, cand_index, dist);
                        }
                    }
                }
                if self.is_arc_not_terminal(&mut this_arc, cand_index) {
                    let end = self.target(&mut this_arc);
                    self.find_repl(
                        candidates,
                        depth,
                        end,
                        PrevBytes::EMPTY,
                        word_index + length_replacement,
                        cand_index + 1,
                    );
                }
            }
            // General case.
            if self.cuted(depth, word_index, cand_index) <= self.effect_edit_distance {
                if self.is_end_of_candidate(&mut this_arc, word_index) {
                    let dist = self.ed(
                        self.word_len - 1 - (word_index - depth),
                        depth,
                        self.word_len - 1,
                        cand_index,
                    );
                    if dist <= self.effect_edit_distance {
                        self.push_candidate(candidates, cand_index, dist);
                    }
                }
                if self.is_arc_not_terminal(&mut this_arc, cand_index) {
                    let end = self.target(&mut this_arc);
                    self.find_repl(
                        candidates,
                        depth + 1,
                        end,
                        PrevBytes::EMPTY,
                        word_index + 1,
                        cand_index + 1,
                    );
                }
            }
        }
    }

    /// The target node of the arc (`0` if it is terminal), computed once.
    #[inline]
    fn target(&self, st: &mut ArcState) -> usize {
        match st.target {
            Some(t) => t,
            None => {
                let t = self.fsa.end_node(st.arc);
                st.target = Some(t);
                t
            }
        }
    }

    /// `isArcNotTerminal`: the search can go on below this arc.
    #[inline]
    fn is_arc_not_terminal(&self, st: &mut ArcState, cand_index: usize) -> bool {
        self.target(st) != 0
            && !(self.contains_separators
                && self.candidate[cand_index] == self.meta.separator_char as u16)
    }

    /// `isEndOfCandidate`: the candidate is a word of the dictionary (final
    /// arc, or arc followed by the separator) of a proper length. The answer
    /// does not change while an arc is processed, so it is kept.
    #[inline]
    fn is_end_of_candidate(&self, st: &mut ArcState, word_index: i32) -> bool {
        if let Some(v) = st.end_of_candidate {
            return v;
        }
        let v = (self.word_len - 1 - word_index).abs() <= self.effect_edit_distance
            && (self.fsa.is_arc_final(st.arc) || self.is_before_separator(st));
        st.end_of_candidate = Some(v);
        v
    }

    /// `isBeforeSeparator`.
    #[inline]
    fn is_before_separator(&self, st: &mut ArcState) -> bool {
        if self.contains_separators {
            let arc1 = self.fsa.arc(self.target(st), self.meta.separator);
            return arc1 != 0 && !self.fsa.is_arc_terminal(arc1);
        }
        false
    }

    /// The UTF-16 unit of the processed word at a Java index (`0` out of
    /// range, where Java throws).
    #[inline]
    fn word_at(&self, index: i32) -> u16 {
        if index >= 0 {
            self.word_processed
                .get(index as usize)
                .copied()
                .unwrap_or(0)
        } else {
            0
        }
    }

    /// `ed`: the edit distance between the first `i + 1` units of the word
    /// and the first `j + 1` units of the candidate, from the neighbouring
    /// cells of the matrix (see Oflazer). The result is stored in the matrix.
    fn ed(&mut self, i: i32, j: i32, word_index: i32, cand_index: usize) -> i32 {
        let w = self.word_at(word_index);
        let c = self.candidate[cand_index];
        let result = if self.are_equal(w, c) {
            // Last characters are the same.
            self.h_matrix.get(i, j)
        } else if word_index > 0
            && cand_index > 0
            && w == self.candidate[cand_index - 1]
            && self.word_at(word_index - 1) == c
        {
            // Last two characters are transposed.
            let a = self.h_matrix.get(i - 1, j - 1); // transposition, e.g. ababab, ababba
            let b = self.h_matrix.get(i + 1, j); // deletion, e.g. abab, aba
            let c = self.h_matrix.get(i, j + 1); // insertion e.g. aba, abab
            1 + a.min(b).min(c)
        } else {
            // Otherwise.
            let a = self.h_matrix.get(i, j); // replacement, e.g. ababa, ababb
            let b = self.h_matrix.get(i + 1, j); // deletion, e.g. ab, a
            let c = self.h_matrix.get(i, j + 1); // insertion e.g. a, ab
            1 + a.min(b).min(c)
        };
        self.h_matrix.set(i + 1, j + 1, result);
        result
    }

    /// `areEqual`: equality of two characters for the edit distance: same
    /// character, declared equivalent, or (with `ignore-diacritics`) same
    /// base letter, also across case with `convert-case`.
    #[inline]
    fn are_equal(&self, x: u16, y: u16) -> bool {
        if x == y {
            return true;
        }
        if !self.equivalent_chars.is_empty() {
            if let Some(chars) = self.equivalent_chars.get(&x) {
                if chars.contains(&y) {
                    return true;
                }
            }
        }
        if self.meta.ignore_diacritics {
            let xn = unicode::nfd_first(x);
            let yn = unicode::nfd_first(y);
            if xn == yn {
                // Avoid case conversion, if possible.
                return true;
            }
            if self.meta.convert_case && unicode::is_letter(xn) {
                // Again, case conversion only when needed.
                let test_needed = unicode::is_lower_case(xn) != unicode::is_lower_case(yn);
                if test_needed {
                    return unicode::to_lower_case(xn) == unicode::to_lower_case(yn);
                }
            }
            return false;
        }
        false
    }

    /// `cuted`: the cut-off edit distance, i.e. the minimum of [`Self::ed`]
    /// over the prefixes of the word whose length is within the edit distance
    /// of the candidate's (see Oflazer).
    fn cuted(&mut self, depth: i32, word_index: i32, cand_index: usize) -> i32 {
        // Min chars from word to consider - 1.
        let l = (depth - self.effect_edit_distance).max(0);
        // Max chars from word to consider - 1.
        let u = (self.word_len - 1 - (word_index - depth)).min(depth + self.effect_edit_distance);
        let mut min_ed = self.effect_edit_distance + 1; // what is to be computed
        let mut wi = word_index + l - depth;
        let mut i = l;
        while i <= u {
            let d = self.ed(i, depth, wi, cand_index);
            if d < min_ed {
                min_ed = d;
            }
            i += 1;
            wi += 1;
        }
        min_ed
    }

    /// `matchAnyToOne`: matches the last letter of the candidate against two
    /// or more letters of the word; returns how many letters of the word it
    /// stands for, `0` if no replacement pair applies.
    fn match_any_to_one(&self, word_index: i32, cand_index: usize) -> i32 {
        if self.replacements_any_to_one.is_empty() {
            return 0;
        }
        if let Some(reps) = self
            .replacements_any_to_one
            .get(&self.candidate[cand_index])
        {
            for rep in reps {
                let mut i = 0usize;
                while i < rep.len()
                    && (word_index + i as i32) < self.word_len
                    && rep[i] == self.word_at(word_index + i as i32)
                {
                    i += 1;
                }
                if i == rep.len() {
                    return i as i32;
                }
            }
        }
        0
    }

    /// `matchAnyToTwo`: matches the last two letters of the candidate against
    /// the letters of the word around `word_index - 1`.
    fn match_any_to_two(&self, word_index: i32, cand_index: usize) -> i32 {
        if self.replacements_any_to_two.is_empty() {
            return 0;
        }
        if cand_index > 0 && cand_index < self.candidate.len() && word_index > 0 {
            let two = [self.candidate[cand_index - 1], self.candidate[cand_index]];
            if let Some(reps) = self.replacements_any_to_two.get(&two) {
                for rep in reps {
                    if rep.len() == 2
                        && word_index < self.word_len
                        && two[0] == self.word_at(word_index - 1)
                        && two[1] == self.word_at(word_index)
                    {
                        return 0; // unnecessary replacements
                    }
                    let mut i = 0usize;
                    while i < rep.len()
                        && (word_index - 1 + i as i32) < self.word_len
                        && rep[i] == self.word_at(word_index - 1 + i as i32)
                    {
                        i += 1;
                    }
                    if i == rep.len() {
                        return i as i32;
                    }
                }
            }
        }
        0
    }

    /// `isCamelCase`. Note that compounds with a dash such as
    /// "Waschmaschinen-Test" are camel case for this method.
    pub fn is_camel_case(&self, s: &str) -> bool {
        is_camel_case(&s.encode_utf16().collect::<Vec<u16>>())
    }

    /// `convertsCase`: whether the dictionary supports case conversions.
    pub fn converts_case(&self) -> bool {
        self.meta.convert_case
    }

    /// `getAllReplacements`: every rewriting of `s` obtained by applying, or
    /// not, the replacement pairs whose target is longer than two characters,
    /// from `from_index` on (in UTF-16 units). The first result is `s` itself.
    /// The recursion stops at level 6.
    pub fn get_all_replacements(&self, s: &str, from_index: usize, level: usize) -> Vec<String> {
        let s16: Vec<u16> = s.encode_utf16().collect();
        self.all_replacements_utf16(&s16, from_index as isize, level)
            .iter()
            .map(|w| String::from_utf16_lossy(w))
            .collect()
    }

    fn all_replacements_utf16(&self, s: &[u16], from_index: isize, level: usize) -> Vec<Vec<u16>> {
        let mut replaced: Vec<Vec<u16>> = Vec::new();
        if level > MAX_RECURSION_LEVEL {
            // Stop searching at some point.
            replaced.push(s.to_vec());
            return replaced;
        }
        let mut index = MAX_WORD_LENGTH;
        let mut key: Option<&(Vec<u16>, Vec<Vec<u16>>)> = None;
        let mut key_length = 0;
        let mut found = false;
        // Find the first possible replacement after from_index; select the
        // longest possible key.
        for entry in &self.replacements_the_rest {
            let aux_key = &entry.0;
            if let Some(aux_index) = index_of(s, aux_key, from_index) {
                if aux_index < index || (aux_index == index && aux_key.len() >= key_length) {
                    index = aux_index;
                    key = Some(entry);
                    key_length = aux_key.len();
                }
            }
        }
        if index < MAX_WORD_LENGTH {
            if let Some((key, reps)) = key {
                for rep in reps {
                    // Start a branch without replacement (only once per key).
                    if !found {
                        replaced.extend(self.all_replacements_utf16(
                            s,
                            (index + key.len()) as isize,
                            level + 1,
                        ));
                        found = true;
                    }
                    // Avoid unnecessary replacements (ex. don't replace L by
                    // L·L when L·L already present).
                    let ind = index_of(s, rep, from_index - rep.len() as isize + 1);
                    if rep.len() > key.len() {
                        if let Some(ind) = ind {
                            if ind == index
                                || ind as isize == index as isize - rep.len() as isize + 1
                            {
                                continue;
                            }
                        }
                    }
                    // Start a branch with replacement.
                    let mut sb = s.to_vec();
                    sb.splice(index..index + key.len(), rep.iter().copied());
                    replaced.extend(self.all_replacements_utf16(
                        &sb,
                        (index + rep.len()) as isize,
                        level + 1,
                    ));
                }
            }
        }
        if !found {
            replaced.push(s.to_vec());
        }
        replaced
    }
}

/// An arc being explored by `find_repl`, with what is already known of it.
struct ArcState {
    arc: usize,
    /// Its target node, once computed.
    target: Option<usize>,
    /// The result of `is_end_of_candidate`, once computed.
    end_of_candidate: Option<bool>,
}

impl ArcState {
    #[inline]
    fn new(arc: usize) -> ArcState {
        ArcState {
            arc,
            target: None,
            end_of_candidate: None,
        }
    }
}

/// The bytes of a UTF-8 character being read arc by arc (Java's `prevBytes`).
#[derive(Clone, Copy)]
struct PrevBytes {
    buf: [u8; 4],
    len: u8,
    /// More than four bytes without a complete character: never valid again.
    overflow: bool,
}

impl PrevBytes {
    const EMPTY: PrevBytes = PrevBytes {
        buf: [0; 4],
        len: 0,
        overflow: false,
    };

    /// These bytes followed by `b`.
    #[inline]
    fn with(mut self, b: u8) -> PrevBytes {
        if usize::from(self.len) < self.buf.len() {
            self.buf[usize::from(self.len)] = b;
            self.len += 1;
        } else {
            self.overflow = true;
        }
        self
    }

    /// The first UTF-16 unit of the character these bytes encode, `None` if
    /// they are not (yet) a complete UTF-8 character: what Java's
    /// `CharsetDecoder` reports as malformed input on a partial sequence.
    #[inline]
    fn decode_first_unit(&self) -> Option<u16> {
        if self.overflow {
            return None;
        }
        if self.len == 1 && self.buf[0] < 0x80 {
            return Some(u16::from(self.buf[0]));
        }
        let s = std::str::from_utf8(&self.buf[..usize::from(self.len)]).ok()?;
        let c = s.chars().next()?;
        let mut units = [0u16; 2];
        Some(c.encode_utf16(&mut units)[0])
    }
}

/// `initialUppercase`: the first unit unchanged, the rest lowercased.
fn initial_uppercase(word: &[u16]) -> String {
    let (first, rest) = word.split_at(1.min(word.len()));
    let mut s = String::from_utf16_lossy(first);
    s.push_str(&unicode::lower_string(&String::from_utf16_lossy(rest)));
    s
}

/// `containsNoDigit`.
fn contains_no_digit(s: &[u16]) -> bool {
    !s.iter().any(|&c| unicode::is_digit(c))
}

/// `isAllUppercase`: no lowercase letter (characters without case are
/// ignored).
fn is_all_uppercase(s: &[u16]) -> bool {
    !s.iter()
        .any(|&c| unicode::is_letter(c) && unicode::is_lower_case(c))
}

/// `isNotAllLowercase`: at least one letter that is not lowercase.
fn is_not_all_lowercase(s: &[u16]) -> bool {
    s.iter()
        .any(|&c| unicode::is_letter(c) && !unicode::is_lower_case(c))
}

/// `isNotCapitalizedWord`: false only for an uppercase letter followed by no
/// other non-lowercase letter.
fn is_not_capitalized_word(s: &[u16]) -> bool {
    if !s.is_empty() && unicode::is_upper_case(s[0]) {
        return s[1..]
            .iter()
            .any(|&c| unicode::is_letter(c) && !unicode::is_lower_case(c));
    }
    true
}

/// `isMixedCase`.
fn is_mixed_case(s: &[u16]) -> bool {
    !is_all_uppercase(s) && is_not_capitalized_word(s) && is_not_all_lowercase(s)
}

/// `isCamelCase`.
fn is_camel_case(s: &[u16]) -> bool {
    !s.is_empty()
        && !is_all_uppercase(s)
        && is_not_capitalized_word(s)
        && unicode::is_upper_case(s[0])
        && (s.len() <= 1 || unicode::is_lower_case(s[1]))
        && is_not_all_lowercase(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn case_predicates() {
        assert!(is_all_uppercase(&u("MAISON")) && is_all_uppercase(&u("A-1")));
        assert!(!is_all_uppercase(&u("Maison")));
        assert!(!is_not_capitalized_word(&u("Maison")));
        assert!(is_not_capitalized_word(&u("maison")) && is_not_capitalized_word(&u("MaiSon")));
        assert!(is_mixed_case(&u("mAison")) && is_mixed_case(&u("MaiSon")));
        assert!(!is_mixed_case(&u("Maison")) && !is_mixed_case(&u("MAISON")));
        assert!(!is_mixed_case(&u("maison")));
        assert!(is_camel_case(&u("GrandÉléphant")) && !is_camel_case(&u("Grand")));
        assert!(!is_camel_case(&u("GRand")) && !is_camel_case(&u("grandÉléphant")));
        assert!(contains_no_digit(&u("abc")) && !contains_no_digit(&u("ab3")));
        assert_eq!(initial_uppercase(&u("ÉCOLE")), "École");
    }

    #[test]
    fn h_matrix_initial_state() {
        // Same values as morfologik.speller.HMatrix for distance 1.
        let h = HMatrix::new(1, 120);
        assert_eq!(h.get(0, 0), 0); // H(0,0)
        assert_eq!(h.get(1, 0), 1);
        assert_eq!(h.get(0, 1), 1);
        assert_eq!(h.get(2, 0), 2);
        assert_eq!(h.get(3, 3), 0); // inside the band: not initialised
        assert_eq!(h.get(2, 4), 2); // upper edge of the band
        assert_eq!(h.get(-1, -1), 0); // index wraps to the previous row, as in Java
        assert_eq!(h.get(500, 0), 2); // out of the array: too far
    }

    #[test]
    fn utf8_steps() {
        let dec = |bytes: &[u8]| {
            bytes
                .iter()
                .fold(PrevBytes::EMPTY, |p, &b| p.with(b))
                .decode_first_unit()
        };
        assert_eq!(dec(b"a"), Some(u16::from(b'a')));
        assert_eq!(dec(&[0xc3]), None);
        assert_eq!(dec(&[0xc3, 0xa9]), Some(0xe9));
        assert_eq!(dec(&[0xf0, 0x9f, 0x98]), None);
        assert_eq!(dec(&[0xf0, 0x9f, 0x98, 0x80]), Some(0xd83d));
        assert_eq!(dec(&[0x80]), None);
        assert_eq!(dec(&[0x80, 0x80, 0x80, 0x80, b'a']), None);
    }
}
