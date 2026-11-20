//! Port of `org.languagetool.rules.AbstractCompoundRule` with its data
//! (`CompoundRuleData`) and of the French `org.languagetool.rules.fr.CompoundRule`:
//! words that are written with a hyphen or as one word (`arc en ciel`).
//!
//! `compounds.txt` lists the compounds with their hyphens; the text is
//! searched for their spellings with spaces (or with only some of the
//! hyphens). Each match is reported under an id of its own,
//! `FR_COMPOUNDS_ARC_EN_CIEL` ([`SpecificIdRule`]), whose description names
//! the compound as listed (Java: as written in the text).
//!
//! A suggestion is only made when the tagger knows the word
//! (`RuleContext::lang.tagger`); without a tagger every suggestion is kept.

use std::collections::HashSet;

use super::super::checker::NativeRule;
use super::super::rule::{RuleContext, RuleInfo, RuleMatch};
use super::super::strtools;
use super::super::token::{Sentence, SENTENCE_START};
use super::{is_java_digit, java_split, java_trim, rule_info, rule_match, to_id, SpecificIdRule, SpecificIds, MISC};

/// `AbstractCompoundRule.MAX_TERMS`: the most parts a compound may have.
const MAX_TERMS: usize = 5;

/// `CompoundRuleData`: the compounds of a `compounds.txt`, spelt with spaces.
#[derive(Default)]
pub struct CompoundRuleData {
    incorrect_compounds: HashSet<String>,
    /// Compounds that may be written as one word.
    joined_suggestion: HashSet<String>,
    /// Among those, the ones whose inner parts lose their capital.
    joined_lower_case_suggestion: Vec<String>,
    /// Compounds that may be written with hyphens.
    dash_suggestion: HashSet<String>,
    /// The lower-cased first word of every compound.
    first_words: HashSet<String>,
    /// Some compound holds `\d`, standing for any number.
    has_digit_patterns: bool,
    /// The compounds in file order (the ids of the rule are built from them).
    entries: Vec<String>,
}

impl CompoundRuleData {
    /// Reads `compounds.txt`: one compound per line, with its hyphens, and
    /// a last character telling what to suggest: `+` one word, `*` hyphens,
    /// `?` one word without inner capitals, `$` both of the latter, nothing
    /// for one word or hyphens.
    pub fn parse(text: &str) -> Result<CompoundRuleData, String> {
        let mut data = CompoundRuleData::default();
        for line in text.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = java_trim(line.split('#').next().unwrap_or(""));
            let line = line.replace('-', " ");
            let parts = java_split(&line, ' ');
            if parts.len() == 1 {
                return Err(format!("Not a compound: {line}"));
            }
            if parts.len() > MAX_TERMS {
                return Err(format!("Too many compound parts: {line}, maximum allowed: {MAX_TERMS}"));
            }
            if data.incorrect_compounds.contains(&line.to_lowercase()) {
                return Err(format!("Duplicated word: {line}"));
            }
            let (compound, joined, dash, joined_lower) = match line.chars().next_back() {
                Some('+') => (&line[..line.len() - 1], true, false, false),
                Some('*') => (&line[..line.len() - 1], false, true, false),
                Some('?') => (&line[..line.len() - 1], true, false, true),
                Some('$') => (&line[..line.len() - 1], true, true, true),
                _ => (&line[..], true, true, false),
            };
            if joined {
                data.joined_suggestion.insert(compound.to_string());
            }
            if dash {
                data.dash_suggestion.insert(compound.to_string());
            }
            if joined_lower {
                data.joined_lower_case_suggestion.push(compound.to_string());
            }
            if data.incorrect_compounds.insert(compound.to_string()) {
                data.entries.push(compound.to_string());
            }
            if let Some(space) = compound.find(' ').filter(|&i| i > 0) {
                let first_word = compound[..space].to_lowercase();
                if !first_word.contains("\\d") {
                    data.first_words.insert(first_word);
                }
            }
            if compound.contains("\\d") {
                data.has_digit_patterns = true;
            }
        }
        Ok(data)
    }
}

