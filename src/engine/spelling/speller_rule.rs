//! The French spelling rule (`FR_SPELLING_RULE`).
//!
//! Port of `org.languagetool.rules.fr.MorfologikFrenchSpellerRule` and of
//! what it inherits from
//! `org.languagetool.rules.spelling.morfologik.MorfologikSpellerRule` and
//! `org.languagetool.rules.spelling.SpellingCheckRule` (LanguageTool,
//! LGPL-2.1-or-later).
//!
//! A token is checked when it is not tagged, not immunized, not marked
//! "ignore spelling" by the disambiguator or by a multi-word entry of the
//! word lists, and not a URL, an e-mail address or a token without Latin
//! letter. A misspelled token gets, in this order: the suggestions for a
//! wrongly placed space (with the previous or the next token), the curated
//! French suggestions (missing apostrophe or hyphen, `m2` for `m²`, glued
//! words), then the dictionary suggestions at edit distance 1, 2 if there are
//! none, 3 if there are still none, reordered by
//! `MorfologikFrenchSpellerRule.orderSuggestions`.
//!
//! Not ported, because nothing reaches them without a user configuration:
//! user dictionaries, the limit on the number of suggestions, the foreign
//! language detection, translations (English/German only) and the machine
//! learning suggestion rankers.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use super::super::checker::NativeRule;
use super::super::pattern::{SpellChecker, WordTagger};
use super::super::rule::{is_punctuation_mark, RuleContext, RuleInfo, RuleMatch};
use super::super::strtools;
use super::super::token::{Readings, Sentence};
use super::fsa_builder::fsa_from_lines;
use super::morfologik_speller::MorfologikMultiSpeller;
use super::strings::{
    is_latin_script, is_other_category, java_is_letter, java_split, remove_diacritics,
    split_camel_case, split_digits_at_end,
};
use super::urls::{is_email, is_url};
use super::wordlist::{load_words, read_resource, speller_lines};
use super::Tokenize;
use crate::morfologik::Dictionary;
use crate::re::{self, Flags, Regex};

/// `SpellingCheckRule.MAX_TOKEN_LENGTH`.
const MAX_TOKEN_LENGTH: usize = 200;
/// `MorfologikSpellerRule.MAX_FREQUENCY_FOR_SPLITTING`.
const MAX_FREQUENCY_FOR_SPLITTING: i32 = 21;
/// `messages.getString("spelling")` and `"desc_spelling_short"` in French.
const MESSAGE: &str = "Faute de frappe possible trouvée.";
const SHORT_MESSAGE: &str = "Faute de frappe";

const TOKEN_AT_START: [&str; 25] = [
    "non", "en", "a", "le", "la", "les", "pour", "de", "du", "des", "un", "une", "mon", "ma",
    "mes", "ton", "ta", "tes", "son", "sa", "ses", "leur", "leurs", "ce", "cet",
];
const PREFIX_WITH_WHITESPACE: [&str; 59] = [
    "agro",
    "anti",
    "archi",
    "auto",
    "aéro",
    "cardio",
    "co",
    "cyber",
    "demi",
    "ex",
    "extra",
    "géo",
    "hospitalo",
    "hydro",
    "hyper",
    "hypo",
    "infra",
    "inter",
    "macro",
    "mega",
    "meta",
    "mi",
    "micro",
    "mini",
    "mono",
    "multi",
    "musculo",
    "méga",
    "méta",
    "néo",
    "omni",
    "pan",
    "para",
    "pluri",
    "poly",
    "post",
    "prim",
    "pro",
    "proto",
    "pré",
    "pseudo",
    "psycho",
    "péri",
    "re",
    "retro",
    "ré",
    "semi",
    "simili",
    "socio",
    "super",
    "supra",
    "sus",
    "trans",
    "tri",
    "télé",
    "ultra",
    "uni",
    "vice",
    "éco",
];
const EXCEPTIONS_EGRAVE: [&str; 3] = ["burkinabè", "koinè", "épistémè"];
const SPLIT_DIGITS_AT_END: [&str; 7] = ["et", "ou", "de", "en", "à", "aux", "des"];

/// One of the curated suggestions of
/// `MorfologikFrenchSpellerRule.getAdditionalTopSuggestionsString`: when the
/// word matches `word`, and the part at `position` has a reading whose tag
/// matches `postag`, the two parts are joined with `separator`.
struct Split {
    word: Arc<Regex>,
    postag: Arc<Regex>,
    position: usize,
    separator: &'static str,
}

fn splits() -> &'static [Split] {
    static SPLITS: OnceLock<Vec<Split>> = OnceLock::new();
    SPLITS.get_or_init(|| {
        let split = |word: &str, postag: &str, position: usize, separator: &'static str| Split {
            word: re::compile(word, Flags::IGNORE_CASE).expect("valid pattern"),
            postag: re::compile(postag, Flags::default()).expect("valid pattern"),
            position,
            separator,
        };
        vec![
            // APOSTROF_INICI_VERBS / VERB_INDSUBJ, and so on in Java's order.
            split(
                "^([lnts])(h?[aeiouàéèíòóú].*[^è])$",
                "V .*(ind|sub).*",
                2,
                "'",
            ),
            split(
                "^(m)(h?[aeiouàéèíòóú].*[^è])$",
                "V .* [123] s|V .* [23] p",
                2,
                "'",
            ),
            split("^(c)([eiéèê].*)$", "V .* 3 s", 2, "'"),
            split(
                "^([lntsmd]|nous|vous)(h?[aeiouàéèíòóú].*[^è])$",
                "V.* inf",
                2,
                "'",
            ),
            split(
                "^([ld])(h?[aeiouàéèíòóú]...+)$",
                "[NJZ] .* (s|sp)|V .inf|V .*ppa.* s",
                2,
                "'",
            ),
            split(
                "^(d)(h?[aeiouàéèíòóú].+)$",
                "[NJZ] .* (p|sp)|V .*ppa.* p",
                2,
                "'",
            ),
            split(
                "^([\\p{L}]+)[’']?(moi|toi|le|la|lui|nous|vous|les|leur|y|en)$",
                "V.* imp .*",
                1,
                "-",
            ),
            split("^([\\p{L}]+[^e])[’']?(je)$", "V .*(ind).* 1 s", 1, "-"),
            split("^([\\p{L}]+)[’']?(tu)$", "V .*(ind).* 2 s", 1, "-"),
            split(
                "^([\\p{L}]+[^aeiou])[’']?(il|elle|ce|on)$",
                "V .*(ind).* 3 s",
                1,
                "-",
            ),
            split("^([\\p{L}]+)[’']?(nous)$", "V .*(ind).* 1 p", 1, "-"),
            split("^([\\p{L}]+)[’']?(vous)$", "V .*(ind).* 2 p", 1, "-"),
            split("^([\\p{L}]+)[’']?(ils|elles)$", "V .*(ind).* 3 p", 1, "-"),
        ]
    })
}

