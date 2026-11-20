//! Port of `org.languagetool.rules.fr.MakeContractionsFilter`
//! (`AbstractMakeContractionsFilter`): contracts "de le", "à le", "de les"
//! and "à les" in the suggestions.

use std::collections::HashMap;

use super::static_regex;
use crate::engine::rule::{RuleContext, RuleFilter, RuleMatch};
use crate::engine::token::{Readings, Sentence};
use crate::re::Flags;

static_regex!(de_le, "\\bde le\\b", Flags::IGNORE_CASE_UNICODE_CLASS);
static_regex!(a_le, "\\bà le\\b", Flags::IGNORE_CASE_UNICODE_CLASS);
static_regex!(de_les, "\\bde les\\b", Flags::IGNORE_CASE_UNICODE_CLASS);
static_regex!(a_les, "\\bà les\\b", Flags::IGNORE_CASE_UNICODE_CLASS);

/// `fixContractions`.
fn fix_contractions(suggestion: &str) -> String {
    let s = de_le().replace_all(suggestion, "du");
    let s = a_le().replace_all(&s, "au");
    let s = de_les().replace_all(&s, "des");
    a_les().replace_all(&s, "aux")
}

pub(crate) struct MakeContractionsFilter;

impl RuleFilter for MakeContractionsFilter {
    fn accept(
        &self,
        mut m: RuleMatch,
        _args: &HashMap<String, String>,
        _pattern_token_pos: usize,
        _pattern_tokens: &[&Readings],
        _token_positions: &[usize],
        _sentence: &Sentence,
        _ctx: &RuleContext,
    ) -> Option<RuleMatch> {
        for s in m.suggestions.iter_mut() {
            *s = fix_contractions(s);
        }
        Some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contracts_prepositions_and_articles() {
        assert_eq!(fix_contractions("pareil à le chat"), "pareil au chat");
        assert_eq!(fix_contractions("pareil à les chats"), "pareil aux chats");
        assert_eq!(fix_contractions("près de le mur"), "près du mur");
        assert_eq!(fix_contractions("près de les murs"), "près des murs");
        assert_eq!(fix_contractions("De Le Mans"), "du Mans");
        // "de les" is only reached once "de le" has failed on its word boundary.
        assert_eq!(fix_contractions("de lesquels"), "de lesquels");
        assert_eq!(fix_contractions("à la mer"), "à la mer");
    }
}
