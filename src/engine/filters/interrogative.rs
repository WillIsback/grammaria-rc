//! Port of `org.languagetool.rules.fr.InterrogativeVerbFilter`: for a
//! misspelt verb before an inverted pronoun ("mange-tu"), proposes the
//! speller's suggestions that are verb forms of the pronoun's person
//! ("manges-tu").
//!
//! The suggestions come from the language's speller
//! ([`Lang::speller`](crate::engine::pattern::Lang)) and tagger; without them
//! the match is kept with the suggestions of its message.

use std::collections::HashMap;

use super::{parse_int, rebuilt_match, tag_pattern, TagPattern};
use crate::engine::rule::{RuleContext, RuleFilter, RuleMatch};
use crate::engine::strtools;
use crate::engine::token::{Readings, Sentence};

tag_pattern!(obj_2p, "R pers obj 2 p");
tag_pattern!(obj_1p, "R pers obj 1 p");
tag_pattern!(obj, "R pers obj.*");
tag_pattern!(first_s, ".* 1 s");
tag_pattern!(second_s, ".* 2 s");
tag_pattern!(third_s, ".* 3( [mfe])? s");
tag_pattern!(first_p, ".* 1 p");
tag_pattern!(second_p, ".* 2 p");
tag_pattern!(third_p, ".* 3( [mf])? p");
tag_pattern!(any_verb, "V .*");

tag_pattern!(verb_for_obj_2p, "V.* (imp) [23] [sp]|V .*(ind|cond).* 2 p");
tag_pattern!(verb_for_obj_1p, "V.* (imp) .*|V .*(ind|cond).* 1 p");
tag_pattern!(verb_imperative, "V.* (imp) .*");
tag_pattern!(verb_1s, "V .*(ind|cond).* 1 s");
tag_pattern!(verb_2s, "V .*(ind|cond).* 2 s");
tag_pattern!(verb_3s, "V .*(ind|cond).* 3 s");
tag_pattern!(verb_1p, "V .*(ind|cond).* 1 p");
tag_pattern!(verb_2p, "V .*(ind|cond).* 2 p");
tag_pattern!(verb_3p, "V .*(ind|cond).* 3 p");

/// `makeWrong`: a misspelling of a known word, so that the speller proposes
/// its neighbours.
fn make_wrong(s: &str) -> String {
    const SWAPS: [(char, &str); 14] = [
        ('a', "ä"),
        ('e', "ë"),
        ('i', "í"),
        ('o', "ö"),
        ('u', "ü"),
        ('é', "ë"),
        ('à', "ä"),
        ('è', "ë"),
        ('ù', "ü"),
        ('â', "ä"),
        ('ê', "ë"),
        ('î', "ï"),
        ('ô', "ö"),
        ('û', "ü"),
    ];
    for (from, to) in SWAPS {
        if s.contains(from) {
            return s.replace(from, to);
        }
    }
    format!("{s}-")
}

pub(crate) struct InterrogativeVerbFilter;

impl RuleFilter for InterrogativeVerbFilter {
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
        let pronoun = pattern_tokens[position("PronounFrom")?];
        let verb = pattern_tokens[position("VerbFrom")?];
        let mut replacements: Vec<String> = Vec::new();
        let mut extra_suggestions: Vec<String> = Vec::new();

        // The verb forms that go with the pronoun.
        let desired: Option<&TagPattern> = if obj_2p().matches_tagged(pronoun) {
            Some(verb_for_obj_2p())
        } else if obj_1p().matches_tagged(pronoun) {
            Some(verb_for_obj_1p())
        } else if obj().matches_tagged(pronoun) {
            Some(verb_imperative())
        } else if first_s().matches_tagged(pronoun) {
            // "trompé-je", "trompè-je" for a verb in -er.
            if let (Some(reading), Some(synth)) = (verb.readings.iter().find(|r| any_verb().matches_tag(r)), &ctx.lang.synthesizer) {
                if let Some(base) = synth.synthesize_regexp(reading, "V ppa [me] sp?", true).first().and_then(|p| p.strip_suffix('é')) {
                    extra_suggestions.push(format!("{base}é"));
                    extra_suggestions.push(format!("{base}è"));
                }
            }
            Some(verb_1s())
        } else if second_s().matches_tagged(pronoun) {
            Some(verb_2s())
        } else if third_s().matches_tagged(pronoun) {
            Some(verb_3s())
        } else if first_p().matches_tagged(pronoun) {
            Some(verb_1p())
        } else if second_p().matches_tagged(pronoun) {
            Some(verb_2p())
        } else if third_p().matches_tagged(pronoun) {
            Some(verb_3p())
        } else {
            None
        };
        let separator = if pronoun.token.starts_with('-') { "" } else { "-" };
        if !extra_suggestions.is_empty() {
            for extra in &extra_suggestions {
                let complete = format!("{extra}{separator}{}", pronoun.token);
                if !replacements.contains(&complete) && !complete.ends_with("e-je") {
                    replacements.push(complete);
                }
            }
        } else if let (Some(desired), Some(speller), Some(tagger)) = (desired, &ctx.lang.speller, &ctx.lang.tagger) {
            // `morfologikRule.match` on the verb alone: a known verb is
            // misspelt on purpose, an unknown one is checked as written
            // (unless the speller is told to leave it alone).
            let suggestions = if verb.is_tagged() {
                speller.spelling_suggestions(&make_wrong(&verb.token))
            } else if verb.immunized || verb.ignored_by_speller {
                Vec::new()
            } else {
                speller.spelling_suggestions(&verb.token)
            };
            for suggestion in suggestions {
                let analyzed = tagger.tag_word(&suggestion);
                if !desired.matches_tagged(&analyzed) {
                    continue;
                }
                let mut complete = format!("{}{separator}{}", analyzed.token, pronoun.token);
                if strtools::equals_ignore_case(&complete, "peux-je") {
                    complete = strtools::preserve_case("puis-je", &complete);
                }
                let forms = match complete.strip_suffix("e-je") {
                    Some(base) => vec![format!("{base}é-je"), format!("{base}è-je")],
                    None => vec![complete],
                };
                for form in forms {
                    if !replacements.contains(&form) {
                        replacements.push(form);
                    }
                }
            }
        }
        let mut rule_match = rebuilt_match(&m, sentence, m.from_pos, m.to_pos, m.message.clone());
        if !replacements.is_empty() {
            rule_match.suggestions = replacements;
        }
        Some(rule_match)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::filters::testing::*;
    use crate::engine::pattern::SpellChecker;
    use std::sync::Arc;