/// The multi-token entries of the word lists. LanguageTool turns each into a
/// `DisambiguationPatternRule` with the `IGNORE_SPELLING` action, made of
/// case-sensitive tokens, and applies them all to every sentence
/// (`Rule.getSentenceWithImmunization`); here they are indexed by their first
/// token, which gives the same marks without trying every entry.
#[derive(Default)]
struct IgnoredPhrases {
    /// First token -> the tokens that must follow it.
    by_first: HashMap<String, Vec<Box<[String]>>>,
}

impl IgnoredPhrases {
    fn add(&mut self, tokens: Vec<String>) {
        let mut it = tokens.into_iter();
        let Some(first) = it.next() else { return };
        self.by_first.entry(first).or_default().push(it.collect());
    }

    /// For each non-whitespace token: is it part of a multi-token entry?
    fn mark(&self, tokens: &[&Readings]) -> Vec<bool> {
        let mut ignored = vec![false; tokens.len()];
        if self.by_first.is_empty() {
            return ignored;
        }
        // A pattern token matches when one reading of the token has its text.
        let has_text = |t: &Readings, text: &str| {
            &*t.token == text || t.readings.iter().any(|r| &*r.token == text)
        };
        for i in 0..tokens.len() {
            let first = tokens[i];
            let mut check = |text: &str| {
                let Some(candidates) = self.by_first.get(text) else {
                    return;
                };
                for rest in candidates {
                    if i + rest.len() < tokens.len()
                        && rest
                            .iter()
                            .enumerate()
                            .all(|(k, text)| has_text(tokens[i + 1 + k], text))
                    {
                        for flag in &mut ignored[i..=i + rest.len()] {
                            *flag = true;
                        }
                    }
                }
            };
            check(&first.token);
            for r in &first.readings {
                if r.token != first.token {
                    check(&r.token);
                }
            }
        }
        ignored
    }
}

/// `MorfologikFrenchSpellerRule`.
pub struct FrenchSpellerRule {
    info: RuleInfo,
    /// `speller1`, `speller2`, `speller3`: edit distance 1, 2 and 3.
    speller1: MorfologikMultiSpeller,
    speller2: MorfologikMultiSpeller,
    speller3: MorfologikMultiSpeller,
    /// `wordsToBeIgnored`: the one-token entries of the word lists.
    words_to_be_ignored: HashSet<String>,
    /// `wordsToBeProhibited`.
    words_to_be_prohibited: HashSet<String>,
    /// `antiPatterns`: the multi-token entries of the word lists.
    phrases: IgnoredPhrases,
    tagger: Arc<dyn WordTagger>,
    tokenize: Tokenize,
    /// `convertsCase`: taken from the dictionary's metadata.
    converts_case: bool,
    /// `calcSpellerSuggestions` results, by word.
    suggestions: Mutex<HashMap<String, Arc<Vec<String>>>>,
}

/// A match being built by `getRuleMatches`.
struct Pending {
    from: usize,
    to: usize,
    suggestions: Vec<String>,
}

