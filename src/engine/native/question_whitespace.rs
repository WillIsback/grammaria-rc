//! Port of `org.languagetool.rules.fr.QuestionWhitespaceRule` and of its
//! subclass `QuestionWhitespaceStrictRule`: the (non-breaking) space French
//! puts before `? ! ; :` and inside `« »`.
//!
//! The base rule asks for some whitespace; the strict one, tagged `picky`
//! and off by default, for a non-breaking one.

use super::super::checker::NativeRule;
use super::super::rule::{RuleContext, RuleInfo, RuleMatch};
use super::super::token::{Readings, Sentence};
use super::antipattern::{immunized_tokens, AntiPattern, TokenTest};
use super::{rule_info, rule_match, MISC};

/// Espace fine insécable.
const NARROW_NBSP: &str = "\u{202f}";
const NBSP: &str = "\u{a0}";
const SHORT_MESSAGE: &str = "Insérer une espace insécable";

pub struct QuestionWhitespaceRule {
    info: RuleInfo,
    strict: bool,
    anti_patterns: Vec<AntiPattern>,
}

impl QuestionWhitespaceRule {
    /// The strict rule (`FRENCH_WHITESPACE_STRICT`) or the base one
    /// (`FRENCH_WHITESPACE`).
    pub fn new(strict: bool) -> QuestionWhitespaceRule {
        let id = if strict { "FRENCH_WHITESPACE_STRICT" } else { "FRENCH_WHITESPACE" };
        let mut info = rule_info(id, "Insertion des espaces fines insécables", &MISC, "uncategorized");
        if strict {
            info.tags = vec!["picky".to_string()];
            info.default_temp_off = true;
        }
        let not_after_space = |test: TokenTest| test.whitespace_before(false);
        let anti_patterns = vec![
            // smileys such as :-) and :)
            AntiPattern::new(vec![
                TokenTest::token_regex("[:;]"),
                not_after_space(TokenTest::cs_token("-")),
                not_after_space(TokenTest::token_regex("[\\(\\)D]")),
            ]),
            AntiPattern::new(vec![TokenTest::token_regex("[:;]"), not_after_space(TokenTest::token_regex("[\\(\\)D]"))]),
            // times like 23:20
            AntiPattern::new(vec![TokenTest::token_regex(".*\\d{1,2}"), TokenTest::token(":"), TokenTest::token_regex("\\d{1,2}")]),
            // "??"
            AntiPattern::new(vec![TokenTest::token_regex("[?!]"), TokenTest::token_regex("[?!]")]),
            // MAC addresses
            AntiPattern::new(vec![
                TokenTest::token_regex("[a-z0-9]{2}"),
                TokenTest::token(":"),
                TokenTest::token_regex("[a-z0-9]{2}"),
                TokenTest::token(":"),
                TokenTest::token_regex("[a-z0-9]{2}"),
            ]),
            // CSV data
            AntiPattern::new(vec![
                TokenTest::token(";"),
                not_after_space(TokenTest::token_regex(".+")),
                not_after_space(TokenTest::token(";")),
            ]),
            AntiPattern::new(vec![
                not_after_space(TokenTest::token_regex(".+")),
                not_after_space(TokenTest::token(";")),
                not_after_space(TokenTest::token_regex(".+")),
            ]),
        ];
        QuestionWhitespaceRule { info, strict, anti_patterns }
    }

    /// `isAllowedWhitespaceChar`: may this token stand before the mark? Any
    /// whitespace for the base rule; for the strict one a non-breaking space,
    /// or no whitespace at all (which the base rule reports).
    fn is_allowed_whitespace(&self, token: &Readings) -> bool {
        if self.strict {
            matches!(&*token.token, NARROW_NBSP | NBSP) || !token.is_whitespace
        } else {
            token.is_whitespace
        }
    }
}

/// `^(file|s?ftp|...)$`: the word before a colon is a URL scheme.
fn is_url_scheme(token: &str) -> bool {
    matches!(
        token,
        "file" | "ftp" | "sftp" | "finger" | "git" | "gopher" | "hdl" | "http" | "https" | "shttp" | "imap" | "mailto" | "mms" | "nntp"
            | "news" | "snews" | "newspost" | "snewspost" | "newsreply" | "snewsreply" | "prospero" | "rsync" | "rtspu" | "sip" | "sips"
            | "svn" | "svn+ssh" | "telnet" | "wais"
    )
}

impl NativeRule for QuestionWhitespaceRule {
    fn info(&self) -> &RuleInfo {
        &self.info
    }

