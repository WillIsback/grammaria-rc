//! Port of `org.languagetool.rules.MultipleWhitespaceRule`: several
//! whitespace characters in a row.

use super::super::checker::NativeRule;
use super::super::rule::{RuleContext, RuleInfo, RuleMatch};
use super::super::token::{Readings, Sentence};
use super::{messages, rule_info, rule_match, TYPOGRAPHY};

pub struct MultipleWhitespaceRule {
    info: RuleInfo,
}

impl MultipleWhitespaceRule {
    pub fn new() -> MultipleWhitespaceRule {
        MultipleWhitespaceRule { info: rule_info("WHITESPACE_RULE", messages::DESC_WHITESPACEREPETITION, &TYPOGRAPHY, "whitespace") }
    }
}

impl Default for MultipleWhitespaceRule {
    fn default() -> Self {
        Self::new()
    }
}

/// Whitespace other than a line break or a zero-width character.
fn is_first_white(token: &Readings) -> bool {
    (token.is_whitespace || &*token.token == "\u{a0}") && !token.is_linebreak && !token.token.contains(['\u{200b}', '\u{feff}', '\u{2060}'])
}

/// The same, tabs excepted: what may be removed after a first whitespace.
fn is_removable_white(token: &Readings) -> bool {
    is_first_white(token) && &*token.token != "\t"
}

impl NativeRule for MultipleWhitespaceRule {
    fn info(&self) -> &RuleInfo {
        &self.info
    }

    fn is_text_level(&self) -> bool {
        true
    }

    fn match_text(&self, rule: usize, sentences: &[Sentence], _ctx: &RuleContext) -> Vec<RuleMatch> {
        let mut matches = Vec::new();
        let mut pos = 0;
        for sentence in sentences {
            let tokens = &sentence.tokens;
            let mut i = 1;
            while i < tokens.len() {
                if is_first_white(&tokens[i]) {
                    let first = i;
                    i += 1;
                    while i < tokens.len() && is_removable_white(&tokens[i]) {
                        i += 1;
                    }
                    i -= 1;
                    if i > first {
                        matches.extend(rule_match(
                            rule,
                            pos + tokens[first].start_pos,
                            pos + tokens[i].end_pos(),
                            messages::WHITESPACE_REPETITION,
                            "",
                            vec![tokens[first].token.to_string()],
                        ));
                    }
                } else if tokens[i].is_linebreak {
                    // Indentation after a line break is fine. (As in Java,
                    // the token that ends it is skipped: a tab there does
                    // not start a run of whitespace.)
                    i += 1;
                    while i < tokens.len() && is_removable_white(&tokens[i]) {
                        i += 1;
                    }
                }
                i += 1;
            }
            pos += sentence.text.len();
        }
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{context, plain};
    use super::*;

    fn check(sentences: &[&str]) -> Vec<(usize, usize, String)> {
        let sentences: Vec<Sentence> = sentences.iter().map(|s| plain(s)).collect();
        MultipleWhitespaceRule::new()
            .match_text(0, &sentences, &context())
            .into_iter()
            .map(|m| (m.from_pos, m.to_pos, m.suggestions[0].clone()))
            .collect()
    }

    #[test]
    fn repeated_whitespace() {
        assert_eq!(check(&["Il  dort."]), vec![(2, 4, " ".to_string())]);
        assert_eq!(check(&["Il dort.  ", "Elle \u{a0} lit."]), vec![(8, 10, " ".to_string()), (14, 18, " ".to_string())]);
        assert!(check(&["Il dort. ", "Elle lit."]).is_empty());
    }

    #[test]
    fn line_breaks_tabs_and_indentation() {
        assert!(check(&["Il\n  dort."]).is_empty());
        assert!(check(&["Il \tdort."]).is_empty());
        assert_eq!(check(&["Il\t  dort."]), vec![(2, 5, "\t".to_string())]);
        assert_eq!(check(&["Il\n  dort  bien."]), vec![(9, 11, " ".to_string())]);
        // The tab that ends the indentation is skipped.
        assert_eq!(check(&["Il\n \t  dort."]), vec![(5, 7, " ".to_string())]);
    }
}