impl FrenchSpellerRule {
    /// Loads the rule from LanguageTool's French resource directory:
    /// `french.dict`/`french.info` (the dictionary named by
    /// `MorfologikFrenchSpellerRule.getFileName`, here already in memory as
    /// `dictionary`), `hunspell/ignore.txt`, `hunspell/spelling.txt`,
    /// `hunspell/spelling_custom.txt`, `spelling_global.txt`,
    /// `hunspell/prohibit.txt` and `hunspell/prohibit_custom.txt`.
    ///
    /// `tagger` stands for `FrenchTagger.INSTANCE` and `tokenize` for the
    /// language's word tokenizer.
    pub fn load(
        assets: &Path,
        dictionary: Arc<Dictionary>,
        tagger: Arc<dyn WordTagger>,
        tokenize: Tokenize,
    ) -> std::io::Result<FrenchSpellerRule> {
        let read = |name: &str| read_resource(&assets.join(name));
        let spelling = read("hunspell/spelling.txt");
        let spelling_custom = read("hunspell/spelling_custom.txt");
        let spelling_global = read("spelling_global.txt");

        // `SpellingCheckRule.init`: the entries to ignore, in file order.
        let ignore = read("hunspell/ignore.txt");
        let mut ignore_words: Vec<String> = Vec::new();
        for content in [&ignore, &spelling, &spelling_custom, &spelling_global] {
            ignore_words.extend(load_words(content.as_deref()));
        }

        // Building the word-list dictionary and tokenizing the entries to
        // ignore are independent: they run side by side.
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .clamp(1, 4);
        let chunk_size = ignore_words.len().div_ceil(workers).max(1);
        let (plain_text, tokenized) = std::thread::scope(|scope| {
            // `initSpeller`: the existing plain-text dictionaries, chained.
            let fsa = scope.spawn(|| {
                let mut chained = String::new();
                for content in [&spelling, &spelling_custom, &spelling_global]
                    .into_iter()
                    .flatten()
                {
                    chained.push_str(content);
                }
                let lines = speller_lines(&chained);
                if lines.is_empty() {
                    Ok(None)
                } else {
                    fsa_from_lines(&lines).map(Some)
                }
            });
            let tokenize = &tokenize;
            let handles: Vec<_> = ignore_words
                .chunks(chunk_size)
                .map(|part| {
                    scope.spawn(move || {
                        part.iter()
                            .map(|line| tokenize(line))
                            .collect::<Vec<Vec<String>>>()
                    })
                })
                .collect();
            let tokenized: Vec<Vec<String>> = handles
                .into_iter()
                .flat_map(|h| h.join().expect("tokenizer thread panicked"))
                .collect();
            (fsa.join().expect("dictionary thread panicked"), tokenized)
        });
        let plain_text =
            plain_text?.map(|fsa| Arc::new(Dictionary::new(fsa, dictionary.metadata.clone())));
        let speller1 = MorfologikMultiSpeller::new(dictionary.clone(), plain_text.clone(), 1);
        let speller2 = MorfologikMultiSpeller::new(dictionary.clone(), plain_text.clone(), 2);
        let speller3 = MorfologikMultiSpeller::new(dictionary, plain_text, 3);
        let converts_case = speller1.converts_case();

        let mut rule = FrenchSpellerRule {
            info: RuleInfo {
                id: "FR_SPELLING_RULE".to_string(),
                sub_id: None,
                description: "Faute de frappe possible".to_string(),
                category_id: "TYPOS".to_string(),
                category_name: "Faute de frappe possible".to_string(),
                issue_type: "misspelling".to_string(),
                ..RuleInfo::default()
            },
            speller1,
            speller2,
            speller3,
            words_to_be_ignored: HashSet::new(),
            words_to_be_prohibited: HashSet::new(),
            phrases: IgnoredPhrases::default(),
            tagger,
            tokenize,
            converts_case,
            suggestions: Mutex::new(HashMap::new()),
        };
        for (line, tokens) in ignore_words.into_iter().zip(tokenized) {
            rule.add_ignore_words(line, tokens);
        }
        for name in ["hunspell/prohibit.txt", "hunspell/prohibit_custom.txt"] {
            rule.words_to_be_prohibited
                .extend(load_words(read(name).as_deref()));
        }
        Ok(rule)
    }

    /// `addIgnoreWords`: an entry of several tokens becomes an "ignore
    /// spelling" pattern, an entry of one token an ignored word.
    /// `tokens` is the entry cut by the word tokenizer.
    fn add_ignore_words(&mut self, line: String, tokens: Vec<String>) {
        if tokens.len() > 1 {
            self.phrases.add(
                tokens
                    .into_iter()
                    .filter(|t| !super::strings::java_trim(t).is_empty())
                    .collect(),
            );
        } else {
            self.words_to_be_ignored.insert(line);
        }
    }

    pub fn info(&self) -> &RuleInfo {
        &self.info
    }

    /// `isMisspelled(String)`: unknown to the binary dictionary and to the
    /// word lists (the ignore list is not consulted).
    pub fn is_misspelled(&self, word: &str) -> bool {
        self.speller1.is_misspelled(word)
    }

    fn is_prohibited(&self, word: &str) -> bool {
        self.words_to_be_prohibited.contains(word)
    }

    /// `isIgnoredNoCase` (`ignoreWordsWithLength` is 0 for French).
    fn is_ignored_no_case(&self, word: &str) -> bool {
        self.words_to_be_ignored.contains(word)
            || (!strtools::is_mixed_case(word)
                && self.converts_case
                && self.words_to_be_ignored.contains(&word.to_lowercase()))
    }

    /// `ignoreWord(String)` of `SpellingCheckRule` and `MorfologikSpellerRule`.
    fn ignore_word(&self, word: &str) -> bool {
        if strtools::utf16_len(word) > MAX_TOKEN_LENGTH {
            return true;
        }
        // No letter of the Latin script.
        if !word.is_empty() && !word.chars().any(is_latin_script) {
            return true;
        }
        let ignored = match word.strip_suffix('.') {
            // e.g. word at end of sentence
            Some(without_dot) if !self.words_to_be_ignored.contains(word) => {
                self.is_ignored_no_case(without_dot)
            }
            _ => self.is_ignored_no_case(word),
        };
        ignored || is_speller_emoji(word)
    }

    /// `canBeIgnored`, `ignored` being the marks of the multi-token entries.
    fn can_be_ignored(&self, token: &Readings, ignored: bool) -> bool {
        token.is_sent_start
            || token.immunized
            || token.ignored_by_speller
            || ignored
            || is_url(&token.token)
            || is_email(&token.token)
            || (token.is_tagged() && !self.is_prohibited(&token.token))
            || self.ignore_word(&token.token)
    }

