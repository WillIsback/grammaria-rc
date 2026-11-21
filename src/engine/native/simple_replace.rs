//! Port of `org.languagetool.rules.AbstractSimpleReplaceRule` with its data
//! loader (`SimpleReplaceDataLoader`) and of the French
//! `org.languagetool.rules.fr.SimpleReplaceRule`: unknown words with a
//! listed replacement (`replace.txt`).
//!
//! The French rule ignores the case of the word, skips the words the tagger
//! knows and does not look at lemmas (`setCheckLemmas(false)`: that branch
//! of the Java class is not ported). Each match is reported under an id of
//! its own, `FR_SIMPLE_REPLACE_SIMPLE_JAI` ([`SpecificIdRule`]); the id and
//! the description are built from the entry of `replace.txt`, where Java
//! uses the word as written in the text (the id is the same, the description
//! may differ by its case).

use std::collections::HashMap;

use super::super::checker::NativeRule;
use super::super::rule::{RuleContext, RuleInfo, RuleMatch};
use super::super::strtools;
use super::super::token::{Sentence, SENTENCE_START};
use super::{
    java_split, java_trim, rule_info, rule_match, to_id, SpecificIdRule, SpecificIds, TYPOS,
};

pub struct SimpleReplaceRule {
    info: RuleInfo,
    /// Wrong word -> replacements.
    wrong_words: HashMap<String, Vec<String>>,
    ids: SpecificIds,
}

/// `SimpleReplaceDataLoader.loadWords`: lines `wrong|forms=replacement|other`,
/// added to `map` (a later file overrides an earlier one).
fn load_words(
    text: &str,
    map: &mut HashMap<String, Vec<String>>,
    order: &mut Vec<String>,
) -> Result<(), String> {
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts = java_split(line, '=');
        if parts.len() != 2 {
            return Err(format!(
                "Error in line '{line}', expected format 'word=replacement'"
            ));
        }
        if java_trim(parts[1]).is_empty() {
            return Err(format!(
                "Error in line '{line}', replacement cannot be empty"
            ));
        }
        let replacements: Vec<String> = java_split(parts[1], '|')
            .into_iter()
            .map(str::to_string)
            .collect();
        for wrong_form in java_split(parts[0], '|') {
            if map
                .insert(wrong_form.to_string(), replacements.clone())
                .is_none()
            {
                order.push(wrong_form.to_string());
            }
        }
    }
    Ok(())
}

impl SimpleReplaceRule {
    /// The French rule (`FR_SIMPLE_REPLACE_SIMPLE`) over the texts of
    /// `replace.txt` and `replace_custom.txt`, and the rules its matches are
    /// reported under.
    pub fn french(files: &[&str]) -> Result<(SimpleReplaceRule, Vec<SpecificIdRule>), String> {
        let mut wrong_words = HashMap::new();
        let mut order = Vec::new();
        for text in files {
            load_words(text, &mut wrong_words, &mut order)?;
        }
        // `$match` stands for the word in the description of each id.
        let info = rule_info(
            "FR_SIMPLE_REPLACE_SIMPLE",
            "Mot incorrect : $match",
            &TYPOS,
            "misspelling",
        );
        let keys = order.into_iter().map(|word| {
            let id = to_id(&format!("{}_{}", info.id, word));
            let description = info.description.replace("$match", &word);
            (word, id, description)
        });
        let (ids, rules) = SpecificIds::new(keys, &info);
        Ok((
            SimpleReplaceRule {
                info,
                wrong_words,
                ids,
            },
            rules,
        ))
    }
}

impl NativeRule for SimpleReplaceRule {
    fn info(&self) -> &RuleInfo {
        &self.info
    }

