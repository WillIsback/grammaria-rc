//! Port of `org.languagetool.rules.SentenceWhitespaceRule`: no space, or too
//! many spaces, between two sentences.

use super::super::checker::NativeRule;
use super::super::rule::{RuleContext, RuleInfo, RuleMatch};
use super::super::token::{Readings, Sentence};
use super::{messages, rule_info, rule_match, TYPOGRAPHY};

pub struct SentenceWhitespaceRule {
    info: RuleInfo,
    max_spaces_between_sentences: usize,
}

impl SentenceWhitespaceRule {
    pub fn new(max_spaces_between_sentences: usize) -> SentenceWhitespaceRule {
        SentenceWhitespaceRule {
            info: rule_info("SENTENCE_WHITESPACE", messages::MISSING_SPACE_BETWEEN_SENTENCES, &TYPOGRAPHY, "whitespace"),
            max_spaces_between_sentences,
        }
    }

    /// `addRepeatedWhitespaceMatches`: several spaces after a sentence-ending
    /// mark inside one sentence.
    fn add_repeated_whitespace_matches(&self, rule: usize, tokens: &[Readings], pos: usize, matches: &mut Vec<RuleMatch>) {
        let mut i = 1;
        while i < tokens.len() {
            if is_space_token(&tokens[i]) && follows_sentence_end(tokens, i) {
                let first = i;
                let mut last = i;
                i += 1;
                while i < tokens.len() && is_space_token(&tokens[i]) {
                    last = i;
                    i += 1;
                }
                i -= 1;
                // Space tokens hold spaces only: their length is the same in
                // bytes and in UTF-16 units.
                let length: usize = tokens[first..=last].iter().map(|t| t.token.len()).sum();
                let followed_by_text = last + 1 < tokens.len() && !tokens[last + 1].is_whitespace && !is_line_break_token(&tokens[last + 1]);
                if length > self.max_spaces_between_sentences && followed_by_text {
                    matches.extend(rule_match(
                        rule,
                        pos + tokens[first].start_pos,
                        pos + tokens[last].end_pos(),
                        messages::WHITESPACE_REPETITION,
                        "",
                        vec![" ".to_string()],
                    ));
                }
            }
            i += 1;
        }
    }
}

fn is_only_spaces(token: &str) -> bool {
    token.bytes().all(|b| b == b' ')
}

/// A token made of spaces only (the empty sentence start included).
fn is_space_token(token: &Readings) -> bool {
    is_only_spaces(&token.token)
}

fn follows_sentence_end(tokens: &[Readings], whitespace_pos: usize) -> bool {
    for i in (1..whitespace_pos).rev() {
        if !tokens[i].is_whitespace {
            return matches!(&*tokens[i].token, "." | "!" | "?");
        }
    }
    false
}

/// `StringTools.containsLineBreak`.
fn contains_line_break(token: &str) -> bool {
    token.contains(['\n', '\r', '\u{c}', '\u{b}', '\u{85}', '\u{2028}', '\u{2029}'])
}

fn is_line_break_token(token: &Readings) -> bool {
    token.is_linebreak || contains_line_break(&token.token)
}

fn leading_spaces_length(tokens: &[Readings]) -> usize {
    tokens.iter().skip(1).take_while(|t| is_space_token(t)).map(|t| t.token.len()).sum()
}

fn has_text_after_leading_spaces(tokens: &[Readings], leading_spaces_length: usize) -> bool {
    let mut pos = 1;
    let mut spaces_length = 0;
    while pos < tokens.len() && spaces_length < leading_spaces_length {
        spaces_length += tokens[pos].token.len();
        pos += 1;
    }
    pos < tokens.len() && !tokens[pos].is_whitespace && !is_line_break_token(&tokens[pos])
}

impl NativeRule for SentenceWhitespaceRule {
    fn info(&self) -> &RuleInfo {
        &self.info
    }

    fn is_text_level(&self) -> bool {
        true
    }

    fn match_text(&self, rule: usize, sentences: &[Sentence], _ctx: &RuleContext) -> Vec<RuleMatch> {
        let mut matches = Vec::new();
        let mut is_first_sentence = true;
        // The whitespace token that ends the previous sentence, if any.
        let mut prev_ending_whitespace: &str = "";
        let mut prev_ends_with_line_break = false;
        let mut pos = 0;
        for sentence in sentences {
            let tokens = &sentence.tokens;
            self.add_repeated_whitespace_matches(rule, tokens, pos, &mut matches);
            if is_first_sentence {
                is_first_sentence = false;
            } else if !prev_ends_with_line_break && !(tokens.len() > 1 && is_line_break_token(&tokens[1])) {
                let leading = leading_spaces_length(tokens);
                if is_only_spaces(prev_ending_whitespace)
                    && prev_ending_whitespace.len() + leading > self.max_spaces_between_sentences
                    && (!prev_ending_whitespace.is_empty() || leading > 0)
                    && has_text_after_leading_spaces(tokens, leading)
                {
                    matches.extend(rule_match(
                        rule,
                        pos - prev_ending_whitespace.len(),
                        pos + leading,
                        messages::WHITESPACE_REPETITION,
                        "",
                        vec![" ".to_string()],
                    ));
                } else if prev_ending_whitespace.is_empty() && tokens.len() > 1 {
                    let first_token: &str = &tokens[1].token;
                    matches.extend(rule_match(
                        rule,
                        pos,
                        pos + first_token.len(),
                        messages::ADD_SPACE_BETWEEN_SENTENCES,
                        "",
                        vec![format!(" {first_token}")],
                    ));
                }
            }
            if let Some(last) = tokens.last() {
                // `lastToken.replace(' ', ' ').trim().isEmpty()`
                let blank = last.token.chars().all(|c| c <= ' ' || c == '\u{a0}');
                prev_ending_whitespace = if blank { &last.token } else { "" };
                prev_ends_with_line_break = is_line_break_token(last);
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
        SentenceWhitespaceRule::new(1)
            .match_text(0, &sentences, &context())
            .into_iter()
            .map(|m| (m.from_pos, m.to_pos, m.suggestions[0].clone()))
            .collect()
    }

    #[test]
    fn missing_space_between_sentences() {
        assert_eq!(check(&["Il dort.", "Elle lit."]), vec![(8, 12, " Elle".to_string())]);
        assert!(check(&["Il dort. ", "Elle lit."]).is_empty());
        assert!(check(&["Il dort.\n", "Elle lit."]).is_empty());
        assert!(check(&["Il dort.", "\nElle lit."]).is_empty());
        // A non-breaking space between sentences is a space.
        assert!(check(&["Il dort.\u{a0}", "Elle lit."]).is_empty());
    }

    #[test]
    fn too_many_spaces() {
        // Each space is a token: the last one of the first sentence and the
        // first ones of the second add up.
        assert_eq!(check(&["Il dort. ", "  Elle lit."]), vec![(8, 11, " ".to_string())]);
        // Inside a sentence, after a sentence-ending mark.
        assert_eq!(check(&["Il dort !  Elle lit."]), vec![(9, 11, " ".to_string())]);
        assert!(check(&["Il dort  bien."]).is_empty());
    }
}
