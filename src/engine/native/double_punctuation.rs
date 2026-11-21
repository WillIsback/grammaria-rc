//! Port of `org.languagetool.rules.DoublePunctuationRule`: two full stops or
//! two commas in a row.

use super::super::checker::NativeRule;
use super::super::rule::{RuleContext, RuleInfo, RuleMatch};
use super::super::token::Sentence;
use super::{messages, prev_char_start, rule_info, rule_match, PUNCTUATION};

const COMMA: &str = ",";

pub struct DoublePunctuationRule {
    info: RuleInfo,
}

impl DoublePunctuationRule {
    pub fn new() -> DoublePunctuationRule {
        DoublePunctuationRule {
            info: rule_info(
                "DOUBLE_PUNCTUATION",
                messages::DESC_DOUBLE_PUNCT,
                &PUNCTUATION,
                "typographical",
            ),
        }
    }
}

impl Default for DoublePunctuationRule {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeRule for DoublePunctuationRule {
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
        let n = sentence.len();
        let text: &str = &sentence.text;
        let mut start_pos = 0;
        let mut dot_count = 0;
        let mut comma_count = 0;
        for i in 1..n {
            let token: &str = &sentence.tok(i).token;
            let next_token: Option<&str> = if i < n - 1 {
                Some(&sentence.tok(i + 1).token)
            } else {
                None
            };
            let prev_prev_token: Option<&str> = if i > 1 {
                Some(&sentence.tok(i - 2).token)
            } else {
                None
            };
            if token == "." {
                dot_count += 1;
                comma_count = 0;
                start_pos = sentence.tok(i).start_pos;
            } else if token == COMMA {
                comma_count += 1;
                dot_count = 0;
                start_pos = sentence.tok(i).start_pos;
            }
            if dot_count == 2
                && !matches!(next_token, Some("." | "…" | "/" | "\\"))
                && !matches!(token, "/" | "\\")
                && !matches!(prev_prev_token, Some("?" | "!" | "…" | "."))
            {
                let suggestions = vec![".".to_string(), "…".to_string()];
                matches.extend(rule_match(
                    rule,
                    prev_char_start(text, start_pos),
                    start_pos + 1,
                    messages::TWO_DOTS,
                    messages::DOUBLE_DOTS_SHORT,
                    suggestions,
                ));
                dot_count = 0;
            } else if comma_count == 2 && next_token != Some(COMMA) {
                let suggestions = vec![COMMA.to_string()];
                matches.extend(rule_match(
                    rule,
                    prev_char_start(text, start_pos),
                    start_pos + 1,
                    messages::TWO_COMMAS,
                    messages::DOUBLE_COMMAS_SHORT,
                    suggestions,
                ));
                comma_count = 0;
            }
            if token != "." && token != COMMA {
                dot_count = 0;
                comma_count = 0;
            }
        }
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{context, plain};
    use super::*;

    fn check(text: &str) -> Vec<(usize, usize, Vec<String>)> {
        DoublePunctuationRule::new()
            .match_sentence(0, &plain(text), &context())
            .into_iter()
            .map(|m| (m.from_pos, m.to_pos, m.suggestions))
            .collect()
    }

    #[test]
    fn two_dots_or_commas() {
        assert_eq!(
            check("Oui.. non"),
            vec![(3, 5, vec![".".to_string(), "…".to_string()])]
        );
        assert_eq!(check("Oui,, non"), vec![(3, 5, vec![",".to_string()])]);
        // The match starts one character before the second mark.
        assert_eq!(
            check("Oui.\u{a0}. non"),
            vec![(4, 7, vec![".".to_string(), "…".to_string()])]
        );
    }

    #[test]
    fn ellipses_and_paths_are_left_alone() {
        assert!(check("Oui... non").is_empty());
        assert!(check("Oui ?.. non").is_empty());
        assert!(check("dans ../dossier").is_empty());
        assert!(check("Oui. Non, si.").is_empty());
    }
}