pub struct CompoundRule {
    info: RuleInfo,
    data: CompoundRuleData,
    ids: SpecificIds,
    with_hyphen_message: &'static str,
    without_hyphen_message: &'static str,
    with_or_without_hyphen_message: &'static str,
    short_message: &'static str,
    /// The first word of a sentence is looked up without its capital.
    sentence_starts_with_upper_case: bool,
}

/// A token waiting in the queue of the last [`MAX_TERMS`] tokens.
#[derive(Clone, Copy)]
struct Queued<'a> {
    token: &'a str,
    whitespace_before: bool,
    sentence_start: bool,
    start_pos: usize,
    end_pos: usize,
}

impl CompoundRule {
    /// The French rule (`FR_COMPOUNDS`) over the text of `compounds.txt`,
    /// and the rules its matches are reported under.
    pub fn french(compounds: &str) -> Result<(CompoundRule, Vec<SpecificIdRule>), String> {
        let data = CompoundRuleData::parse(compounds)?;
        // `$match` stands for the compound in the description of each id.
        let info = rule_info("FR_COMPOUNDS", "Mots avec trait d’union : $match", &MISC, "misspelling");
        let keys = data.entries.iter().map(|compound| {
            (compound.clone(), to_id(&format!("{}_{}", info.id, compound)), info.description.replace("$match", compound))
        });
        let (ids, rules) = SpecificIds::new(keys, &info);
        let rule = CompoundRule {
            info,
            data,
            ids,
            with_hyphen_message: "Écrivez avec un trait d’union.",
            without_hyphen_message: "Écrivez avec un mot seul sans espace ni trait d’union.",
            with_or_without_hyphen_message: "Écrivez avec un mot seul ou avec trait d’union.",
            short_message: "Erreur de trait d’union",
            sentence_starts_with_upper_case: true,
        };
        Ok((rule, rules))
    }

    /// `sentenceMayContainCompound`: some token of the sentence starts a
    /// compound.
    fn sentence_may_contain_compound(&self, sentence: &Sentence) -> bool {
        // The keys are the lower-cased tokens of the sentence.
        sentence.token_offsets.keys().any(|token| {
            if token.is_empty() {
                return false;
            }
            if !token.contains('-') {
                let first = token.find(' ').filter(|&i| i > 0).map_or(&token[..], |i| &token[..i]);
                return self.data.first_words.contains(first);
            }
            let normalized = normalize(token);
            let first = normalized.find(' ').filter(|&i| i > 0).map_or(&normalized[..], |i| &normalized[..i]);
            !first.is_empty() && self.data.first_words.contains(&first.to_lowercase())
        })
    }

    /// `couldStartCompound`: the first real token of the queue starts a
    /// compound, or is itself made of several words.
    fn could_start_compound(&self, queue: &[Queued]) -> bool {
        let Some(first) = queue.iter().find(|q| !q.token.is_empty()) else { return true };
        let token = first.token;
        if !token.contains(['-', ' ']) {
            if is_lower_case(token) {
                return self.data.first_words.contains(token);
            }
            return self.data.first_words.contains(&token.to_lowercase());
        }
        let normalized = normalize(token);
        normalized.is_empty() || normalized.contains(' ') || self.data.first_words.contains(&normalized.to_lowercase())
    }

    /// `isMisspelled` of the French rule: the tagger does not know the word.
    fn is_misspelled(&self, word: &str, ctx: &RuleContext) -> bool {
        match &ctx.lang.tagger {
            Some(tagger) => !tagger.tag_word(word).is_tagged(),
            None => false,
        }
    }

    /// `filterReplacements`: drop the suggestions equal to the text and the
    /// misspelt ones.
    fn filter_replacements(&self, replacements: Vec<String>, original: &str, ctx: &RuleContext) -> Vec<String> {
        let mut kept = Vec::with_capacity(replacements.len());
        for replacement in replacements {
            let replacement = collapse_dashes(&replacement);
            if replacement != original && !self.is_misspelled(&replacement, ctx) {
                kept.push(replacement);
            }
        }
        kept
    }
}

/// Is `s` its own lower case?
fn is_lower_case(s: &str) -> bool {
    s.chars().all(|c| {
        let mut lower = c.to_lowercase();
        lower.next() == Some(c) && lower.next().is_none()
    })
}