    /// `match(AnalyzedSentence)`.
    pub fn match_sentence(&self, rule: usize, sentence: &Sentence) -> Vec<RuleMatch> {
        let tokens: Vec<&Readings> = sentence.non_ws_tokens();
        let ignored = self.phrases.mark(&tokens);
        let mut matches: Vec<Pending> = Vec::new();
        let mut is_first_word = true;
        for (idx, token) in tokens.iter().enumerate() {
            if self.can_be_ignored(token, ignored[idx]) {
                if idx > 0 && is_first_word && !is_punctuation_mark(&token.token) {
                    is_first_word = false;
                }
                continue;
            }
            let start_pos = token.start_pos;
            let word: &str = &token.readings[0].token;
            let new_rule_idx = matches.len();
            if let Some(m) = self.get_rule_matches(word, start_pos, &mut matches, idx, &tokens) {
                matches.push(m);
            }
            if matches.len() > new_rule_idx {
                // Characters of the token that its readings do not have
                // (soft hyphens).
                let hidden_char_offset = token.token.len().saturating_sub(word.len());
                if hidden_char_offset > 0 {
                    for m in &mut matches[new_rule_idx..] {
                        if token.end_pos() < m.to {
                            continue;
                        }
                        m.to += hidden_char_offset;
                    }
                }
            }
            if is_first_word && !matches.is_empty() && idx < tokens.len() - 1 {
                // The suggestions of the sentence's first word are capitalised.
                let mut new_replacements: Vec<String> = Vec::new();
                for replacement in &matches[0].suggestions {
                    let replacement = if *replacement == replacement.to_lowercase() {
                        strtools::uppercase_first_char(replacement)
                    } else {
                        replacement.clone()
                    };
                    if !new_replacements.contains(&replacement) {
                        new_replacements.push(replacement);
                    }
                }
                matches[0].suggestions = new_replacements;
            }
            if idx > 0 && is_first_word && !is_punctuation_mark(&token.token) {
                is_first_word = false;
            }
        }
        let text: &str = &sentence.text;
        matches
            .into_iter()
            .filter(|m| {
                m.from < m.to
                    && text.is_char_boundary(m.from)
                    && m.to <= text.len()
                    && text.is_char_boundary(m.to)
            })
            .map(|m| RuleMatch {
                rule,
                from_pos: m.from,
                to_pos: m.to,
                pattern_from: m.from,
                pattern_to: m.to,
                message: MESSAGE.to_string(),
                short_message: SHORT_MESSAGE.to_string(),
                suggestions: m.suggestions,
                original_error_str: text[m.from..m.to].to_string(),
            })
            .collect()
    }

    /// `createWrongSplitMatch`: a match over two tokens; it replaces the
    /// previous match when that one started at the same place.
    fn create_wrong_split_match(
        so_far: &mut Vec<Pending>,
        pos: usize,
        covered_word: &str,
        suggestion1: &str,
        suggestion2: &str,
        prev_pos: usize,
    ) -> Pending {
        if so_far.last().is_some_and(|prev| prev.from == prev_pos) {
            so_far.pop();
        }
        Pending {
            from: prev_pos,
            to: pos + covered_word.len(),
            suggestions: vec![format!("{suggestion1} {suggestion2}").trim().to_string()],
        }
    }

    /// `getRuleMatches`: the match for one misspelled word, if any.
    fn get_rule_matches(
        &self,
        word: &str,
        start_pos: usize,
        so_far: &mut Vec<Pending>,
        idx: usize,
        tokens: &[&Readings],
    ) -> Option<Pending> {
        let speller = &self.speller1;
        let misspelled = |w: &str| speller.is_misspelled(w);
        let frequency = |w: &str| speller.frequency(w);
        if !misspelled(word) && !self.is_prohibited(word) {
            return None;
        }
        if so_far.last().is_some_and(|last| last.to > start_pos) {
            return None;
        }
        let mut rule_match: Option<Pending> = None;
        // to be added before/after the suggestion if there is a suggestion
        // for a split word
        let mut before_suggestion_str = String::new();
        let mut after_suggestion_str = String::new();
        let has_digit = |w: &str| w.chars().any(|c| c.is_ascii_digit());

        if idx > 0 && tokens[idx].whitespace_before {
            let prev_word: &str = &tokens[idx - 1].token;
            if !prev_word.is_empty()
                && !has_digit(prev_word)
                && frequency(prev_word) < MAX_FREQUENCY_FOR_SPLITTING
            {
                let prev_start_pos = tokens[idx - 1].start_pos;
                let (sugg1a, last) = split_last_char(prev_word);
                let sugg1b = format!("{last}{word}");
                if char_len(sugg1a) > 1
                    && char_len(&sugg1b) > 2
                    && !misspelled(sugg1a)
                    && !misspelled(&sugg1b)
                    && frequency(sugg1a) + frequency(&sugg1b) > frequency(prev_word)
                {
                    rule_match = Some(Self::create_wrong_split_match(
                        so_far,
                        start_pos,
                        word,
                        sugg1a,
                        &sugg1b,
                        prev_start_pos,
                    ));
                    before_suggestion_str = format!("{prev_word} ");
                }
                let (first, sugg2b) = split_first_char(word);
                let sugg2a = format!("{prev_word}{first}");
                if char_len(sugg2b) > 2 && !misspelled(&sugg2a) && !misspelled(sugg2b) {
                    match &mut rule_match {
                        None => {
                            if frequency(&sugg2a) + frequency(sugg2b) > frequency(prev_word) {
                                rule_match = Some(Self::create_wrong_split_match(
                                    so_far,
                                    start_pos,
                                    word,
                                    &sugg2a,
                                    sugg2b,
                                    prev_start_pos,
                                ));
                                before_suggestion_str = format!("{prev_word} ");
                            }
                        }
                        Some(m) => m
                            .suggestions
                            .push(format!("{sugg2a} {sugg2b}").trim().to_string()),
                    }
                }
                let sugg = format!("{prev_word}{word}");
                if word == word.to_lowercase() && !misspelled(&sugg) {
                    match &mut rule_match {
                        None => {
                            if frequency(&sugg) >= frequency(prev_word) {
                                rule_match = Some(Pending {
                                    from: prev_start_pos,
                                    to: start_pos + word.len(),
                                    suggestions: vec![sugg],
                                });
                                before_suggestion_str = format!("{prev_word} ");
                            }
                        }
                        Some(m) => m.suggestions.push(sugg),
                    }
                }
                if rule_match.is_some() && misspelled(prev_word) {
                    return rule_match;
                }
            }
        }
        if rule_match.is_none() && idx < tokens.len() - 1 && tokens[idx + 1].whitespace_before {
            let next_word: &str = &tokens[idx + 1].token;
            if !next_word.is_empty()
                && !has_digit(next_word)
                && frequency(next_word) < MAX_FREQUENCY_FOR_SPLITTING
            {
                let next_start_pos = tokens[idx + 1].start_pos;
                let (sugg1a, last) = split_last_char(word);
                let sugg1b = format!("{last}{next_word}");
                if char_len(sugg1a) > 1
                    && char_len(&sugg1b) > 2
                    && !misspelled(sugg1a)
                    && !misspelled(&sugg1b)
                    && frequency(sugg1a) + frequency(&sugg1b) > frequency(next_word)
                {
                    rule_match = Some(Self::create_wrong_split_match(
                        so_far,
                        next_start_pos,
                        next_word,
                        sugg1a,
                        &sugg1b,
                        start_pos,
                    ));
                    after_suggestion_str = format!(" {next_word}");
                }
                let (first, sugg2b) = split_first_char(next_word);
                let sugg2a = format!("{word}{first}");
                if char_len(sugg2b) > 2 && !misspelled(&sugg2a) && !misspelled(sugg2b) {
                    match &mut rule_match {
                        None => {
                            if frequency(&sugg2a) + frequency(sugg2b) > frequency(next_word) {
                                rule_match = Some(Self::create_wrong_split_match(
                                    so_far,
                                    next_start_pos,
                                    next_word,
                                    &sugg2a,
                                    sugg2b,
                                    start_pos,
                                ));
                                after_suggestion_str = format!(" {next_word}");
                            }
                        }
                        Some(m) => m
                            .suggestions
                            .push(format!("{sugg2a} {sugg2b}").trim().to_string()),
                    }
                }
                let sugg = format!("{word}{next_word}");
                if next_word == next_word.to_lowercase() && !misspelled(&sugg) {
                    match &mut rule_match {
                        None => {
                            if frequency(&sugg) >= frequency(next_word) {
                                rule_match = Some(Pending {
                                    from: start_pos,
                                    to: next_start_pos + next_word.len(),
                                    suggestions: vec![sugg],
                                });
                                after_suggestion_str = format!(" {next_word}");
                            }
                        }
                        Some(m) => m.suggestions.push(sugg),
                    }
                }
                if rule_match.is_some() && misspelled(next_word) {
                    return rule_match;
                }
            }
        }
        let mut rule_match = rule_match.unwrap_or(Pending {
            from: start_pos,
            to: start_pos + word.len(),
            suggestions: Vec::new(),
        });

        let mut prevent_further_suggestions = false;
        let mut clean_word = word;
        // A word glued to a number or a bullet: `pStartsWithNumbersBullets`
        // (`^(\d[\.,\d]*|\P{L}+)(.*)$`), unless it starts with characters of
        // `pStartsWithNumbersBulletsExceptions` (`^([\p{C}\-\$%&]+)(.*)$`).
        if let Some(split_at) = starts_with_numbers_bullets(word) {
            let is_exception = word
                .chars()
                .next()
                .is_some_and(|c| is_other_category(c) || matches!(c, '-' | '$' | '%' | '&'));
            if !is_exception {
                let (first_part, second_part) = word.split_at(split_at);
                let multitoken_is_misspelled =
                    (self.tokenize)(second_part).iter().any(|t| misspelled(t));
                if (!multitoken_is_misspelled || self.is_ignored_no_case(second_part))
                    && !self.is_prohibited(second_part)
                {
                    rule_match
                        .suggestions
                        .push(format!("{first_part} {second_part}"));
                    prevent_further_suggestions = true;
                } else {
                    before_suggestion_str = format!("{first_part} ");
                    clean_word = second_part;
                }
            }
        }
        if !prevent_further_suggestions {
            // `appendLazySuggestions` + `joinBeforeAfterSuggestions`.
            for suggestion in self.calc_speller_suggestions(clean_word).iter() {
                rule_match.suggestions.push(format!(
                    "{before_suggestion_str}{suggestion}{after_suggestion_str}"
                ));
            }
        }
        Some(rule_match)
    }

