//! Port of `org.languagetool.rules.fr.FrenchNumberInWordFilter`
//! (`org.languagetool.rules.AbstractNumberInWordFilter`): for a word with a
//! digit typed in it ("b0njour", "bon1jour"), proposes the word with `0`
//! read as `o` and the word without its digits, when the speller knows them.
//!
//! Needs the language's speller
//! ([`Lang::speller`](crate::engine::pattern::Lang)); without one the match
//! is rejected, as when no suggestion is found.

use std::collections::HashMap;

use crate::engine::rule::{RuleContext, RuleFilter, RuleMatch};
use crate::engine::token::{Readings, Sentence};

pub(crate) struct FrenchNumberInWordFilter;

impl RuleFilter for FrenchNumberInWordFilter {
    fn accept(
        &self,
        mut m: RuleMatch,
        args: &HashMap<String, String>,
        _pattern_token_pos: usize,
        _pattern_tokens: &[&Readings],
        _token_positions: &[usize],
        _sentence: &Sentence,
        ctx: &RuleContext,
    ) -> Option<RuleMatch> {
        let speller = ctx.lang.speller.as_ref()?;
        let word = args.get("word")?;
        let word_replacing_zero_o = word.replace('0', "o");
        let word_without_number: String = word.chars().filter(|c| !c.is_ascii_digit()).collect();
        let mut replacements: Vec<String> = Vec::new();
        if !speller.is_misspelled(&word_replacing_zero_o) && *word != word_replacing_zero_o {
            replacements.push(word_replacing_zero_o);
        }
        if !speller.is_misspelled(&word_without_number) {
            replacements.push(word_without_number.clone());
        }
        if replacements.is_empty() {
            replacements = speller.spelling_suggestions(&word_without_number);
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
    use crate::engine::filters::testing::*;
    use crate::engine::pattern::SpellChecker;
    use std::sync::Arc;

    struct Words;

    impl SpellChecker for Words {
        fn is_misspelled(&self, word: &str) -> bool {
            !matches!(word, "bonjour" | "bon")
        }
        fn spelling_suggestions(&self, word: &str) -> Vec<String> {
            if word == "bnjour" {
                vec!["bonjour".to_string()]
            } else {
                Vec::new()
            }
        }
    }

    fn suggest(word: &str, with_speller: bool) -> Option<Vec<String>> {
        let mut ctx = bare_context();
        if with_speller {
            ctx.lang.speller = Some(Arc::new(Words));
        }
        let s = sentence(&[(word, &[])]);
        let m = match_over(&s, 1, 1, "");
        FrenchNumberInWordFilter
            .accept(
                m,
                &args(&[("word", word)]),
                1,
                &s.non_ws_tokens()[1..],
                &[1],
                &s,
                &ctx,
            )
            .map(|m| m.suggestions)
    }

    #[test]
    fn zero_read_as_o_then_digits_removed_then_the_speller() {
        assert_eq!(suggest("b0njour", true), Some(vec!["bonjour".to_string()]));
        assert_eq!(suggest("bon1jour", true), Some(vec!["bonjour".to_string()]));
        assert_eq!(suggest("b0n", true), Some(vec!["bon".to_string()]));
        assert_eq!(
            suggest("b9njour", true),
            Some(vec!["bonjour".to_string()]),
            "the speller's suggestions for \"bnjour\""
        );
        assert_eq!(suggest("x1yz", true), None);
        assert_eq!(suggest("b0njour", false), None);
    }
}
