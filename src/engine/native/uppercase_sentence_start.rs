//! Port of `org.languagetool.rules.UppercaseSentenceStartRule`: a sentence
//! that starts with a lower-case letter. Uses `WordTokenizer.isUrl` and
//! `isEMail` and `StringTools.isCamelCase`.

use std::sync::Arc;

use super::super::checker::NativeRule;
use super::super::rule::{RuleContext, RuleInfo, RuleMatch};
use super::super::strtools;
use super::super::token::Sentence;
use super::{is_line_terminator, messages, rule_info, rule_match, CASING};
use crate::re::{self, Flags, Regex};

/// A list label: one letter or a Roman numeral (the pattern also accepts
/// the empty string).
const NUMERALS_EN: &str = "[a-z]|(m{0,4}(c[md]|d?c{0,3})(x[cl]|l?x{0,3})(i[xv]|v?i{0,3}))$";
const NO_PROTOCOL_URL: &str =
    "([a-zA-Z0-9][a-zA-Z0-9-]+\\.)?([a-zA-Z0-9][a-zA-Z0-9-]+)\\.([a-zA-Z0-9][a-zA-Z0-9-]+)/.*";
/// `WordTokenizer.E_MAIL` without its leading `(?<!:)`, which cannot fail
/// when the whole token has to match.
const E_MAIL: &str =
    "@?\\b[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@((\\[[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}\\])|(([a-zA-Z\\-0-9]+\\.)+[a-zA-Z]{2,}))\\b";
/// `WordTokenizer.getProtocols`.
const PROTOCOLS: [&str; 19] = [
    "http", "https", "ws", "wss", "ftp", "ftps", "sftp", "file", "mailto", "tel", "sms", "git",
    "ssh", "data", "magnet", "smb", "slack", "spotify", "magnet",
];
/// Words that keep their lower case at the start of a sentence.
const EXCEPTIONS: [&str; 8] = ["n", "w", "x86", "ⓒ", "ø", "cc", "pH", "heylogin"];
const QUOTE_STARTS: [&str; 9] = ["\"", "'", "„", "»", "«", "“", "‘", "¡", "¿"];

pub struct UppercaseSentenceStartRule {
    info: RuleInfo,
    numerals: Arc<Regex>,
    no_protocol_url: Arc<Regex>,
    e_mail: Arc<Regex>,
}

impl UppercaseSentenceStartRule {
    pub fn new() -> UppercaseSentenceStartRule {
        let pattern = |src: &str| re::compile(src, Flags::default()).expect("valid pattern");
        UppercaseSentenceStartRule {
            info: rule_info(
                "UPPERCASE_SENTENCE_START",
                messages::DESC_UPPERCASE_SENTENCE,
                &CASING,
                "typographical",
            ),
            numerals: pattern(NUMERALS_EN),
            no_protocol_url: pattern(NO_PROTOCOL_URL),
            e_mail: pattern(E_MAIL),
        }
    }

    /// `WordTokenizer.isUrl`.
    fn is_url(&self, token: &str) -> bool {
        if token.starts_with("www.") {
            return true;
        }
        if PROTOCOLS.iter().any(|p| {
            token
                .strip_prefix(p)
                .is_some_and(|rest| rest.starts_with("://"))
        }) {
            return true;
        }
        // The pattern needs a dot and a slash.
        token.contains('/') && self.no_protocol_url.is_full_match(token)
    }

    /// `WordTokenizer.isEMail`.
    fn is_e_mail(&self, token: &str) -> bool {
        token.contains('@') && self.e_mail.is_full_match(token)
    }
}

impl Default for UppercaseSentenceStartRule {
    fn default() -> Self {
        Self::new()
    }
}

/// `[.?!…]|`: the previous paragraph ended a sentence, or there is none.
fn ends_sentence_or_empty(s: &str) -> bool {
    s.is_empty() || is_sentence_end(s)
}

fn is_sentence_end(word: &str) -> bool {
    matches!(word, "." | "?" | "!" | "…")
}

/// `[ "'„«»‘’“”\n]`.
fn is_whitespace_or_quote(s: &str) -> bool {
    matches!(
        s,
        " " | "\"" | "'" | "„" | "«" | "»" | "‘" | "’" | "“" | "”" | "\n"
    )
}