    /// `calcSpellerSuggestions` (cached by word).
    fn calc_speller_suggestions(&self, word: &str) -> Arc<Vec<String>> {
        if let Some(found) = self
            .suggestions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(word)
        {
            return found.clone();
        }
        let computed = Arc::new(self.compute_speller_suggestions(word));
        let mut cache = self.suggestions.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= 50_000 {
            cache.clear();
        }
        cache.insert(word.to_string(), computed.clone());
        computed
    }

    fn compute_speller_suggestions(&self, word: &str) -> Vec<String> {
        let words_of = |speller: &MorfologikMultiSpeller| -> Vec<String> {
            speller
                .weighted_suggestions_from_default_dicts(word)
                .iter()
                .map(|s| s.word.clone())
                .collect()
        };
        let mut default_suggestions = words_of(&self.speller1);
        let only_case_differs = default_suggestions
            .first()
            .is_some_and(|first| strtools::equals_ignore_case(word, first));
        let length = strtools::utf16_len(word);
        if length >= 3 && (only_case_differs || default_suggestions.is_empty()) {
            // speller1 uses a maximum edit distance of 1, it won't find
            // suggestion for "garentee", "greatful" etc.
            default_suggestions.extend(words_of(&self.speller2));
            if length >= 5 && default_suggestions.is_empty() {
                default_suggestions.extend(words_of(&self.speller3));
            }
        }
        let top_suggestions = self.additional_top_suggestions(word);
        default_suggestions.splice(0..0, top_suggestions);
        if default_suggestions.is_empty() {
            return Vec::new();
        }
        // `filterSuggestions`: no prohibited word, no repeat. (Its handling of
        // "Name s" needs the English proper noun tag and never applies.)
        let mut filtered: Vec<String> = Vec::with_capacity(default_suggestions.len());
        for suggestion in default_suggestions {
            if !self.is_prohibited(&suggestion) && !filtered.contains(&suggestion) {
                filtered.push(suggestion);
            }
        }
        order_suggestions(filtered, word)
    }

