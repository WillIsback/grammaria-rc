//! Port of `org.languagetool.rules.fr.SuggestionsFilter`: drops the
//! suggestions that match the regular expression `RemoveSuggestionsRegexp`.

use std::collections::HashMap;

use crate::engine::rule::{RuleContext, RuleFilter, RuleMatch};
use crate::engine::token::{Readings, Sentence};
use crate::re::{self, Flags};

pub(crate) struct SuggestionsFilter;

impl RuleFilter for SuggestionsFilter {
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
        let pattern = re::compile(args.get("RemoveSuggestionsRegexp")?, Flags::IGNORE_CASE).ok()?;
        m.suggestions.retain(|s| !pattern.is_full_match(s));
        Some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::filters::testing::*;

    #[test]
    fn removes_matching_suggestions_ignoring_case() {
        let s = sentence(&[("je", &[]), ("manger", &[])]);
        let mut m = match_over(&s, 2, 2, "");
        m.suggestions = vec!["mange".into(), "mangé".into(), "MANGÈ".into(), "mangeais".into()];
        let out = SuggestionsFilter
            .accept(m, &args(&[("RemoveSuggestionsRegexp", "^.*[éè]$")]), 1, &s.non_ws_tokens()[1..], &[1, 1], &s, &bare_context())
            .unwrap();
        assert_eq!(out.suggestions, ["mange", "mangeais"]);
    }

    #[test]
    fn a_match_without_suggestions_is_kept() {
        let s = sentence(&[("je", &[])]);
        let m = match_over(&s, 1, 1, "");
        let out = SuggestionsFilter.accept(m, &args(&[("RemoveSuggestionsRegexp", "^.*[è]$")]), 1, &s.non_ws_tokens()[1..], &[1], &s, &bare_context());
        assert!(out.is_some_and(|m| m.suggestions.is_empty()));
    }
}
