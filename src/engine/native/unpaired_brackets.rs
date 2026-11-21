//! Port of `org.languagetool.rules.GenericUnpairedBracketsRule` (with
//! `SymbolLocator` and `UnsyncStack`): brackets that are opened and never
//! closed, or closed and never opened, anywhere in the text.

use std::sync::Arc;

use super::super::checker::NativeRule;
use super::super::rule::{RuleContext, RuleInfo, RuleMatch};
use super::super::token::Sentence;
use super::{full_text, java_trim, messages, rule_info, rule_match, PUNCTUATION};
use crate::re::{self, Flags, Regex};

/// `NUMERALS_EN`: what may stand before a closing parenthesis in a list
/// item (`1)`, `a)`, `iv)`).
const NUMERALS_EN: &str =
    "(?i)\\d{1,2}?[a-z']*|M*(D?C{0,3}|C[DM])(L?X{0,3}|X[LC])(V?I{0,3}|I[VX])$";
const PUNCTUATION_RE: &str = "[\\p{Punct}…–—]";
const PUNCTUATION_NO_DOT: &str = "[ldmnstLDMNST]'|[–—\\p{Punct}&&[^.]]";

pub struct GenericUnpairedBracketsRule {
    info: RuleInfo,
    start_symbols: Vec<String>,
    end_symbols: Vec<String>,
    /// For each end symbol: does it occur once among the end symbols?
    unique: Vec<bool>,
    numerals: Arc<Regex>,
    punctuation: Arc<Regex>,
    punctuation_no_dot: Arc<Regex>,
    url_with_parenthesis: Arc<Regex>,
}

#[derive(Clone, Copy, PartialEq)]
enum SymbolType {
    Opening,
    Closing,
}

/// `SymbolLocator`: a symbol and where it stands.
struct Locator<'a> {
    symbol: &'a str,
    symbol_type: SymbolType,
    /// On the rule match stack: the number of matches when it was pushed.
    index: usize,
    /// Byte offset in the text.
    start_pos: usize,
    sentence: usize,
}

impl GenericUnpairedBracketsRule {
    pub fn new(start_symbols: &[&str], end_symbols: &[&str]) -> GenericUnpairedBracketsRule {
        assert_eq!(
            start_symbols.len(),
            end_symbols.len(),
            "different number of start and end symbols"
        );
        let pattern = |src: &str| re::compile(src, Flags::default()).expect("valid pattern");
        GenericUnpairedBracketsRule {
            info: rule_info(
                "UNPAIRED_BRACKETS",
                messages::DESC_UNPAIRED_BRACKETS,
                &PUNCTUATION,
                "typographical",
            ),
            start_symbols: start_symbols.iter().map(|s| s.to_string()).collect(),
            end_symbols: end_symbols.iter().map(|s| s.to_string()).collect(),
            unique: end_symbols
                .iter()
                .map(|s| end_symbols.iter().filter(|t| t == &s).count() == 1)
                .collect(),
            numerals: pattern(NUMERALS_EN),
            punctuation: pattern(PUNCTUATION_RE),
            punctuation_no_dot: pattern(PUNCTUATION_NO_DOT),
            url_with_parenthesis: pattern("https?://.+"),
        }
    }

    fn start_index(&self, symbol: &str) -> Option<usize> {
        self.start_symbols.iter().position(|s| s == symbol)
    }

    fn end_index(&self, symbol: &str) -> Option<usize> {
        self.end_symbols.iter().position(|s| s == symbol)
    }

    /// `isNoException`: smileys and parentheses that belong to a URL are not
    /// brackets.
    fn is_no_exception(&self, sentence: &Sentence, i: usize) -> bool {
        let token: &str = &sentence.tok(i).token;
        let parenthesis = token == ")" || token == "(";
        if i > 0 {
            let prev: &str = &sentence.tok(i - 1).token;
            if prev.contains('(') && self.url_with_parenthesis.is_full_match(prev) {
                return false;
            }
        }
        if i >= 2 {
            let prev_prev: &str = &sentence.tok(i - 2).token;
            let prev: &str = &sentence.tok(i - 1).token;
            if (prev_prev == ":" || prev_prev == ";") && prev == "-" && parenthesis {
                return false;
            }
        }
        if i >= 1 {
            let prev: &str = &sentence.tok(i - 1).token;
            if (prev == ":" || prev == ";") && !sentence.tok(i).whitespace_before && parenthesis {
                return false;
            }
        }
        true
    }