    /// `MorfologikFrenchSpellerRule.getAdditionalTopSuggestionsString`.
    fn additional_top_suggestions(&self, word: &str) -> Vec<String> {
        if word == "voulai" {
            return vec!["voulais".to_string(), "voulait".to_string()];
        }
        const UNITS: [(&str, &str); 10] = [
            ("mm2", "mm²"),
            ("cm2", "cm²"),
            ("dm2", "dm²"),
            ("m2", "m²"),
            ("km2", "km²"),
            ("mm3", "mm³"),
            ("cm3", "cm³"),
            ("dm3", "dm³"),
            ("m3", "m³"),
            ("km3", "km³"),
        ];
        for (written, unit) in UNITS {
            if strtools::equals_ignore_case(word, written) {
                return vec![unit.to_string()];
            }
        }
        let parts = split_camel_case(word);
        if parts.len() > 1
            && char_len(&parts[0]) > 1
            && parts.iter().all(|part| !self.speller1.is_misspelled(part))
        {
            return vec![parts.join(" ")];
        }
        let parts = split_digits_at_end(word);
        if parts.len() > 1
            && self.tagger.tag_word(&parts[0]).is_tagged()
            && (char_len(&parts[0]) > 2
                || SPLIT_DIGITS_AT_END.contains(&parts[0].to_lowercase().as_str()))
        {
            return vec![parts.join(" ")];
        }
        let mut new_suggestions = Vec::new();
        for split in splits() {
            new_suggestions.extend(self.find_suggestion(word, split, true));
        }
        new_suggestions
    }

    /// `findSuggestion`: `l'` + verb, verb + `-il`, etc.; when the part to
    /// tag is itself misspelled, its first suggestions are tried in its
    /// place.
    fn find_suggestion(&self, word: &str, split: &Split, recursive: bool) -> Vec<String> {
        let mut new_suggestions = Vec::new();
        let chars: Vec<char> = word.chars().collect();
        let Some(caps) = split.word.full_match_chars(&chars) else {
            return new_suggestions;
        };
        let group1 = caps.get(&chars, 1).unwrap_or_default();
        let group2 = caps.get(&chars, 2).unwrap_or_default();
        let new_suggestion = if split.position == 1 {
            &group1
        } else {
            &group2
        };
        let readings = self.tagger.tag_word(new_suggestion);
        // `matchPostagRegexp`: an untagged reading counts as "UNKNOWN".
        if readings
            .readings
            .iter()
            .any(|r| split.postag.is_full_match(r.pos().unwrap_or("UNKNOWN")))
        {
            new_suggestions.push(format!("{group1}{}{group2}", split.separator));
            return new_suggestions;
        }
        if recursive {
            let more = self.speller1.suggestions(new_suggestion);
            for (i, suggestion) in more.iter().enumerate() {
                let new_word = if split.position == 1 {
                    format!("{suggestion}{group2}")
                } else {
                    format!("{group1}{suggestion}")
                };
                new_suggestions.extend(self.find_suggestion(&new_word, split, false));
                if i > 5 {
                    break;
                }
            }
        }
        new_suggestions
    }

    /// `getSpellingSuggestions(String)`: the suggestions of the rule for one
    /// word checked alone (nothing for a word the rule accepts).
    pub fn spelling_suggestions(&self, word: &str) -> Vec<String> {
        let token = Readings::untagged(word, 0);
        let sentence = Sentence::new(Arc::from(word), vec![token], Vec::new());
        self.match_sentence(usize::MAX, &sentence)
            .into_iter()
            .next()
            .map(|m| m.suggestions)
            .unwrap_or_default()
    }
}

/// `MorfologikFrenchSpellerRule.orderSuggestions`.
fn order_suggestions(suggestions: Vec<String>, word: &str) -> Vec<String> {
    let mut new_suggestions: Vec<String> = Vec::with_capacity(suggestions.len());
    let word_without_diacritics = remove_diacritics(word);
    let total = suggestions.len();
    for (i, suggestion) in suggestions.into_iter().enumerate() {
        let lower = suggestion.to_lowercase();
        let parts = java_split(&lower, ' ');
        let first = parts.first().copied().unwrap_or("");
        // remove wrong split prefixes
        if parts.len() == 2 && PREFIX_WITH_WHITESPACE.contains(&first) {
            continue;
        }
        // remove: informè V
        if char_len(first) == 1 && !matches!(first, "a" | "à" | "y") {
            continue;
        }
        // remove: informè
        if lower.ends_with('è') && !EXCEPTIONS_EGRAVE.contains(&lower.as_str()) {
            continue;
        }
        // set at the top of the list the suggestions that only differ in
        // diacritics
        let mut pos_new_sugg = 0;
        while new_suggestions.len() > pos_new_sugg
            && strtools::equals_ignore_case(
                &remove_diacritics(&new_suggestions[pos_new_sugg]),
                &word_without_diacritics,
            )
        {
            pos_new_sugg += 1;
        }
        // move some split words to first place
        if parts.len() == 2 && TOKEN_AT_START.contains(&first) && char_len(parts[1]) > 1 {
            new_suggestions.insert(pos_new_sugg, suggestion);
            continue;
        }
        if strtools::equals_ignore_case(&word_without_diacritics, &remove_diacritics(&suggestion)) {
            new_suggestions.insert(pos_new_sugg, suggestion);
            continue;
        }
        // move words with apostrophe or hyphen to second position
        let clean_suggestion: String = suggestion
            .chars()
            .filter(|c| !matches!(c, '\'' | '-'))
            .collect();
        if i > 1 && total > 2 && strtools::equals_ignore_case(&clean_suggestion, word) {
            if pos_new_sugg == 0 {
                pos_new_sugg = 1;
            }
            new_suggestions.insert(pos_new_sugg.min(new_suggestions.len()), suggestion);
            continue;
        }
        new_suggestions.push(suggestion);
    }
    new_suggestions
}

/// `StringTools.isEmoji`: a token with characters outside the Basic
/// Multilingual Plane that is not made of letters, digits, punctuation and
/// spaces.
fn is_speller_emoji(word: &str) -> bool {
    if !word.chars().any(|c| c as u32 > 0xFFFF) {
        return false;
    }
    !word.chars().all(|c| {
        java_is_letter(c)
            || c.is_ascii_digit()
            || is_punctuation(c)
            || super::strings::is_space_separator(c)
    })
}

