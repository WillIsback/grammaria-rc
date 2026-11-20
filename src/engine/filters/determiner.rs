//! Port of `org.languagetool.rules.fr.WordWithDeterminerFilter`: for a
//! determiner (or adjective) and a word that disagree, proposes every
//! agreeing pair ("la homme" -> "l'homme", "les hommes"), keeping only the
//! pairs on which the elision rules find nothing to say.

use std::collections::HashMap;
use std::sync::Arc;

use super::{parse_int, rebuilt_match, tag_pattern, FilterEnv};
use crate::engine::rule::{RuleContext, RuleFilter, RuleMatch};
use crate::engine::strtools;
use crate::engine::token::{Readings, Sentence};

tag_pattern!(det_pattern, "(P.)?D .*|J .*|V.* ppa .*");
tag_pattern!(word_pattern, "[ZNJ] .*|V.* ppa .*");
tag_pattern!(ends_ms, ".+([me]) (s|sp)");
tag_pattern!(ends_fs, ".+([fe]) (s|sp)");
tag_pattern!(ends_mp, ".+([me]) (p|sp)");
tag_pattern!(ends_fp, ".+([fe]) (p|sp)");

/// Masculine singular, feminine singular, masculine plural, feminine plural.
const GENDER_NUMBER: [&str; 4] = ["([me]) (s|sp)", "([fe]) (s|sp)", "([me]) (p|sp)", "([fe]) (p|sp)"];
const DETERMINER: &str = "((P.)?D |J |V.* ppa )";
/// Forms of "beau", "fou", "mou", "nouveau" that are no determiners.
const EXCEPTIONS_DETERMINER: [&str; 4] = ["bels", "fols", "mols", "nouvels"];
/// A suggestion must not trigger a rule of this category...
pub(crate) const CATEGORY_TO_CHECK: &str = "CAT_ELISION";
/// ...nor one of these rules.
pub(crate) const RULES_TO_CHECK: [&str; 5] = ["CET_CE", "CE_CET", "MA_VOYELLE", "MON_NFS", "VIEUX"];

/// Upper bound of the remembered suggestion verdicts.
const MAX_VERDICTS: usize = 50_000;

pub(crate) struct WordWithDeterminerFilter {
    env: Arc<FilterEnv>,
}

impl WordWithDeterminerFilter {
    pub(crate) fn new(env: Arc<FilterEnv>) -> WordWithDeterminerFilter {
        WordWithDeterminerFilter { env }
    }

    /// `suggestionHasNoErrors`: analyse the suggestion as a text of its own
    /// and run the elision rules on it. Without an analyzer every suggestion
    /// passes.
    fn suggestion_has_no_errors(&self, suggestion: &str, ctx: &RuleContext) -> bool {
        if !self.env.has_analyzer() {
            return true;
        }
        if let Some(&verdict) = self.env.suggestion_verdicts.lock().unwrap_or_else(|e| e.into_inner()).get(suggestion) {
            return verdict;
        }
        let verdict = match self.env.analyze(suggestion, ctx) {
            Some(sentence) => !self
                .env
                .rules()
                .iter()
                .filter(|r| r.info.category_id == CATEGORY_TO_CHECK || RULES_TO_CHECK.contains(&r.info.id.as_str()))
                .any(|r| !r.match_sentence(usize::MAX, &sentence, ctx).is_empty()),
            None => true,
        };
        let mut verdicts = self.env.suggestion_verdicts.lock().unwrap_or_else(|e| e.into_inner());
        if verdicts.len() >= MAX_VERDICTS {
            verdicts.clear();
        }
        verdicts.insert(suggestion.to_string(), verdict);
        verdict
    }
}