    fn match_sentence(&self, rule: usize, sentence: &Sentence, _ctx: &RuleContext) -> Vec<RuleMatch> {
        let mut matches = Vec::new();
        let tokens = &sentence.tokens;
        // Every match is on one of these marks, or right after «.
        if !tokens.iter().any(|t| matches!(&*t.token, "?" | "!" | ";" | ":" | "»" | "«")) {
            return matches;
        }
        let immunized = immunized_tokens(sentence, &self.anti_patterns);
        let mut prev_prev_token: &str = "";
        let mut prev_token: &str = "";
        for i in 1..tokens.len() {
            let token: &str = &tokens[i].token;
            if immunized[i] || prev_token == "(" || prev_token == "[" {
                prev_prev_token = prev_token;
                prev_token = token;
                continue;
            }
            let mut msg: Option<&str> = None;
            let mut suggestion = String::new();
            let mut i_from = i - 1;
            let mut i_to = i;
            let prev_token_to_change = if tokens[i - 1].is_whitespace { "" } else { prev_token };
            if !self.is_allowed_whitespace(&tokens[i - 1]) {
                if token == "?" && prev_token != "!" {
                    msg = Some("Le point d'interrogation est précédé d'une espace fine insécable.");
                    suggestion = format!("{prev_token_to_change}{NARROW_NBSP}?");
                } else if token == "!" && prev_token != "?" {
                    msg = Some("Le point d'exclamation est précédé d'une espace fine insécable.");
                    suggestion = format!("{prev_token_to_change}{NARROW_NBSP}!");
                } else if token == ";" {
                    msg = Some("Le point-virgule est précédé d'une espace fine insécable.");
                    suggestion = format!("{prev_token_to_change}{NARROW_NBSP};");
                } else if token == ":" {
                    if !is_url_scheme(prev_token) {
                        msg = Some("Les deux-points sont précédés d'une espace insécable.");
                        suggestion = format!("{prev_token_to_change}{NBSP}:");
                    }
                } else if token == "»" {
                    if prev_prev_token == "«" {
                        msg = Some("Les guillemets sont toujours accompagnés d'une espace insécable.");
                        suggestion = format!("«{NBSP}{prev_token_to_change}{NBSP}»");
                        i_from = i - 2;
                    } else {
                        msg = Some("Le guillemet fermant est précédé d'une espace insécable.");
                        suggestion = format!("{prev_token_to_change}{NBSP}»");
                    }
                }
            }
            if prev_token == "«" {
                if token.is_empty() {
                    msg = Some("Le guillemet ouvrant est suivi d'une espace insécable.");
                    suggestion = format!("«{NBSP}");
                    i_to = i - 1;
                } else if !self.is_allowed_whitespace(&tokens[i]) {
                    let next_token: &str = tokens.get(i + 1).map_or("", |t| &t.token);
                    if next_token != "»" {
                        msg = Some("Le guillemet ouvrant est suivi d'une espace insécable.");
                        suggestion = if tokens[i].is_whitespace { format!("«{NBSP}") } else { format!("«{NBSP}{token}") };
                    }
                }
            }
            if let Some(msg) = msg {
                matches.extend(rule_match(rule, tokens[i_from].start_pos, tokens[i_to].end_pos(), msg, SHORT_MESSAGE, vec![suggestion]));
            }
            prev_prev_token = prev_token;
            prev_token = token;
        }
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{context, plain};
    use super::*;

    fn check_with(strict: bool, text: &str) -> Vec<(usize, usize, String)> {
        QuestionWhitespaceRule::new(strict)
            .match_sentence(0, &plain(text), &context())
            .into_iter()
            .map(|m| (m.from_pos, m.to_pos, m.suggestions[0].clone()))
            .collect()
    }

    fn check(text: &str) -> Vec<(usize, usize, String)> {
        check_with(false, text)
    }

    #[test]
    fn marks_without_a_space_before() {
        assert_eq!(check("Comment ça va?"), vec![(12, 15, "va\u{202f}?".to_string())]);
        assert_eq!(check("Super! Voici: un"), vec![(0, 6, "Super\u{202f}!".to_string()), (7, 13, "Voici\u{a0}:".to_string())]);
        assert!(check("Comment ça va ?").is_empty());
        assert!(check("Comment ça va\u{202f}? Bien\u{a0}!").is_empty());
        assert!(check("Quoi ?!").is_empty());
    }

    #[test]
    fn quotation_marks() {
        assert_eq!(check("Il dit «oui» ici"), vec![(7, 14, "«\u{a0}oui\u{a0}»".to_string())]);
        assert_eq!(check("Il dit «oui ici"), vec![(7, 12, "«\u{a0}oui".to_string())]);
        assert_eq!(check("Il dit « oui» ici"), vec![(10, 15, "oui\u{a0}»".to_string())]);
        assert!(check("Il dit « oui » ici").is_empty());
    }

    #[test]
    fn antipatterns_and_urls() {
        assert!(check("Bravo:-) et:) et;D").is_empty());
        assert!(check("Il est 23:20").is_empty());
        assert!(check("Quoi?? Non!!").is_empty());
        assert!(check("adresse 00:1a:2b").is_empty());
        assert!(check("un;deux;trois").is_empty());
        assert!(check("voir http://exemple").is_empty());
        assert!(check("(: oui").is_empty());
    }

    #[test]
    fn strict_rule_wants_non_breaking_spaces() {
        assert_eq!(check_with(true, "Comment ça va ?"), vec![(14, 16, "\u{202f}?".to_string())]);
        assert!(check_with(true, "Comment ça va\u{202f}?").is_empty());
        // No space at all is the base rule's business.
        assert!(check_with(true, "Comment ça va?").is_empty());
        assert_eq!(check_with(true, "Il dit « oui"), vec![(7, 10, "«\u{a0}".to_string())]);
        let rule = QuestionWhitespaceRule::new(true);
        assert!(rule.info().default_temp_off && rule.info().tags == vec!["picky".to_string()]);
    }
}