/// `\p{P}`, as far as a word can tell.
fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation() && !"$+<=>^`|~".contains(c)
        || matches!(c as u32, 0xA1 | 0xA7 | 0xAB | 0xB6 | 0xB7 | 0xBB | 0xBF | 0x2010..=0x2027 | 0x2030..=0x205E | 0x3001..=0x3003 | 0x3008..=0x3011)
}

/// The end of group 1 of `^(\d[\.,\d]*|\P{L}+)(.*)$` in `word`, when the
/// pattern matches.
fn starts_with_numbers_bullets(word: &str) -> Option<usize> {
    let first = word.chars().next()?;
    if word.contains(['\n', '\r']) {
        return None;
    }
    let end = if first.is_ascii_digit() {
        word.char_indices()
            .skip(1)
            .find(|(_, c)| !(c.is_ascii_digit() || matches!(c, '.' | ',')))
            .map(|(i, _)| i)
    } else if !java_is_letter(first) {
        word.char_indices()
            .find(|(_, c)| java_is_letter(*c))
            .map(|(i, _)| i)
    } else {
        return None;
    };
    Some(end.unwrap_or(word.len()))
}

fn char_len(s: &str) -> usize {
    strtools::utf16_len(s)
}

/// `(s.substring(0, s.length() - 1), s.substring(s.length() - 1))`.
fn split_last_char(s: &str) -> (&str, &str) {
    match s.char_indices().next_back() {
        Some((i, _)) => s.split_at(i),
        None => (s, ""),
    }
}

/// `(s.charAt(0), s.substring(1))`.
fn split_first_char(s: &str) -> (&str, &str) {
    match s.chars().next() {
        Some(c) => s.split_at(c.len_utf8()),
        None => (s, ""),
    }
}

impl SpellChecker for FrenchSpellerRule {
    fn is_misspelled(&self, word: &str) -> bool {
        FrenchSpellerRule::is_misspelled(self, word)
    }

    fn spelling_suggestions(&self, word: &str) -> Vec<String> {
        FrenchSpellerRule::spelling_suggestions(self, word)
    }
}

/// The rule as the checker runs it (the same object serves as the language's
/// default spelling rule, hence the shared pointer).
pub struct FrenchSpellerNativeRule(pub Arc<FrenchSpellerRule>);

impl NativeRule for FrenchSpellerNativeRule {
    fn info(&self) -> &RuleInfo {
        self.0.info()
    }