    fn match_sentence(
        &self,
        rule: usize,
        sentence: &Sentence,
        _ctx: &RuleContext,
    ) -> Vec<RuleMatch> {
        let mut matches = Vec::new();
        for i in 0..sentence.len() {
            let token = sentence.tok(i);
            // The rule is mostly about spelling: it skips what the speller
            // skips, and the words the tagger knows.
            if token.readings[0].pos() == Some(SENTENCE_START)
                || token.immunized
                || token.ignored_by_speller
                || token.is_tagged()
            {
                continue;
            }
            let original: &str = &token.token;
            let (key, possible_replacements) = match self.wrong_words.get_key_value(original) {
                Some(found) => found,
                None => match self.wrong_words.get_key_value(&original.to_lowercase()) {
                    Some(found) => found,
                    None => continue,
                },
            };
            let mut replacements: Vec<String> = if strtools::is_all_uppercase(original) {
                possible_replacements
                    .iter()
                    .map(|s| s.to_uppercase())
                    .collect()
            } else {
                possible_replacements.clone()
            };
            if let Some(same) = replacements.iter().position(|r| r == original) {
                replacements.remove(same);
            }
            if replacements.is_empty() {
                continue;
            }
            // The message quotes the first replacement before its case is
            // adapted.
            let message = format!("Vouliez-vous dire « {} » ?", replacements[0]);
            if strtools::starts_with_uppercase(original) {
                for replacement in &mut replacements {
                    *replacement = strtools::uppercase_first_char(replacement);
                }
            }
            matches.extend(rule_match(
                self.ids.rule_index(rule, key),
                token.start_pos,
                token.end_pos(),
                &message,
                "Mot incorrect",
                replacements,
            ));
        }
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{context, sentence};
    use super::*;

    const DATA: &str =
        "# comment\njai=j'ai\nca=ça|CA\nmalgrés=malgré\nlair|dair=l'air|air\nbon fin=bonne fin\n";

    fn check(tokens: &[&str]) -> Vec<(String, usize, usize, Vec<String>, String)> {
        let (rule, ids) = SimpleReplaceRule::french(&[DATA, "jai=j’ai\n"]).unwrap();
        rule.match_sentence(5, &sentence(tokens), &context())
            .into_iter()
            .map(|m| {
                (
                    ids[m.rule - 6].info().id.clone(),
                    m.from_pos,
                    m.to_pos,
                    m.suggestions,
                    m.message,
                )
            })
            .collect()
    }

    #[test]
    fn unknown_words_with_a_replacement() {
        assert_eq!(
            check(&["Il", " ", "vient", " ", "malgrés", " ", "tout", "."]),
            vec![(
                "FR_SIMPLE_REPLACE_SIMPLE_MALGRÉS".to_string(),
                9,
                17,
                vec!["malgré".to_string()],
                "Vouliez-vous dire « malgré » ?".to_string()
            )]
        );
        // The later file wins; the suggestion takes the capital of the word.
        assert_eq!(
            check(&["Jai", " ", "faim", "."]),
            vec![(
                "FR_SIMPLE_REPLACE_SIMPLE_JAI".to_string(),
                0,
                3,
                vec!["J’ai".to_string()],
                "Vouliez-vous dire « j’ai » ?".to_string()
            )]
        );
        // In capitals: the replacements too, the word itself excepted.
        assert_eq!(
            check(&["CA", " ", "alors", "."]),
            vec![(
                "FR_SIMPLE_REPLACE_SIMPLE_CA".to_string(),
                0,
                2,
                vec!["ÇA".to_string()],
                "Vouliez-vous dire « ÇA » ?".to_string()
            )]
        );
        let found = check(&["dair", " ", "pur", "."]);
        assert_eq!(found[0].0, "FR_SIMPLE_REPLACE_SIMPLE_DAIR");
        assert_eq!(found[0].3, vec!["l'air".to_string(), "air".to_string()]);
    }

    #[test]
    fn skipped_tokens() {
        // Tagged, immunized.
        assert!(check(&["ca/ça/R dem m s", " ", "va", "."]).is_empty());
        assert!(check(&["*jai", " ", "faim", "."]).is_empty());
        let mut s = sentence(&["jai", " ", "faim", "."]);
        s.tokens[1].ignored_by_speller = true;
        let (rule, _) = SimpleReplaceRule::french(&[DATA]).unwrap();
        assert!(rule.match_sentence(0, &s, &context()).is_empty());
    }

    #[test]
    fn data_errors() {
        assert!(SimpleReplaceRule::french(&["mot\n"]).is_err());
        assert!(SimpleReplaceRule::french(&["mot= \n"]).is_err());
        assert!(SimpleReplaceRule::french(&["a=b=c\n"]).is_err());
    }
}
