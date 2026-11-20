//! Port of `org.languagetool.rules.LongSentenceRule`: a sentence, or a part
//! of it between colons and semicolons, with more words than the limit.
//! Tagged `picky`: it does not run at the default level.

use super::super::checker::NativeRule;
use super::super::rule::{RuleContext, RuleInfo, RuleMatch};
use super::super::token::{Readings, Sentence};
use super::{messages, rule_info, rule_match, STYLE};

const OPENING_QUOTES: [&str; 8] = ["\"", "“", "„", "«", "(", "[", "{", "—"];
const CLOSING_QUOTES: [&str; 8] = ["\"", "”", "“", "»", ")", "]", "}", "—"];

pub struct LongSentenceRule {
    info: RuleInfo,
    max_words: usize,
    message: String,
}

impl LongSentenceRule {
    pub fn new(max_words: usize) -> LongSentenceRule {
        let limit = max_words.to_string();
        let mut info = rule_info("TOO_LONG_SENTENCE", &messages::LONG_SENTENCE_RULE_DESC.replace("{0}", &limit), &STYLE, "style");
        info.tags = vec!["picky".to_string()];
        LongSentenceRule { info, max_words, message: messages::LONG_SENTENCE_RULE_MSG2.replace("{0}", &limit) }
    }
}

/// `isWordCount`: the token starts with a letter. (Java tests the first
/// UTF-16 unit: half of a character outside the BMP is not a letter.)
fn is_word_count(token: &str) -> bool {
    token.chars().next().is_some_and(|c| c.len_utf16() == 1 && c.is_alphabetic())
}

/// `[?!.]["“”„»«]`: a sentence end inside a quotation.
fn has_quoted_sentence_end(text: &str) -> bool {
    let mut prev_ends = false;
    for c in text.chars() {
        if prev_ends && matches!(c, '"' | '“' | '”' | '„' | '»' | '«') {
            return true;
        }
        prev_ends = matches!(c, '?' | '!' | '.');
    }
    false
}

impl NativeRule for LongSentenceRule {
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
            if tokens.len() < self.max_words || has_quoted_sentence_end(&sentence.text) {
                pos += sentence.text.len();
                continue;
            }
            let mut spans: Vec<(usize, usize)> = Vec::new();
            let mut from_token: Option<&Readings> = None;
            let mut to_token: Option<&Readings> = None;
            let mut index_of_quote: Option<usize> = None;
            let mut i = 0;
            while i < tokens.len() {
                let mut num_words = 0;
                // One part of the sentence, up to a colon, a semicolon or a
                // line break.
                while i < tokens.len() && !matches!(&*tokens[i].token, ":" | ";" | "\n" | "\r\n" | "\n\r") {
                    let token: &str = &tokens[i].token;
                    match index_of_quote {
                        None => index_of_quote = OPENING_QUOTES.iter().position(|q| *q == token),
                        Some(open) => {
                            if CLOSING_QUOTES.iter().position(|q| *q == token) == Some(open) {
                                index_of_quote = None;
                            }
                        }
                    }
                    if is_word_count(token) && index_of_quote.is_none() {
                        if from_token.is_none() {
                            from_token = Some(&tokens[i]);
                        }
                        if num_words == self.max_words {
                            if to_token.is_none() {
                                // The last word of the sentence, with the
                                // mark that follows it.
                                if let Some(j) = (0..tokens.len()).rev().find(|&j| is_word_count(&tokens[j].token)) {
                                    let followed_by_end = tokens.len() > j + 1 && matches!(&*tokens[j + 1].token, "?" | "!" | ".");
                                    to_token = Some(if followed_by_end { &tokens[j + 1] } else { &tokens[j] });
                                }
                            }
                            match (from_token, to_token) {
                                (Some(from), Some(to)) => spans.push((from.start_pos, to.end_pos())),
                                _ => spans.push((tokens[0].start_pos, tokens[tokens.len() - 1].end_pos())),
                            }
                            break;
                        }
                        num_words += 1;
                    }
                    i += 1;
                }
                i += 1;
            }
            for (from, to) in spans {
                matches.extend(rule_match(rule, pos + from, pos + to, &self.message, "", Vec::new()));
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

    fn check(max_words: usize, text: &str) -> Vec<(usize, usize)> {
        LongSentenceRule::new(max_words).match_text(0, &[plain(text)], &context()).into_iter().map(|m| (m.from_pos, m.to_pos)).collect()
    }

    #[test]
    fn counts_words_up_to_the_limit() {
        assert_eq!(check(5, "Un deux trois quatre cinq six sept."), vec![(0, 35)]);
        assert!(check(7, "Un deux trois quatre cinq six sept.").is_empty());
        // Each part between colons is counted on its own.
        assert!(check(5, "Un deux trois quatre : cinq six sept huit.").is_empty());
        // Words between brackets do not count.
        assert!(check(5, "Un deux (trois quatre cinq) six sept huit.").is_empty());
        // A sentence end inside a quotation: the sentence is left alone.
        assert!(check(5, "Un deux trois quatre cinq six sept.» Huit.").is_empty());
    }

    #[test]
    fn is_picky() {
        let rule = LongSentenceRule::new(40);
        assert_eq!(rule.info().tags, vec!["picky".to_string()]);
        assert!(rule.info().description.contains("40"));
    }
}
