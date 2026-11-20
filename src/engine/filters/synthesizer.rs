//! Port of `org.languagetool.rules.fr.AdvancedSynthesizerFilter`
//! (`org.languagetool.rules.AbstractAdvancedSynthesizerFilter`): inflects the
//! lemma of one matched token with the tag of another one, and puts the
//! result in the suggestions (`{suggestion}`, `{Suggestion}`, `{SUGGESTION}`).

use std::collections::HashMap;

use super::{rebuilt_match, token_position, UNKNOWN};
use crate::engine::rule::{RuleContext, RuleFilter, RuleMatch};
use crate::engine::strtools;
use crate::engine::token::{Reading, Readings, Sentence};
use crate::re::{self, Flags, Regex};

pub(crate) struct AdvancedSynthesizerFilter;

/// `getAnalyzedToken`: the first reading whose tag matches, else the first
/// reading.
fn analyzed_token<'a>(token: &'a Readings, pattern: &Regex) -> &'a Reading {
    token.readings.iter().find(|r| pattern.is_full_match(r.pos().unwrap_or(UNKNOWN))).unwrap_or(&token.readings[0])
}

/// `getCompositePostag`: `postag_replace` with `\aN` and `\bN` replaced by
/// the groups of `lemma_select` in the lemma token's tag and of
/// `postag_select` in the tag token's tag.
fn composite_postag(lemma_select: &Regex, postag_select: &Regex, original_postag: &str, desired_postag: &str, postag_replace: &str) -> String {
    let a_chars: Vec<char> = original_postag.chars().collect();
    let b_chars: Vec<char> = desired_postag.chars().collect();
    let mut result = postag_replace.to_string();
    if let (Some(a), Some(b)) = (lemma_select.full_match_chars(&a_chars), postag_select.full_match_chars(&b_chars)) {
        for i in 1..=lemma_select.group_count() {
            if let Some(group) = a.get(&a_chars, i) {
                result = result.replace(&format!("\\a{i}"), &group);
            }
        }
        for i in 1..=postag_select.group_count() {
            if let Some(group) = b.get(&b_chars, i) {
                result = result.replace(&format!("\\b{i}"), &group);
            }
        }
    }
    result
}

impl RuleFilter for AdvancedSynthesizerFilter {
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
        let postag_select = re::compile(args.get("postagSelect")?, Flags::default()).ok()?;
        let lemma_select = re::compile(args.get("lemmaSelect")?, Flags::default()).ok()?;
        let new_lemma = args.get("newLemma").map(String::as_str).unwrap_or("");
        let postag_from = token_position(args.get("postagFrom")?, pattern_tokens, &m, false)?;
        let lemma_from = token_position(args.get("lemmaFrom")?, pattern_tokens, &m, false)?;
        let postag_replace = args.get("postagReplace");

        let lemma_token = pattern_tokens[lemma_from];
        let lemma_reading = analyzed_token(lemma_token, &lemma_select);
        let mut desired_lemma: Option<String> = lemma_reading.lemma.as_deref().map(str::to_string);
        let original_postag = lemma_reading.pos();
        let mut desired_postag: String = analyzed_token(pattern_tokens[postag_from], &postag_select).pos()?.to_string();
        if !new_lemma.is_empty() {
            // `getNewLemma` (a lemma derived from the old one, `_...`) is
            // not defined for French.
            desired_lemma = if new_lemma.starts_with('_') { None } else { Some(new_lemma.to_string()) };
        }
        let desired_lemma = desired_lemma?;
        if let Some(postag_replace) = postag_replace {
            desired_postag = composite_postag(&lemma_select, &postag_select, original_postag?, &desired_postag, postag_replace);
        }
        let is_word_capitalized = strtools::is_capitalized_word(&lemma_token.token);
        let is_word_all_upper = strtools::is_all_uppercase(&lemma_token.token);
        let token = Reading::from_strs("", Some(&desired_postag), Some(&desired_lemma));
        let replacements = match &ctx.lang.synthesizer {
            Some(synth) => synth.synthesize_regexp(&token, &desired_postag, true),
            None => Vec::new(),
        };
        if replacements.is_empty() {
            return Some(m);
        }
        let mut new_match = rebuilt_match(&m, sentence, m.from_pos, m.to_pos, m.message.clone());
        let mut list: Vec<String> = Vec::new();
        let mut suggestion_used = false;
        for r in &m.suggestions {
            for nr in &replacements {
                if r.contains("{suggestion}") || r.contains("{Suggestion}") || r.contains("{SUGGESTION}") {
                    suggestion_used = true;
                }
                let mut nr = nr.clone();
                if is_word_capitalized {
                    nr = strtools::uppercase_first_char(&nr);
                }
                if is_word_all_upper {
                    nr = nr.to_uppercase();
                }
                let complete = r
                    .replace("{suggestion}", &nr)
                    .replace("{Suggestion}", &strtools::uppercase_first_char(&nr))
                    .replace("{SUGGESTION}", &nr.to_uppercase());
                if !list.contains(&complete) {
                    list.push(complete);
                }
            }
        }
        if !suggestion_used {
            list.extend(replacements);
        }
        new_match.suggestions = list;
        Some(new_match)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::filters::testing::*;

