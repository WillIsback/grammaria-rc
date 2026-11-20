//! Antipatterns of Java rules: what `Rule.makeAntiPatterns` builds from
//! `PatternTokenBuilder` tokens and `Rule.getSentenceWithImmunization`
//! applies. An antipattern is a sequence of token tests; the tokens of each
//! of its matches are immunized, and the rule then skips them.
//!
//! Only what the ported rules use of pattern tokens is supported: a token
//! text or regular expression and the whitespace-before condition.

use std::sync::Arc;

use super::super::strtools;
use super::super::token::{Readings, Sentence};
use crate::re::{self, Flags, Regex};

enum Text {
    /// `token(..)` and `csToken(..)`.
    Literal { value: String, case_sensitive: bool },
    /// `tokenRegex(..)`: the whole token matches, case-insensitively.
    Regex(Arc<Regex>),
}

/// One pattern token.
pub struct TokenTest {
    text: Text,
    whitespace_before: Option<bool>,
}

impl TokenTest {
    /// `new PatternTokenBuilder().token(value)`.
    pub fn token(value: &str) -> TokenTest {
        TokenTest { text: Text::Literal { value: value.to_string(), case_sensitive: false }, whitespace_before: None }
    }

    /// `new PatternTokenBuilder().csToken(value)`.
    pub fn cs_token(value: &str) -> TokenTest {
        TokenTest { text: Text::Literal { value: value.to_string(), case_sensitive: true }, whitespace_before: None }
    }

    /// `new PatternTokenBuilder().tokenRegex(source)`.
    pub fn token_regex(source: &str) -> TokenTest {
        TokenTest { text: Text::Regex(re::compile(source, Flags::IGNORE_CASE).expect("valid pattern")), whitespace_before: None }
    }

    /// `setIsWhiteSpaceBefore(value)`.
    pub fn whitespace_before(mut self, value: bool) -> TokenTest {
        self.whitespace_before = Some(value);
        self
    }

    fn matches(&self, token: &Readings) -> bool {
        if self.whitespace_before.is_some_and(|expected| expected != token.whitespace_before) {
            return false;
        }
        match &self.text {
            Text::Literal { value, case_sensitive: true } => **value == *token.token,
            Text::Literal { value, case_sensitive: false } => strtools::equals_ignore_case(value, &token.token),
            Text::Regex(regex) => regex.is_full_match(&token.token),
        }
    }
}

/// A sequence of tests on consecutive non-whitespace tokens.
pub struct AntiPattern {
    tokens: Vec<TokenTest>,
    /// The order to run the tests in: literal texts first, they are the
    /// cheapest to refute.
    order: Vec<usize>,
}

impl AntiPattern {
    pub fn new(tokens: Vec<TokenTest>) -> AntiPattern {
        let mut order: Vec<usize> = (0..tokens.len()).collect();
        order.sort_by_key(|&k| matches!(tokens[k].text, Text::Regex(_)));
        AntiPattern { tokens, order }
    }
}

/// `getSentenceWithImmunization(sentence)`: for each token of the sentence
/// (whitespace included), is it immunized, either already or by a match of
/// one of `patterns`?
pub fn immunized_tokens(sentence: &Sentence, patterns: &[AntiPattern]) -> Vec<bool> {
    let mut immunized: Vec<bool> = sentence.tokens.iter().map(|t| t.immunized).collect();
    let n = sentence.len();
    for pattern in patterns {
        let size = pattern.tokens.len();
        if size == 0 || size > n {
            continue;
        }
        for start in 0..=n - size {
            if pattern.order.iter().all(|&k| pattern.tokens[k].matches(sentence.tok(start + k))) {
                for k in 0..size {
                    immunized[sentence.non_ws[start + k]] = true;
                }
            }
        }
    }
    immunized
}

#[cfg(test)]
mod tests {
    use super::super::testing::plain;
    use super::*;

    #[test]
    fn immunizes_the_matched_tokens() {
        let smiley = AntiPattern::new(vec![TokenTest::token_regex("[:;]"), TokenTest::token_regex("[\\(\\)D]").whitespace_before(false)]);
        let s = plain("Oui :D non : d");
        // SENT_START Oui _ : D _ non _ : _ d
        let flags = immunized_tokens(&s, &[smiley]);
        assert_eq!(flags, vec![false, false, false, true, true, false, false, false, false, false, false]);
    }

    #[test]
    fn literal_tokens() {
        let s = plain("a B");
        assert_eq!(immunized_tokens(&s, &[AntiPattern::new(vec![TokenTest::token("b")])]), vec![false, false, false, true]);
        assert_eq!(immunized_tokens(&s, &[AntiPattern::new(vec![TokenTest::cs_token("b")])]), vec![false; 4]);
    }
}
