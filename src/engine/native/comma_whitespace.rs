//! Port of `org.languagetool.rules.CommaWhitespaceRule`: a space before a
//! comma, a closing parenthesis or a full stop, after an opening
//! parenthesis, or missing after a comma.

use std::sync::Arc;

use super::super::checker::NativeRule;
use super::super::rule::{RuleContext, RuleInfo, RuleMatch};
use super::super::token::{Readings, Sentence};
use super::{is_java_digit, messages, rule_info, rule_match, TYPOGRAPHY};
use crate::re::{self, Flags, Regex};

const COMMA: &str = ",";

pub struct CommaWhitespaceRule {
    info: RuleInfo,
    /// Also report spaces on both sides of a quote (off for French).
    quotes_whitespace_check: bool,
    file_extension: Arc<Regex>,
}

impl CommaWhitespaceRule {
    pub fn new(quotes_whitespace_check: bool) -> CommaWhitespaceRule {
        CommaWhitespaceRule {
            info: rule_info(
                "COMMA_PARENTHESIS_WHITESPACE",
                messages::DESC_COMMA_WHITESPACE,
                &TYPOGRAPHY,
                "whitespace",
            ),
            quotes_whitespace_check,
            file_extension: re::compile(
                "([a-z]{3,4}|[A-Z]{3,4}|ai|mp[34]|MP[34])(-.+)?",
                Flags::default(),
            )
            .expect("valid pattern"),
        }
    }

    fn is_file_extension(&self, tokens: &[Readings], i: usize) -> bool {
        i < tokens.len() && self.file_extension.is_full_match(&tokens[i].token)
    }
}

fn is_whitespace_token(token: &Readings) -> bool {
    (token.is_whitespace || &*token.token == "\u{a0}" || token.is_field_code())
        && &*token.token != "\u{200b}"
}

fn is_quote(s: &str) -> bool {
    matches!(s, "'" | "\"" | "’" | "”" | "“" | "«" | "»")
}

fn is_hyphen_or_comma(s: &str) -> bool {
    matches!(s, "-" | ",")
}

fn is_digit_or_dot(s: &str) -> bool {
    s.chars()
        .next()
        .is_some_and(|c| c == '.' || is_java_digit(c))
}

fn contains_digit(s: &str) -> bool {
    s.chars().any(is_java_digit)
}

fn is_proper_noun(token: &Readings) -> bool {
    token.has_pos_tag("NNP") || token.has_pos_tag("NNPS") || token.has_pos_tag_starting_with("NP")
}

/// `(?i)(com|org|net|int|edu|gov|mil|[a-z]{2})`.
fn is_domain(tokens: &[Readings], i: usize) -> bool {
    let Some(token) = tokens.get(i) else {
        return false;
    };
    let token: &str = &token.token;
    if !token.is_ascii() {
        return false;
    }
    (token.len() == 2 && token.bytes().all(|b| b.is_ascii_alphabetic()))
        || ["com", "org", "net", "int", "edu", "gov", "mil"]
            .iter()
            .any(|d| token.eq_ignore_ascii_case(d))
}