    #[test]
    fn composite_tags() {
        let a = Regex::new("V.inf", Flags::default()).unwrap();
        let b = Regex::new("V.ind.pres.(.*)", Flags::default()).unwrap();
        assert_eq!(composite_postag(&a, &b, "V inf", "V ind pres 3 s", "V.sub.pres.\\b1"), "V.sub.pres.3 s");
        // No substitution when one of the tags does not match its selector.
        assert_eq!(composite_postag(&a, &b, "N m s", "V ind pres 3 s", "V.sub.pres.\\b1"), "V.sub.pres.\\b1");
    }

    #[test]
    fn inflects_the_lemma_with_the_other_tokens_tag() {
        let Some(ctx) = context() else { return };
        // "les Maisons Blanc": the lemma of the adjective with the tag of
        // the noun, in the case of the adjective.
        let s = sentence(&[
            ("les", &[("D e p", "le")]),
            ("Maisons", &[("N f p", "maison")]),
            ("Blanc", &[("J m s", "blanc")]),
        ]);
        let tokens = s.non_ws_tokens();
        let mut m = match_over(&s, 3, 3, "");
        m.suggestions = vec!["{suggestion}".into(), "très {suggestion}".into()];
        let a = args(&[("lemmaFrom", "marker"), ("lemmaSelect", "J.*"), ("postagFrom", "marker-1"), ("postagSelect", "N.*")]);
        let out = AdvancedSynthesizerFilter.accept(m.clone(), &a, 1, &tokens[1..], &[1, 1, 1], &s, &ctx).unwrap();
        assert_eq!(out.suggestions, ["Blanches", "très Blanches"]);
        // A tag that inflects nothing leaves the match as it is.
        let a = args(&[("lemmaFrom", "marker"), ("lemmaSelect", "J.*"), ("postagFrom", "1"), ("postagSelect", "D.*")]);
        let out = AdvancedSynthesizerFilter.accept(m.clone(), &a, 1, &tokens[1..], &[1, 1, 1], &s, &ctx).unwrap();
        assert_eq!(out.suggestions, m.suggestions);

        // Verb agreement: "pouvoir" with the tag of "sont".
        let s = sentence(&[("sont", &[("V etre ind pres 3 p", "être")]), ("et", &[("C coor", "et")]), ("Peut", &[("V ind pres 3 s", "pouvoir")])]);
        let tokens = s.non_ws_tokens();
        let mut m = match_over(&s, 3, 3, "");
        m.suggestions = vec!["{suggestion}".into()];
        let a = args(&[("lemmaFrom", "3"), ("lemmaSelect", "V.*"), ("postagFrom", "1"), ("postagSelect", "V.*"), ("postagReplace", "V ind pres 3 p")]);
        let out = AdvancedSynthesizerFilter.accept(m.clone(), &a, 1, &tokens[1..], &[1, 1, 1], &s, &ctx).unwrap();
        assert_eq!(out.suggestions, ["Peuvent"]);

        // Without a placeholder, the forms are appended as they are.
        m.suggestions = vec!["peut".into()];
        let out = AdvancedSynthesizerFilter.accept(m, &a, 1, &tokens[1..], &[1, 1, 1], &s, &ctx).unwrap();
        assert_eq!(out.suggestions, ["peut", "peuvent"]);
    }

    #[test]
    fn rejects_when_the_lemma_is_unknown() {
        let Some(ctx) = context() else { return };
        let s = sentence(&[("chat", &[("N m s", "chat")]), ("xyz", &[])]);
        let tokens = s.non_ws_tokens();
        let m = match_over(&s, 2, 2, "");
        let a = args(&[("lemmaFrom", "2"), ("lemmaSelect", "J.*"), ("postagFrom", "1"), ("postagSelect", "N.*")]);
        assert!(AdvancedSynthesizerFilter.accept(m, &a, 1, &tokens[1..], &[1, 1], &s, &ctx).is_none());
    }
}