    fn preceded_by_whitespace(&self, sentence: &Sentence, i: usize, j: usize) -> bool {
        if self.start_symbols[j] != self.end_symbols[j] {
            return true;
        }
        let prev = sentence.tok(i - 1);
        prev.is_sent_start
            || sentence.tok(i).whitespace_before
            || self.punctuation_no_dot.is_full_match(&prev.token)
            || self.start_symbols.iter().any(|s| **s == *prev.token)
    }

    fn special_case(&self, sentence: &Sentence, i: usize, j: usize) -> bool {
        if i < sentence.len() - 1 && self.start_symbols[j] == self.end_symbols[j] {
            let next = sentence.tok(i + 1);
            return next.whitespace_before
                || self.punctuation.is_full_match(&next.token)
                || self.end_symbols.iter().any(|s| **s == *next.token)
                || (i >= 1 && sentence.tok(i - 1).token.ends_with('-'))
                || next.token.starts_with('-')
                || &*next.token == "s";
        }
        true
    }

    /// `fillSymbolStack`: is the token `i` of `sentence` the j-th symbol, to
    /// push or to pair with the top of the stack?
    fn fill_symbol_stack<'a>(
        &'a self,
        base: usize,
        sentence: &Sentence,
        sentence_index: usize,
        i: usize,
        j: usize,
        stack: &mut Vec<Locator<'a>>,
    ) -> bool {
        let readings = sentence.tok(i);
        let token: &str = &readings.token;
        let start: &'a str = &self.start_symbols[j];
        let end: &'a str = &self.end_symbols[j];
        if token != start && token != end {
            return false;
        }
        let start_pos = base + readings.start_pos;
        let preceded_by_whitespace = self.preceded_by_whitespace(sentence, i, j);
        let special_case = self.special_case(sentence, i, j);
        let no_exception = self.is_no_exception(sentence, i);
        let closing = |symbol| Locator {
            symbol,
            symbol_type: SymbolType::Closing,
            index: i,
            start_pos,
            sentence: sentence_index,
        };
        if no_exception && preceded_by_whitespace && token == start {
            stack.push(Locator {
                symbol: start,
                symbol_type: SymbolType::Opening,
                index: i,
                start_pos,
                sentence: sentence_index,
            });
            return true;
        }
        if no_exception && (special_case || readings.is_sent_end) && token == end {
            let after_open_parenthesis = stack.last().is_some_and(|top| top.symbol == "(");
            // "1.)" or "a)" in an enumeration is not a closing parenthesis.
            let numbered_with_dot = i > 2
                && end == ")"
                && (sentence.tok(i - 3).has_pos_tag("SENT_START")
                    || sentence.tok(i - 2).whitespace_before)
                && &*sentence.tok(i - 1).token == "."
                && self.numerals.is_full_match(&sentence.tok(i - 2).token)
                && !after_open_parenthesis;
            let numbered = i > 1
                && end == ")"
                && self.numerals.is_full_match(&sentence.tok(i - 1).token)
                && !after_open_parenthesis;
            if numbered_with_dot || numbered {
                return false;
            }
            match stack.last() {
                None => {
                    stack.push(closing(end));
                    return true;
                }
                Some(top) if top.symbol == start => {
                    stack.pop();
                    return true;
                }
                Some(_) => {
                    if self.unique[j] || j == self.end_symbols.len() - 1 {
                        stack.push(closing(end));
                        return true;
                    }
                }
            }
        }
        false
    }

    /// `createMatch`: the match for an unpaired symbol, unless it pairs with
    /// the previous unpaired one (which is then withdrawn) or looks like the
    /// label of a list item.
    fn create_match<'a>(
        &self,
        rule: usize,
        matches: &mut Vec<RuleMatch>,
        match_stack: &mut Vec<Locator<'a>>,
        loc: &Locator<'a>,
        text: &str,
    ) -> Option<RuleMatch> {
        if let (Some(top), Some(index)) = (match_stack.last(), self.end_index(loc.symbol)) {
            if top.symbol == self.start_symbols[index] && matches.len() > top.index {
                matches.remove(top.index);
                match_stack.pop();
                return None;
            }
        }
        match_stack.push(Locator {
            symbol: loc.symbol,
            symbol_type: loc.symbol_type,
            index: matches.len(),
            start_pos: loc.start_pos,
            sentence: loc.sentence,
        });
        let other = match self.start_index(loc.symbol) {
            Some(index) => &self.end_symbols[index],
            None => &self.start_symbols[self.end_index(loc.symbol)?],
        };
        let message = messages::UNPAIRED_BRACKETS.replace("{0}", other);
        let end_pos = loc.start_pos + loc.symbol.len();
        if end_pos < text.len() && loc.symbol == ")" {
            // "b) item" after a line break, "a) item" at the start of the
            // text. Java looks at the two UTF-16 units before the symbol.
            let mut before = text[..loc.start_pos].chars().rev();
            let last = before.next();
            let second_last = before.next();
            let units_before = last.map_or(0, char::len_utf16) + second_last.map_or(0, |_| 1);
            let letter = last.is_some_and(|c| c.is_ascii_alphabetic());
            if units_before >= 2 {
                if letter && second_last == Some('\n') {
                    return None;
                }
            } else if units_before == 1 && letter {
                return None;
            }
        }
        rule_match(rule, loc.start_pos, end_pos, &message, "", Vec::new())
    }
}