impl RuleFilter for WordWithDeterminerFilter {
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
        let position = |key: &str| -> Option<usize> {
            let p = parse_int(args.get(key)?)?;
            (p >= 1 && p as usize <= pattern_tokens.len()).then(|| p as usize - 1)
        };
        let atr_word = pattern_tokens[position("wordFrom")?];
        let atr_determiner = pattern_tokens[position("determinerFrom")?];
        let is_determiner_capitalized = strtools::is_capitalized_word(&atr_determiner.token);
        let is_word_capitalized = strtools::is_capitalized_word(&atr_word.token);
        let is_determiner_all_upper = strtools::is_all_uppercase(&atr_determiner.token) && !strtools::equals_ignore_case(&atr_determiner.token, "L'");
        let is_word_all_upper = strtools::is_all_uppercase(&atr_word.token);
        // LanguageTool fails on a sentence where either reading is missing.
        let at_determiner = det_pattern().find(atr_determiner)?;
        let at_word = word_pattern().find(atr_word)?;
        let word_tag = at_word.pos().unwrap_or("");
        let is_noun = word_tag.starts_with('N') || word_tag.starts_with('Z');
        let is_adjective = word_tag.starts_with('J');
        let prefix = match (is_noun, is_adjective) {
            (true, false) => "[NZ] ",
            (false, true) => "J ",
            _ => "[ZNJ] ",
        };
        let ends = [ends_ms(), ends_fs(), ends_mp(), ends_fp()];
        let mut replacements: Vec<String> = Vec::new();
        for i in 0..4 {
            let (mut determiner_forms, mut word_forms) = match &ctx.lang.synthesizer {
                Some(synth) => (
                    synth.synthesize_regexp(at_determiner, &format!("{DETERMINER}{}", GENDER_NUMBER[i]), true),
                    synth.synthesize_regexp(at_word, &format!("{prefix}{}", GENDER_NUMBER[i]), true),
                ),
                None => (Vec::new(), Vec::new()),
            };
            // A word the synthesizer does not know is kept in the gender and
            // number it already has.
            if determiner_forms.is_empty() && ends[i].matches_tag(at_determiner) {
                determiner_forms.push(at_determiner.token.to_string());
            }
            if word_forms.is_empty() && ends[i].matches_tag(at_word) {
                word_forms.push(at_word.token.to_string());
            }
            for word_form in &word_forms {
                for determiner_form in &determiner_forms {
                    if EXCEPTIONS_DETERMINER.contains(&determiner_form.as_str()) {
                        continue;
                    }
                    let mut determiner = determiner_form.clone();
                    let mut word = word_form.clone();
                    if is_determiner_capitalized {
                        determiner = strtools::uppercase_first_char(&determiner);
                    }
                    if is_word_capitalized {
                        word = strtools::uppercase_first_char(&word);
                    }
                    if is_determiner_all_upper {
                        determiner = determiner.to_uppercase();
                    }
                    if is_word_all_upper {
                        word = word.to_uppercase();
                    }
                    let r = format!("{determiner} {word}").replace("' ", "'");
                    if !replacements.contains(&r) && self.suggestion_has_no_errors(&r, ctx) {
                        // The pair that keeps the word as written comes first.
                        if r.ends_with(&*at_word.token) {
                            replacements.insert(0, r);
                        } else {
                            replacements.push(r);
                        }
                    }
                }
            }
        }
        let mut rule_match = rebuilt_match(&m, sentence, m.from_pos, m.to_pos, m.message.clone());
        let mut all = m.suggestions;
        all.extend(replacements);
        if !all.is_empty() {
            rule_match.suggestions = all;
        }
        Some(rule_match)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::filters::testing::*;
    use crate::engine::filters::FrenchFilters;

    fn suggest(determiner: (&str, &[(&str, &str)]), word: (&str, &[(&str, &str)])) -> Option<Vec<String>> {
        let ctx = context()?;
        let filter = WordWithDeterminerFilter::new(FrenchFilters::new().env.clone());
        let s = sentence(&[determiner, word]);
        let tokens = s.non_ws_tokens();
        let m = match_over(&s, 1, 2, "");
        let a = args(&[("wordFrom", "2"), ("determinerFrom", "1")]);
        Some(filter.accept(m, &a, 1, &tokens[1..], &[1, 1], &s, &ctx)?.suggestions)
    }

    #[test]
    fn proposes_the_agreeing_pairs() {
        // Without an analyzer nothing weeds out the elided article.
        let Some(found) = suggest(("Les", &[("D e p", "le")]), ("voiture", &[("N f s", "voiture")])) else { return };
        assert_eq!(found, ["La voiture", "L'voiture", "Les voitures"]);
        let Some(found) = suggest(("un", &[("D m s", "un")]), ("vaches", &[("N f p", "vache")])) else { return };
        assert_eq!(found, ["des vaches", "une vache"]);
    }

    #[test]
    fn keeps_the_case_of_both_words() {
        let Some(found) = suggest(("ses", &[("D e p", "son")]), ("Considération", &[("N f s", "considération")])) else { return };
        assert!(found.contains(&"ses Considérations".to_string()), "{found:?}");
        let Some(found) = suggest(("LES", &[("D e p", "le")]), ("VOITURE", &[("N f s", "voiture")])) else { return };
        assert_eq!(found, ["LA VOITURE", "L'VOITURE", "LES VOITURES"]);
    }

    #[test]
    fn a_word_without_a_noun_or_adjective_reading_rejects_the_match() {
        let Some(ctx) = context() else { return };
        let filter = WordWithDeterminerFilter::new(FrenchFilters::new().env.clone());
        let s = sentence(&[("les", &[("D e p", "le")]), ("xyz", &[])]);
        let tokens = s.non_ws_tokens();
        let a = args(&[("wordFrom", "2"), ("determinerFrom", "1")]);
        assert!(filter.accept(match_over(&s, 1, 2, ""), &a, 1, &tokens[1..], &[1, 1], &s, &ctx).is_none());
    }
}