    struct FixedSpeller(&'static [&'static str]);

    impl SpellChecker for FixedSpeller {
        fn is_misspelled(&self, _word: &str) -> bool {
            true
        }
        fn spelling_suggestions(&self, _word: &str) -> Vec<String> {
            self.0.iter().map(|s| s.to_string()).collect()
        }
    }

    #[test]
    fn make_wrong_changes_the_first_kind_of_vowel_found() {
        assert_eq!(make_wrong("mange"), "mänge");
        assert_eq!(make_wrong("dire"), "dirë");
        assert_eq!(make_wrong("dit"), "dít");
        assert_eq!(make_wrong("crû"), "crü");
        assert_eq!(make_wrong("zzz"), "zzz-");
    }

    #[test]
    fn keeps_the_suggestions_that_fit_the_pronoun() {
        let Some(mut ctx) = context() else { return };
        ctx.lang.speller = Some(Arc::new(FixedSpeller(&["manges", "mange", "manger", "mangez"])));
        let s = sentence(&[("Mange", &[("V ind pres 3 s", "manger")]), ("-tu", &[("R pers suj 2 s", "tu")])]);
        let tokens = s.non_ws_tokens();
        let a = args(&[("PronounFrom", "2"), ("VerbFrom", "1")]);
        let out = InterrogativeVerbFilter.accept(match_over(&s, 1, 2, ""), &a, 1, &tokens[1..], &[1, 1], &s, &ctx).unwrap();
        assert_eq!(out.suggestions, ["manges-tu"]);

        // Object pronoun of the second person plural: only the "vous" form
        // fits ("mange" is tagged "V imp pres 2 s", which the filter's
        // imperative pattern does not cover).
        let s = sentence(&[("Mange", &[("V ind pres 3 s", "manger")]), ("-vous", &[("R pers obj 2 p", "vous")])]);
        let tokens = s.non_ws_tokens();
        let out = InterrogativeVerbFilter.accept(match_over(&s, 1, 2, ""), &a, 1, &tokens[1..], &[1, 1], &s, &ctx).unwrap();
        assert_eq!(out.suggestions, ["mangez-vous"]);
    }

    #[test]
    fn first_person_of_a_verb_in_er() {
        let Some(ctx) = context() else { return };
        let s = sentence(&[("trompe", &[("V ind pres 1 s", "tromper")]), ("-je", &[("R pers suj 1 s", "je")])]);
        let tokens = s.non_ws_tokens();
        let a = args(&[("PronounFrom", "2"), ("VerbFrom", "1")]);
        let out = InterrogativeVerbFilter.accept(match_over(&s, 1, 2, ""), &a, 1, &tokens[1..], &[1, 1], &s, &ctx).unwrap();
        assert_eq!(out.suggestions, ["trompé-je", "trompè-je"]);
    }

    #[test]
    fn without_a_speller_the_match_keeps_the_suggestions_of_its_message() {
        let Some(ctx) = context() else { return };
        let s = sentence(&[("Mange", &[("V ind pres 3 s", "manger")]), ("-tu", &[("R pers suj 2 s", "tu")])]);
        let tokens = s.non_ws_tokens();
        let a = args(&[("PronounFrom", "2"), ("VerbFrom", "1")]);
        let m = match_over(&s, 1, 2, "Écrivez <suggestion>manges-tu</suggestion>.");
        let out = InterrogativeVerbFilter.accept(m, &a, 1, &tokens[1..], &[1, 1], &s, &ctx).unwrap();
        assert_eq!(out.suggestions, ["manges-tu"]);
    }
}