/// `normalize`: hyphens and runs of whitespace become one space.
fn normalize(s: &str) -> String {
    let s = java_trim(s).replace(" - ", " ").replace('-', " ");
    let mut out = String::with_capacity(s.len());
    let mut in_space = false;
    for c in s.chars() {
        if matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r') {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// `--+` to `-`.
fn collapse_dashes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.chars() {
        if c == '-' && prev_dash {
            continue;
        }
        prev_dash = c == '-';
        out.push(c);
    }
    out
}

/// `StringUtils.uncapitalize`.
fn uncapitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) if first.is_uppercase() => {
            let mut lower = first.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(l), None) => format!("{l}{}", chars.as_str()),
                _ => s.to_string(),
            }
        }
        _ => s.to_string(),
    }
}

/// `isNotAllUppercase`: no word of the text is in capitals (a word without
/// letters counts as capitals, a lone hyphen does not).
fn is_not_all_uppercase(s: &str) -> bool {
    java_split(s, ' ').into_iter().all(|part| part == "-" || !strtools::is_all_uppercase(part))
}

/// `mergeCompound`: the parts written as one word.
fn merge_compound(s: &str, uncapitalize_mid_words: bool) -> String {
    let spaced = s.replace('-', " ");
    let mut out = String::with_capacity(s.len());
    for (k, part) in java_split(&spaced, ' ').into_iter().enumerate() {
        if k > 0 && uncapitalize_mid_words {
            out.push_str(&uncapitalize(part));
        } else {
            out.push_str(part);
        }
    }
    out
}

/// `StringUtils.isNumeric`.
fn is_numeric(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_java_digit)
}

/// `\d+` to the text `\d+`.
fn digits_to_pattern(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_digits = false;
    for c in s.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                out.push_str("\\d+");
            }
            in_digits = true;
        } else {
            out.push(c);
            in_digits = false;
        }
    }
    out
}

fn add_to_queue<'a>(queue: &mut Vec<Queued<'a>>, token: Queued<'a>) {
    if queue.len() == MAX_TERMS {
        queue.remove(0);
    }
    queue.push(token);
}

impl NativeRule for CompoundRule {
    fn info(&self) -> &RuleInfo {
        &self.info
    }