/// `endsLikeRealSentence`.
fn ends_like_real_sentence(text: &str) -> bool {
    let s = java_trim(text);
    s.ends_with('.') || s.ends_with('?') || s.ends_with('!')
}

impl NativeRule for GenericUnpairedBracketsRule {
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
        let mut stack: Vec<Locator> = Vec::new();
        let mut base = 0;
        for (sentence_index, sentence) in sentences.iter().enumerate() {
            for i in 1..sentence.len() {
                for j in 0..self.start_symbols.len() {
                    if self.fill_symbol_stack(base, sentence, sentence_index, i, j, &mut stack) {
                        break;
                    }
                }
            }
            base += sentence.text.len();
        }
        let mut matches: Vec<RuleMatch> = Vec::new();
        if stack.is_empty() {
            return matches;
        }
        // An odd number of symbols around a middle one: only that one is
        // reported. (As in Java, every first-half symbol is compared with the
        // last one.)
        let size = stack.len();
        let symmetric = size > 2
            && size % 2 == 1
            && (0..size / 2).all(|i| {
                self.start_index(stack[i].symbol) == self.end_index(stack[size - 1].symbol)
            });
        let text = full_text(sentences);
        let mut match_stack: Vec<Locator> = Vec::new();
        if symmetric {
            let loc = &stack[size / 2];
            if let Some(m) = self.create_match(rule, &mut matches, &mut match_stack, loc, &text) {
                matches.push(m);
            }
        } else {
            for loc in &stack {
                let m = self.create_match(rule, &mut matches, &mut match_stack, loc, &text);
                if let Some(m) = m {
                    // An opening symbol at the end of a text that stops
                    // mid-sentence may still be closed.
                    if loc.symbol_type == SymbolType::Closing
                        || ends_like_real_sentence(&sentences[loc.sentence].text)
                        || sentences.len() - 1 > loc.sentence
                    {
                        matches.push(m);
                    }
                }
            }
        }
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{context, plain};
    use super::*;

    fn rule() -> GenericUnpairedBracketsRule {
        GenericUnpairedBracketsRule::new(&["[", "(", "{"], &["]", ")", "}"])
    }

    fn check(sentences: &[&str]) -> Vec<(usize, usize)> {
        let sentences: Vec<Sentence> = sentences.iter().map(|s| plain(s)).collect();
        rule()
            .match_text(0, &sentences, &context())
            .into_iter()
            .map(|m| (m.from_pos, m.to_pos))
            .collect()
    }

    #[test]
    fn unpaired_symbols() {
        assert!(check(&["Il (le chat) dort."]).is_empty());
        assert_eq!(check(&["Il (le chat dort."]), vec![(3, 4)]);
        assert_eq!(check(&["Il le chat) dort."]), vec![(10, 11)]);
        assert_eq!(check(&["Il [le chat) dort."]), vec![(3, 4), (11, 12)]);
        // Paired across sentences.
        assert!(check(&["Il (le chat. ", "Le chien) dort."]).is_empty());
        assert_eq!(check(&["Il dort. ", "Le chien) dort."]), vec![(17, 18)]);
    }

    #[test]
    fn unfinished_text_smileys_and_list_items() {
        // The text stops before the sentence ends: the parenthesis may still
        // be closed.
        assert!(check(&["Il (le chat"]).is_empty());
        assert_eq!(check(&["Il (le chat. ", "Il dort"]), vec![(3, 4)]);
        assert!(check(&["Bravo :-) à tous."]).is_empty());
        assert!(check(&["Bravo :) à tous."]).is_empty());
        assert!(check(&["a) premier point."]).is_empty());
        assert!(check(&["1) premier point."]).is_empty());
        assert!(check(&["Voici :\n", "b) second point."]).is_empty());
    }

    #[test]
    fn message_names_the_missing_symbol() {
        let m = rule().match_text(0, &[plain("Il (dort.")], &context());
        assert_eq!(
            m[0].message,
            "Pas de correspondance fermante ou ouvrante pour le caractère « ) »"
        );
    }
}