impl NativeRule for CommaWhitespaceRule {
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
        let tokens = &sentence.tokens;
        let text: &str = &sentence.text;
        let mut prev_token: &str = "";
        let mut prev_prev_token: &str = "";
        let mut prev_white = false;
        for i in 0..tokens.len() {
            let token: &str = &tokens[i].token;
            let is_whitespace = is_whitespace_token(&tokens[i]);
            let mut two_suggestions = false;
            let mut msg: Option<&str> = None;
            let mut suggestion = String::new();
            if is_whitespace && prev_token.starts_with('(') {
                // "- [ ]" as on GitHub
                let is_exception = i + 1 < tokens.len()
                    && prev_token == "["
                    && token == " "
                    && &*tokens[i + 1].token == "]";
                if !is_exception {
                    msg = Some(messages::NO_SPACE_AFTER);
                    suggestion = prev_token.to_string();
                }
            } else if is_whitespace
                && is_quote(prev_token)
                && self.quotes_whitespace_check
                && prev_prev_token == " "
            {
                msg = Some(messages::NO_SPACE_AROUND_QUOTES);
                suggestion = prev_token.to_string();
                two_suggestions = true;
            } else if !is_whitespace
                && prev_token == COMMA
                && !is_quote(token)
                && !is_hyphen_or_comma(token)
                && !contains_digit(prev_prev_token)
                && !contains_digit(token)
                && prev_prev_token != ","
            {
                msg = Some(messages::MISSING_SPACE_AFTER_COMMA);
                suggestion = format!("{COMMA} {token}");
            } else if prev_white {
                if token.starts_with(')') {
                    let is_exception = token == "]" && prev_token == " " && prev_prev_token == "[";
                    if !is_exception {
                        msg = Some(messages::NO_SPACE_BEFORE);
                        suggestion = token.to_string();
                    }
                } else if token == COMMA {
                    msg = Some(messages::SPACE_AFTER_COMMA);
                    suggestion = COMMA.to_string();
                    if i + 1 < tokens.len() && &*tokens[i + 1].token == COMMA {
                        msg = None;
                    }
                    if i + 1 < tokens.len() && !tokens[i + 1].is_whitespace {
                        suggestion = format!("{COMMA} ");
                    }
                } else if token == "."
                    && !is_domain(tokens, i + 1)
                    && !self.is_file_extension(tokens, i + 1)
                    && !is_proper_noun(&tokens[i])
                {
                    msg = Some(messages::NO_SPACE_BEFORE_DOT);
                    suggestion = ".".to_string();
                    // " .5", " ..", " ./path"
                    let before_number_or_dot =
                        i + 1 < tokens.len() && is_digit_or_dot(&tokens[i + 1].token);
                    let before_path = i + 2 < tokens.len()
                        && &*tokens[i + 1].token == "/"
                        && !tokens[i + 2].token.is_empty()
                        && tokens[i + 2].token.bytes().all(|b| b.is_ascii_alphabetic());
                    if before_number_or_dot || before_path {
                        msg = None;
                    }
                }
            }
            if let Some(msg) = msg {
                if !tokens[i].immunized {
                    let from = if two_suggestions {
                        tokens[i - 2].start_pos
                    } else {
                        tokens[i - 1].start_pos
                    };
                    let to = tokens[i].end_pos();
                    // The marked text may already be what would be suggested.
                    let unchanged =
                        to < text.len() && !two_suggestions && text[from..to] == suggestion;
                    if !unchanged {
                        let suggestions = if two_suggestions {
                            vec![format!("{suggestion} "), format!(" {suggestion}")]
                        } else {
                            vec![suggestion]
                        };
                        matches.extend(rule_match(rule, from, to, msg, "", suggestions));
                    }
                }
            }
            prev_prev_token = prev_token;
            prev_token = token;
            // An OpenOffice field code before a comma or a dot is no space.
            prev_white = is_whitespace && !tokens[i].is_field_code();
        }
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{context, plain, sentence};
    use super::*;

    fn check(s: &Sentence) -> Vec<(usize, usize, Vec<String>)> {
        CommaWhitespaceRule::new(false)
            .match_sentence(0, s, &context())
            .into_iter()
            .map(|m| (m.from_pos, m.to_pos, m.suggestions))
            .collect()
    }

    #[test]
    fn spaces_around_commas_and_parentheses() {
        // "Bonjour ,comment ça va ( bien ) ?"
        let found = check(&plain("Bonjour ,comment ça va ( bien ) ?"));
        assert_eq!(
            found,
            vec![
                (7, 9, vec![", ".to_string()]),
                // Overlaps the previous one: the checker keeps the first.
                (8, 16, vec![", comment".to_string()]),
                (24, 26, vec!["(".to_string()]),
                (30, 32, vec![")".to_string()]),
            ]
        );
        assert_eq!(
            check(&plain("Oui,non")),
            vec![(3, 7, vec![", non".to_string()])]
        );
        assert!(check(&plain("Oui, non (peut-être).")).is_empty());
    }

    #[test]
    fn numbers_domains_and_immunized_tokens() {
        assert!(check(&plain("Il vaut 3,5 euros")).is_empty());
        assert!(check(&plain("voir le fichier .txt")).is_empty());
        assert!(check(&plain("le site example .com")).is_empty());
        assert_eq!(
            check(&plain("La fin .")),
            vec![(6, 8, vec![".".to_string()])]
        );
        assert!(check(&sentence(&["La", " ", "fin", " ", "*."])).is_empty());
        // A space before two commas is left to the double punctuation rule.
        assert!(check(&plain("Oui ,, non")).is_empty());
    }
}