/// `.*\d.*` (ASCII digits; `.` stops at line breaks).
fn contains_digit(s: &str) -> bool {
    s.bytes().any(|b| b.is_ascii_digit()) && !s.chars().any(is_line_terminator)
}

/// `[a-z][A-Z].*`: words like `iPhone`.
fn only_lowercase_start(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some('a'..='z'), Some('A'..='Z'))
    ) && !chars.any(is_line_terminator)
}

/// `StringTools.isCamelCase`: `[a-z]+[A-Z][A-Za-z]+`.
fn is_camel_case(s: &str) -> bool {
    let bytes = s.as_bytes();
    let lower = bytes.iter().take_while(|b| b.is_ascii_lowercase()).count();
    lower > 0
        && bytes.len() > lower + 1
        && bytes[lower].is_ascii_uppercase()
        && bytes[lower + 1..].iter().all(|b| b.is_ascii_alphabetic())
}

/// `\d+\. .*`: the sentence is an item of a numbered list.
fn is_digit_dot(text: &str) -> bool {
    let digits = text.bytes().take_while(|b| b.is_ascii_digit()).count();
    digits > 0
        && text[digits..]
            .strip_prefix(". ")
            .is_some_and(|rest| !rest.chars().any(is_line_terminator))
}

/// `.*\n\d+\. `: the sentence ends with the number of the next list item.
fn is_linebreak_digit_dot(text: &str) -> bool {
    let Some(rest) = text.strip_suffix(". ") else {
        return false;
    };
    let without_digits = rest.trim_end_matches(|c: char| c.is_ascii_digit());
    if without_digits.len() == rest.len() {
        return false;
    }
    without_digits
        .strip_suffix('\n')
        .is_some_and(|before| !before.chars().any(is_line_terminator))
}

impl NativeRule for UppercaseSentenceStartRule {
    fn info(&self) -> &RuleInfo {
        &self.info
    }

    fn is_text_level(&self) -> bool {
        true
    }

