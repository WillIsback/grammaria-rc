//! Port of `org.languagetool.rules.IsEnglishWordFilter` (disambiguation):
//! accepts when the words at `formPositions` are English, that is, known to
//! the English tagger (with the tags `postags` when given).
//!
//! LanguageTool takes the tagger from its English module and rejects
//! everything when the module is absent. Here the tagger is given with
//! [`FrenchFilters::set_english_tagger`](super::FrenchFilters::set_english_tagger);
//! there is none by default.

use std::collections::HashMap;
use std::sync::Arc;

use super::{parse_int, FilterEnv};
use crate::engine::rule::{skip_corrected_reference, RuleContext, RuleFilter, RuleMatch};
use crate::engine::token::{Readings, Sentence};
use crate::re::{self, Flags};

pub(crate) struct IsEnglishWordFilter {
    env: Arc<FilterEnv>,
}

impl IsEnglishWordFilter {
    pub(crate) fn new(env: Arc<FilterEnv>) -> IsEnglishWordFilter {
        IsEnglishWordFilter { env }
    }
}

impl RuleFilter for IsEnglishWordFilter {
    fn accept(
        &self,
        m: RuleMatch,
        args: &HashMap<String, String>,
        _pattern_token_pos: usize,
        pattern_tokens: &[&Readings],
        token_positions: &[usize],
        _sentence: &Sentence,
        _ctx: &RuleContext,
    ) -> Option<RuleMatch> {
        let tagger = self.env.english_tagger()?;
        let mut forms: Vec<&str> = Vec::new();
        for position in args.get("formPositions")?.split(',') {
            let index = skip_corrected_reference(token_positions, parse_int(position)?);
            forms.push(&pattern_tokens.get(usize::try_from(index).ok()?)?.token);
        }
        let is_english = match args.get("postags") {
            Some(postags) => {
                let postags: Vec<&str> = postags.split(',').collect();
                if postags.len() != forms.len() {
                    return None;
                }
                let mut all = true;
                for (form, postag) in forms.iter().zip(postags) {
                    let pattern = re::compile(postag, Flags::default()).ok()?;
                    all = all && tagger.tag_word(form).matches_pos_tag_regex(&pattern);
                }
                all
            }
            None => forms.iter().all(|form| tagger.tag_word(form).is_tagged()),
        };
        is_english.then_some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::filters::testing::*;
    use crate::engine::filters::FrenchFilters;
    use crate::engine::pattern::WordTagger;
    use crate::engine::token::Reading;

    struct English;

    impl WordTagger for English {
        fn tag_word(&self, word: &str) -> Readings {
            match word {
                "make" => Readings::new(vec![Reading::from_strs(word, Some("VB"), Some("make"))], 0),
                "the" => Readings::new(vec![Reading::from_strs(word, Some("DT"), Some("the"))], 0),
                _ => Readings::untagged(word, 0),
            }
        }
    }

    #[test]
    fn asks_the_english_tagger() {
        let s = sentence(&[("to", &[]), ("make", &[]), ("faire", &[])]);
        let tokens = s.non_ws_tokens();
        let accept = |filters: &FrenchFilters, a: &[(&str, &str)]| {
            IsEnglishWordFilter::new(filters.env.clone())
                .accept(match_over(&s, 1, 2, ""), &args(a), 1, &tokens[1..], &[1, 1, 1], &s, &bare_context())
                .is_some()
        };
        let without = FrenchFilters::new();
        assert!(!accept(&without, &[("formPositions", "2")]), "no English tagger: never English");
        let with = FrenchFilters::new();
        with.set_english_tagger(Arc::new(English));
        assert!(accept(&with, &[("formPositions", "2")]));
        assert!(accept(&with, &[("formPositions", "2"), ("postags", "VB")]));
        assert!(!accept(&with, &[("formPositions", "2"), ("postags", "NN")]));
        assert!(!accept(&with, &[("formPositions", "2,3")]), "\"faire\" is not English");
    }
}