    fn match_sentence(
        &self,
        rule: usize,
        sentence: &Sentence,
        _ctx: &RuleContext,
    ) -> Vec<RuleMatch> {
        self.0.match_sentence(rule, sentence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(list: &[&str]) -> Vec<String> {
        list.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn order_of_french_suggestions() {
        // Suggestions that only differ by diacritics come first.
        assert_eq!(
            order_suggestions(s(&["près", "tués", "très"]), "tres"),
            s(&["très", "près", "tués"])
        );
        // "prefix word" splits and one-letter first parts are dropped.
        assert_eq!(
            order_suggestions(
                s(&["anti virus", "antivirus", "l antivirus", "a virus"]),
                "antivirrus"
            ),
            s(&["a virus", "antivirus"])
        );
        // A split after a determiner moves up.
        assert_eq!(
            order_suggestions(s(&["lamaison", "la maison"]), "lamaisson"),
            s(&["la maison", "lamaison"])
        );
        // A final è is not French.
        assert_eq!(
            order_suggestions(s(&["informè", "informé", "koinè"]), "informe"),
            s(&["informé", "koinè"])
        );
        // From the third suggestion on, a form with apostrophe or hyphen that
        // is otherwise the word goes to the second place.
        assert_eq!(
            order_suggestions(s(&["sale", "salué", "s'allie", "s'alle"]), "salle"),
            s(&["sale", "s'alle", "salué", "s'allie"])
        );
    }

    #[test]
    fn numbers_and_bullets() {
        assert_eq!(starts_with_numbers_bullets("2eme"), Some(1));
        assert_eq!(starts_with_numbers_bullets("1,5kg"), Some(3));
        assert_eq!(starts_with_numbers_bullets("•mot"), Some("•".len()));
        assert_eq!(starts_with_numbers_bullets("mot"), None);
        assert_eq!(starts_with_numbers_bullets("123"), Some(3));
    }

    #[test]
    fn multi_token_entries_mark_their_tokens() {
        let mut phrases = IgnoredPhrases::default();
        phrases.add(s(&["Joe", "Biden"]));
        phrases.add(s(&["Guillain", "-", "Barré"]));
        let tokens: Vec<Readings> = ["", "Joe", "Biden", "et", "Guillain", "-", "Barré", "Joe"]
            .iter()
            .enumerate()
            .map(|(i, t)| Readings::untagged(t, i))
            .collect();
        let refs: Vec<&Readings> = tokens.iter().collect();
        assert_eq!(
            phrases.mark(&refs),
            [false, true, true, false, true, true, true, false]
        );
    }

    #[test]
    fn words_checked_alone() {
        let Some(tools) = super::super::testing::tools() else {
            return;
        };
        let rule = &tools.spelling.rule;
        for known in ["maison", "Alexi", "Entra", "LanguageTool", ""] {
            assert!(!rule.is_misspelled(known), "{known}");
            assert!(rule.spelling_suggestions(known).is_empty(), "{known}");
        }
        let first = |word: &str, n: usize| {
            rule.spelling_suggestions(word)
                .into_iter()
                .take(n)
                .collect::<Vec<_>>()
        };
        assert!(rule.is_misspelled("connaisance"));
        assert_eq!(first("connaisance", 9), ["connaissance", "connaissances"]);
        // Morfologik's own ranking, quirks included.
        assert_eq!(
            first("annuyant", 9),
            [
                "ennuyant",
                "annulant",
                "ennuyait",
                "ennuyante",
                "ennuyants",
                "ennoyant",
                "ennuyât"
            ]
        );
        // A missing apostrophe or hyphen comes first.
        assert_eq!(
            first("lhomme", 9),
            ["l'homme", "homme", "Lomme", "chômé", "chôme"]
        );
        assert_eq!(first("Mangezle", 9), ["Mangez-le", "Mangez le"]);
        assert_eq!(first("dit'il", 9), ["dit-il", "DIT-il", "détail"]);
        // Through the suggestions of the part to tag ("urri" -> "erre").
        assert_eq!(first("Murri", 4), ["M'erre", "M'erra", "Mûri", "Muri"]);
        // Glued words and units.
        assert_eq!(first("BonjourMonde", 9), ["Bonjour Monde"]);
        assert_eq!(first("mot12", 9), ["mot 12", "Moto2"]);
        assert_eq!(first("m2", 3), ["M2", "m²", "mm"]);
        assert_eq!(first("voulai", 3), ["voulais", "voulait", "coulé"]);
        // Nothing at distance 1: distance 2.
        assert_eq!(first("Sonneborn", 9), ["Bonnebosq", "Bonnecourt"]);
        // An entry of several words of the word lists.
        assert_eq!(first("New Yrok Times", 9), ["New York Times"]);
    }

    #[test]
    fn sentences() {
        let Some(tools) = super::super::testing::tools() else {
            return;
        };
        let check = |text: &str| -> Vec<(String, Vec<String>)> {
            let sentence = tools.analyse(text);
            tools
                .spelling
                .rule
                .match_sentence(0, &sentence)
                .into_iter()
                .map(|m| {
                    (
                        m.original_error_str,
                        m.suggestions.into_iter().take(5).collect(),
                    )
                })
                .collect()
        };
        let m = |covered: &str, suggestions: &[&str]| (covered.to_string(), s(suggestions));
        assert_eq!(
            check("La connaisance est utile."),
            [m("connaisance", &["connaissance", "connaissances"])]
        );
        // The suggestions of the first word are capitalised.
        assert_eq!(
            check("connaisance utile pour tous."),
            [m("connaisance", &["Connaissance", "Connaissances"])]
        );
        assert_eq!(
            check("•Premierement il faut partir."),
            [m("Premierement", &["Premièrement"])]
        );
        assert_eq!(
            check("CONNAISANCE DES RISQUES."),
            [m("CONNAISANCE", &["CONNAISSANCE"])]
        );
        // A space in the wrong place: the match covers both tokens.
        assert_eq!(
            check("Il a un ordi nateur tout neuf."),
            [m(
                "ordi nateur",
                &[
                    "ordinateur",
                    "ordi batteur",
                    "ordi nageur",
                    "ordi dateur",
                    "ordi natter"
                ]
            )]
        );
        assert_eq!(
            check("Elle a pris le déj euner tôt."),
            [m("déj euner", &["déjeuner"])]
        );
        assert_eq!(
            check("Les enfa nts jouent dehors."),
            [m("enfa nts", &["enfants"])]
        );
        assert_eq!(
            check("C'est un texte avec im portant détail."),
            [m(
                "im portant",
                &[
                    "important",
                    "IM portant",
                    "il portant",
                    "km portant",
                    "cm portant"
                ]
            )]
        );
        // Multi-word entries of the word lists.
        assert_eq!(check("Nous avons vu Joe Biden et Entra hier."), []);
        // A word glued to a number.
        assert_eq!(
            check("Le 2eme jour, il a mangé 3pommes."),
            [
                m("2eme", &["2 été", "2 même", "2 me", "2 âme", "2 ère"]),
                m("3pommes", &["3 pommes"])
            ]
        );
        assert_eq!(
            check("Il habite à Paris en frrance depuis lontemps."),
            [
                m(
                    "frrance",
                    &["France", "franque", "errance", "francs", "franc"]
                ),
                m("lontemps", &["longtemps"])
            ]
        );
        assert_eq!(
            check("J'ai lu le New Yrok Times."),
            [m("Yrok", &["York", "Grok"])]
        );
    }

    #[test]
    fn tokens_that_are_not_checked() {
        let Some(tools) = super::super::testing::tools() else {
            return;
        };
        let rule = &tools.spelling.rule;
        let token = |text: &str| Readings::untagged(text, 0);
        assert!(!rule.can_be_ignored(&token("connaisance"), false));
        assert!(
            rule.can_be_ignored(&token("connaisance"), true),
            "part of a multi-word entry"
        );
        assert!(rule.can_be_ignored(&token("https://example.org/pagge"), false));
        assert!(rule.can_be_ignored(&token("jean.dupond@exemple.fr"), false));
        assert!(
            rule.can_be_ignored(&token("1234"), false),
            "no Latin letter"
        );
        assert!(
            rule.can_be_ignored(&token("Привет"), false),
            "no Latin letter"
        );
        assert!(rule.can_be_ignored(&token("AFPA"), false), "in ignore.txt");
        assert!(
            !rule.can_be_ignored(&token("afpa"), false),
            "the list has the word in capitals only"
        );
        assert!(
            rule.can_be_ignored(&token("Wesh"), false),
            "the list has the lowercase form"
        );
        let mut flagged = token("connaisance");
        flagged.ignored_by_speller = true;
        assert!(rule.can_be_ignored(&flagged, false));
        let mut immunized = token("connaisance");
        immunized.immunized = true;
        assert!(rule.can_be_ignored(&immunized, false));
    }

    #[test]
    fn character_helpers() {
        assert_eq!(split_last_char("école"), ("écol", "e"));
        assert_eq!(split_last_char("né"), ("n", "é"));
        assert_eq!(split_first_char("école"), ("é", "cole"));
        assert_eq!(split_first_char(""), ("", ""));
        assert!(!is_speller_emoji("mot"));
        assert!(is_speller_emoji("😂"));
    }
}