    fn match_sentence(&self, rule: usize, sentence: &Sentence, ctx: &RuleContext) -> Vec<RuleMatch> {
        let data = &self.data;
        let use_first_words = !data.has_digit_patterns;
        if use_first_words && !self.sentence_may_contain_compound(sentence) {
            return Vec::new();
        }
        let mut matches: Vec<RuleMatch> = Vec::new();
        let n = sentence.len();
        let text: &str = &sentence.text;
        // Start of the previous match, reported or not.
        let mut prev_match_from: Option<usize> = None;
        let mut queue: Vec<Queued> = Vec::with_capacity(MAX_TERMS);
        let mut strings_to_check: Vec<String> = Vec::with_capacity(MAX_TERMS);
        let mut orig_strings_to_check: Vec<String> = Vec::with_capacity(MAX_TERMS);
        let mut ends: Vec<usize> = Vec::with_capacity(MAX_TERMS);
        // The queue is flushed with empty tokens after the last one.
        for i in 0..n + MAX_TERMS {
            let token = if i >= n {
                let start_pos = queue.first().map_or(0, |q| q.start_pos);
                Queued { token: "", whitespace_before: false, sentence_start: false, start_pos, end_pos: start_pos }
            } else {
                let t = sentence.tok(i);
                if i > 0 && t.immunized {
                    continue;
                }
                Queued {
                    token: &t.token,
                    whitespace_before: t.whitespace_before,
                    sentence_start: t.has_pos_tag(SENTENCE_START),
                    start_pos: t.start_pos,
                    end_pos: t.end_pos(),
                }
            };
            if i == 0 {
                add_to_queue(&mut queue, token);
                continue;
            }
            let first_match_token = queue[0];
            if use_first_words && !self.could_start_compound(&queue) {
                add_to_queue(&mut queue, token);
                continue;
            }
            // The texts that start at the head of the queue: one per token,
            // without hyphens and as written.
            strings_to_check.clear();
            orig_strings_to_check.clear();
            ends.clear();
            let mut sb = String::new();
            let is_first_sent_start = queue[0].sentence_start;
            for (j, q) in queue.iter().enumerate() {
                if q.whitespace_before {
                    sb.push(' ');
                }
                sb.push_str(q.token);
                if j >= 1 || !is_first_sent_start {
                    let mut string_to_check = normalize(&sb);
                    if self.sentence_starts_with_upper_case && is_first_sent_start {
                        string_to_check = uncapitalize(&string_to_check);
                    }
                    strings_to_check.push(string_to_check);
                    orig_strings_to_check.push(java_trim(&sb).to_string());
                    ends.push(q.end_pos);
                }
            }
            for k in (0..strings_to_check.len()).rev() {
                let string_to_check = &strings_to_check[k];
                let orig_string_to_check = &orig_strings_to_check[k];
                let contains_digits = data.has_digit_patterns && java_split(string_to_check, ' ').into_iter().any(is_numeric);
                let listed = data.incorrect_compounds.contains(string_to_check);
                // A compound listed with `\d` for its numbers.
                let digits_pattern = if !listed && contains_digits { Some(digits_to_pattern(string_to_check)) } else { None };
                let listed_as_pattern = digits_pattern.as_ref().is_some_and(|p| data.incorrect_compounds.contains(p));
                if !listed && !listed_as_pattern {
                    continue;
                }
                // The first token at which the text read like this.
                let end_pos = ends[strings_to_check.iter().position(|s| s == string_to_check).unwrap_or(k)];
                let dash = data.dash_suggestion.contains(string_to_check);
                if dash && !orig_string_to_check.contains(' ') {
                    break; // already written with its hyphens
                }
                let mut msg = "";
                let mut replacement: Vec<String> = Vec::new();
                if dash || listed_as_pattern {
                    replacement.push(orig_string_to_check.replace(' ', "-"));
                    msg = self.with_hyphen_message;
                }
                if is_not_all_uppercase(orig_string_to_check) && data.joined_suggestion.contains(string_to_check) {
                    let uncapitalize_mid_words = data.joined_lower_case_suggestion.iter().any(|s| string_to_check.contains(s.as_str()));
                    replacement.push(merge_compound(orig_string_to_check, uncapitalize_mid_words));
                    msg = self.without_hyphen_message;
                }
                let parts = java_split(string_to_check, ' ');
                if parts.first().is_some_and(|p| strtools::utf16_len(p) == 1) {
                    replacement.clear();
                    replacement.push(orig_string_to_check.replace(' ', "-"));
                    msg = self.with_hyphen_message;
                } else if replacement.is_empty() || replacement.len() == 2 {
                    msg = self.with_or_without_hyphen_message;
                }
                let start_pos = first_match_token.start_pos;
                let replacement = self.filter_replacements(replacement, &text[start_pos..end_pos], ctx);
                if replacement.is_empty() {
                    break;
                }
                let key = digits_pattern.as_ref().filter(|_| !listed).unwrap_or(string_to_check);
                let Some(rule_match) = rule_match(self.ids.rule_index(rule, key), start_pos, end_pos, msg, self.short_message, replacement) else {
                    break;
                };
                // Of the matches that start at the same place only the first
                // is reported.
                if prev_match_from != Some(start_pos) {
                    matches.push(rule_match);
                }
                prev_match_from = Some(start_pos);
                break;
            }
            add_to_queue(&mut queue, token);
        }
        matches
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::super::pattern::WordTagger;
    use super::super::super::token::{Reading, Readings};
    use super::super::testing::{context, plain};
    use super::*;

    const DATA: &str = "# comment\narc-en-ciel*\nporte-feuille+\nrond-point\nHaut-Rhin*\nx-ray*\nwebcam-espion? # note\nvis-à-vis*\n";

    /// A tagger that knows a few words, capitalised or not.
    struct Known;

    impl WordTagger for Known {
        fn tag_word(&self, word: &str) -> Readings {
            let known = ["arc-en-ciel", "portefeuille", "rond-point", "Haut-Rhin", "x-ray", "vis-à-vis"];
            let lower = strtools::lowercase_first_char(word);
            let tag = if known.contains(&word) || known.contains(&lower.as_str()) { Some("N m s") } else { None };
            Readings::new(vec![Reading::from_strs(word, tag, tag.map(|_| word))], 0)
        }
    }

    fn check(text: &str) -> Vec<(String, usize, usize, Vec<String>)> {
        let (rule, ids) = CompoundRule::french(DATA).unwrap();
        let mut ctx = context();
        ctx.lang.tagger = Some(Arc::new(Known));
        // The rule sits at index 10, its id rules right after it.
        rule.match_sentence(10, &plain(text), &ctx)
            .into_iter()
            .map(|m| (ids[m.rule - 11].info().id.clone(), m.from_pos, m.to_pos, m.suggestions))
            .collect()
    }

    #[test]
    fn data_file() {
        let data = CompoundRuleData::parse(DATA).unwrap();
        assert!(data.incorrect_compounds.contains("arc en ciel"));
        assert!(data.dash_suggestion.contains("arc en ciel") && !data.joined_suggestion.contains("arc en ciel"));
        assert!(data.joined_suggestion.contains("porte feuille") && !data.dash_suggestion.contains("porte feuille"));
        assert!(data.joined_suggestion.contains("rond point") && data.dash_suggestion.contains("rond point"));
        assert_eq!(data.joined_lower_case_suggestion, vec!["webcam espion".to_string()]);
        assert!(data.first_words.contains("haut") && data.first_words.contains("arc"));
        assert!(!data.has_digit_patterns);
        assert!(CompoundRuleData::parse("mot\n").is_err());
        assert!(CompoundRuleData::parse("a-b-c-d-e-f\n").is_err());
    }

    #[test]
    fn compounds_written_apart() {
        assert_eq!(
            check("Un arc en ciel est là."),
            vec![("FR_COMPOUNDS_ARC_EN_CIEL".to_string(), 3, 14, vec!["arc-en-ciel".to_string()])]
        );
        // Some of the hyphens are there.
        assert_eq!(
            check("Un arc-en ciel est là."),
            vec![("FR_COMPOUNDS_ARC_EN_CIEL".to_string(), 3, 14, vec!["arc-en-ciel".to_string()])]
        );
        assert_eq!(
            check("Mon porte feuille."),
            vec![("FR_COMPOUNDS_PORTE_FEUILLE".to_string(), 4, 17, vec!["portefeuille".to_string()])]
        );
        // "rondpoint" is not a word: only the hyphenated spelling is left.
        assert_eq!(check("Le rond point."), vec![("FR_COMPOUNDS_ROND_POINT".to_string(), 3, 13, vec!["rond-point".to_string()])]);
        assert!(check("Un arc-en-ciel et un rond-point.").is_empty());
    }

    #[test]
    fn case_and_sentence_start() {
        // Compounds are listed with their capitals.
        assert_eq!(check("Le Haut Rhin."), vec![("FR_COMPOUNDS_HAUT_RHIN".to_string(), 3, 12, vec!["Haut-Rhin".to_string()])]);
        assert!(check("Le haut rhin.").is_empty());
        // The first word of the sentence is looked up in lower case; the
        // suggestion keeps what was written.
        assert_eq!(check("Arc en ciel."), vec![("FR_COMPOUNDS_ARC_EN_CIEL".to_string(), 0, 11, vec!["Arc-en-ciel".to_string()])]);
        assert!(check("Voici Arc en ciel.").is_empty());
        // A one-letter first part: hyphens only.
        assert_eq!(check("Un x ray."), vec![("FR_COMPOUNDS_X_RAY".to_string(), 3, 8, vec!["x-ray".to_string()])]);
    }

    #[test]
    fn helpers() {
        assert_eq!(normalize(" arc - en-ciel  bleu "), "arc en ciel bleu");
        assert_eq!(collapse_dashes("a--b---c-d"), "a-b-c-d");
        assert_eq!(merge_compound("Web-Cam Espion", true), "Webcamespion");
        assert_eq!(merge_compound("Web-Cam Espion", false), "WebCamEspion");
        assert!(is_not_all_uppercase("arc - en ciel") && !is_not_all_uppercase("ARC en ciel") && !is_not_all_uppercase("arc 12"));
        assert_eq!(digits_to_pattern("les 12 et 3"), "les \\d+ et \\d+");
        assert_eq!(uncapitalize("Été chaud"), "été chaud");
    }
}