    fn match_text(
        &self,
        rule: usize,
        sentences: &[Sentence],
        _ctx: &RuleContext,
    ) -> Vec<RuleMatch> {
        let mut matches = Vec::new();
        if sentences.len() == 1 && sentences[0].tokens.len() == 2 {
            return matches;
        }
        let mut last_paragraph_string: &str = "";
        let mut pos = 0;
        let mut is_prev_sentence_numbered_list = false;
        for sentence in sentences {
            let n = sentence.len();
            if n < 2 {
                return matches;
            }
            let first_token_obj = sentence.tok(1);
            let first_token: &str = &first_token_obj.token;
            let mut match_token_pos = 1; // 0 is SENT_START
            if n >= 3 && QUOTE_STARTS.contains(&first_token) {
                match_token_pos = 2;
            }
            let match_token = sentence.tok(match_token_pos);
            let check_token: &str = &match_token.token;
            let mut last_token: &str = &sentence.tok(n - 1).token;
            if is_whitespace_or_quote(last_token) {
                last_token = &sentence.tok(n - 2).token;
            }
            let mut prevent_error = false;
            if last_paragraph_string == "," || last_paragraph_string == ";" {
                prevent_error = true;
            }
            if contains_digit(check_token) {
                prevent_error = true;
            }
            if !ends_sentence_or_empty(last_paragraph_string) && !is_sentence_end(last_token) {
                prevent_error = true;
            }
            // `getText().replace('\u00A0', ' ').trim()` is not empty.
            if sentence.text.chars().any(|c| c > ' ' && c != '\u{a0}') {
                last_paragraph_string = last_token;
            }
            if match_token_pos + 1 < n
                && matches!(&*sentence.tok(match_token_pos + 1).token, "." | ")")
                && self.numerals.is_full_match(check_token)
            {
                prevent_error = true;
            }
            if is_prev_sentence_numbered_list
                || first_token_obj.immunized
                || match_token.has_pos_tag("_IS_URL")
            {
                prevent_error = true;
            }
            // `charAt(0)`: a character outside the BMP is never lower case.
            let lowercase_start = check_token
                .chars()
                .next()
                .is_some_and(|c| c.len_utf16() == 1 && c.is_lowercase());
            if lowercase_start
                && !prevent_error
                && !self.is_url(check_token)
                && !self.is_e_mail(check_token)
            {
                let capitalized = strtools::uppercase_first_char(check_token);
                if capitalized != check_token
                    && !only_lowercase_start(check_token)
                    && !EXCEPTIONS.contains(&check_token)
                    && !is_camel_case(check_token)
                {
                    matches.extend(rule_match(
                        rule,
                        pos + match_token.start_pos,
                        pos + match_token.end_pos(),
                        messages::INCORRECT_CASE,
                        messages::CATEGORY_CASE,
                        vec![capitalized],
                    ));
                }
            }
            pos += sentence.text.len();
            is_prev_sentence_numbered_list =
                is_digit_dot(&sentence.text) || is_linebreak_digit_dot(&sentence.text);
        }
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{context, plain, sentence};
    use super::*;

    fn check_sentences(sentences: &[Sentence]) -> Vec<(usize, usize, String)> {
        UppercaseSentenceStartRule::new()
            .match_text(0, sentences, &context())
            .into_iter()
            .map(|m| (m.from_pos, m.to_pos, m.suggestions[0].clone()))
            .collect()
    }

    fn check(sentences: &[&str]) -> Vec<(usize, usize, String)> {
        let sentences: Vec<Sentence> = sentences.iter().map(|s| plain(s)).collect();
        check_sentences(&sentences)
    }

    #[test]
    fn lowercase_sentence_starts() {
        assert_eq!(
            check(&["bonjour. ", "comment ça va ?"]),
            vec![
                (0, 7, "Bonjour".to_string()),
                (9, 16, "Comment".to_string())
            ]
        );
        assert_eq!(
            check(&["« été comme hiver. »"]),
            vec![(3, 8, "Été".to_string())]
        );
        assert!(check(&["Bonjour. ", "Comment ça va ?"]).is_empty());
        // A lone word is not a sentence.
        assert!(check(&["bonjour"]).is_empty());
    }

    #[test]
    fn what_prevents_the_error() {
        // The previous sentence does not end one, and neither does this one.
        assert!(check(&["Voici la liste\n", "premier point\n"]).is_empty());
        assert_eq!(
            check(&["Voici la liste\n", "premier point.\n"]),
            vec![(15, 22, "Premier".to_string())]
        );
        assert!(check(&["Il dit,\n", "premier point."]).is_empty());
        // List labels, numbered lists, words with digits or inner capitals.
        assert!(check(&["a) premier point."]).is_empty());
        assert!(check(&["iv. premier point."]).is_empty());
        assert!(check(&["1. Premier point. ", "second point."]).is_empty());
        assert!(check(&["x86 est une architecture."]).is_empty());
        assert!(check(&["iPhone est une marque."]).is_empty());
        assert!(check(&["eBay est un site."]).is_empty());
        assert!(check(&["pH neutre."]).is_empty());
        assert!(check_sentences(&[sentence(&[
            "www.example.org",
            " ",
            "est",
            " ",
            "un",
            " ",
            "site",
            "."
        ])])
        .is_empty());
        assert!(check_sentences(&[sentence(&[
            "jean@example.org",
            " ",
            "est",
            " ",
            "une",
            " ",
            "adresse",
            "."
        ])])
        .is_empty());
        assert!(check_sentences(&[sentence(&["*bonjour", " ", "à", " ", "tous", "."])]).is_empty());
    }

    #[test]
    fn list_item_patterns() {
        assert!(is_digit_dot("12. Un point."));
        assert!(!is_digit_dot("12. Un point.\n"));
        assert!(!is_digit_dot("a. Un point."));
        assert!(is_linebreak_digit_dot("Un point.\n2. "));
        assert!(!is_linebreak_digit_dot("Un\npoint.\n2. "));
        assert!(!is_linebreak_digit_dot("Un point. 2. "));
        assert!(
            is_camel_case("eBay")
                && is_camel_case("iPhone")
                && !is_camel_case("ebay")
                && !is_camel_case("eB")
        );
    }
}
