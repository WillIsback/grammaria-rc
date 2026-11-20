//! Sentence splitting with SRX rules.
//!
//! Port of `org.languagetool.tokenizers.SRXSentenceTokenizer` and `SrxTools`
//! (LanguageTool, LGPL-2.1-or-later) and of the parts of the `segment`
//! library they use (net.loomchild.segment 2.0.4, MIT): `Srx2SaxParser`,
//! `SrxDocument`, `RuleManager`, `RuleMatcher`, `SrxTextIterator` and
//! `Util.finitize`.
//!
//! An SRX file holds, per language, an ordered list of rules. A rule has a
//! pattern for the text before a position and one for the text after it; a
//! *break* rule cuts the text at the position, unless one of the *no-break*
//! rules listed before it matches at the same position.
//!
//! LanguageTool compiles the patterns with `UNICODE_CHARACTER_CLASS`, so
//! `\s`, `\d`, `\w` and `\b` follow Unicode here (the `(?U)` inline flag).

use std::sync::OnceLock;

use crate::re::{Flags, Regex};
use crate::xml::{Event, Parser};

/// One `<rule>` of an SRX file (`net.loomchild.segment.srx.Rule`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SrxRule {
    pub breaking: bool,
    pub before: String,
    pub after: String,
}

/// A parsed SRX file (`SrxDocument`): named rule lists, and the mapping from
/// language codes to them.
#[derive(Debug)]
pub struct SrxDocument {
    /// When true, every language map that matches a code contributes its
    /// rules, in order; otherwise only the first one does.
    pub cascade: bool,
    language_rules: Vec<(String, Vec<SrxRule>)>,
    /// (language pattern, index into `language_rules`)
    language_maps: Vec<(Regex, usize)>,
}

impl SrxDocument {
    /// `Srx2SaxParser.parse`.
    pub fn parse(xml: &str) -> Result<SrxDocument, String> {
        let mut doc = SrxDocument { cascade: true, language_rules: Vec::new(), language_maps: Vec::new() };
        let mut parser = Parser::new(xml);
        // The element whose text is being read ("beforebreak"/"afterbreak").
        let mut element: Option<String> = None;
        let mut breaking = false;
        let mut before = String::new();
        let mut after = String::new();
        loop {
            match parser.next_event() {
                Event::Eof => break,
                Event::Start { name, attrs, self_closing } => {
                    match name.as_str() {
                        "header" => doc.cascade = Event::attr(&attrs, "cascade") == Some("yes"),
                        "languagerule" => {
                            let rule_name = Event::attr(&attrs, "languagerulename").unwrap_or_default().to_string();
                            doc.language_rules.push((rule_name, Vec::new()));
                        }
                        "languagemap" => {
                            let pattern = Event::attr(&attrs, "languagepattern").unwrap_or_default();
                            let rule_name = Event::attr(&attrs, "languagerulename").unwrap_or_default();
                            // As in the Java map of rule names, the last list of that name wins.
                            let index = doc
                                .language_rules
                                .iter()
                                .rposition(|(n, _)| n == rule_name)
                                .ok_or_else(|| format!("SRX: unknown language rule '{rule_name}'"))?;
                            let re = Regex::new(pattern, Flags::default())
                                .map_err(|e| format!("SRX: bad language pattern '{pattern}': {}", e.0))?;
                            doc.language_maps.push((re, index));
                        }
                        "rule" => breaking = Event::attr(&attrs, "break") != Some("no"),
                        _ => {}
                    }
                    if self_closing {
                        if name == "rule" {
                            let (_, rules) = doc.language_rules.last_mut().ok_or("SRX: rule outside a language rule")?;
                            rules.push(SrxRule { breaking, before: String::new(), after: String::new() });
                        }
                        element = None;
                    } else {
                        element = Some(name);
                    }
                }
                Event::End { name } => {
                    element = None;
                    if name == "rule" {
                        let (_, rules) = doc.language_rules.last_mut().ok_or("SRX: rule outside a language rule")?;
                        rules.push(SrxRule {
                            breaking,
                            before: std::mem::take(&mut before),
                            after: std::mem::take(&mut after),
                        });
                        breaking = false;
                    }
                }
                Event::Text(text) => match element.as_deref() {
                    Some("beforebreak") => before.push_str(&text),
                    Some("afterbreak") => after.push_str(&text),
                    _ => {}
                },
            }
        }
        Ok(doc)
    }

    /// `getLanguageRuleList(languageCode)`: the rule lists whose language
    /// pattern matches the whole code, in file order.
    pub fn language_rule_list(&self, language_code: &str) -> Vec<(&str, &[SrxRule])> {
        let mut out = Vec::new();
        for (pattern, index) in &self.language_maps {
            if pattern.is_full_match(language_code) {
                let (name, rules) = &self.language_rules[*index];
                out.push((name.as_str(), rules.as_slice()));
                if !self.cascade {
                    break;
                }
            }
        }
        out
    }
}

/// `Util.removeBlockQuotes`: turn `\Q...\E` into escaped characters.
fn remove_block_quotes(pattern: &str) -> String {
    let mut out: Vec<char> = Vec::with_capacity(pattern.len());
    let mut quote = false;
    let mut previous = '\0';
    for current in pattern.chars() {
        if quote {
            if previous == '\\' && current == 'E' {
                quote = false;
                // Remove the "\\" added in the previous iteration.
                out.truncate(out.len().saturating_sub(2));
            } else {
                out.push('\\');
                out.push(current);
            }
        } else if previous == '\\' && current == 'Q' {
            quote = true;
            // Remove the "\" added in the previous iteration.
            out.pop();
        } else {
            out.push(current);
        }
        previous = current;
    }
    out.into_iter().collect()
}

/// `Util.finitize`: Java does not accept an unbounded pattern in a
/// look-behind, so `*`, `+` and `{n,}` get the upper bound `infinity`.
pub fn finitize(pattern: &str, infinity: usize) -> String {
    struct Patterns {
        star: Regex,
        plus: Regex,
        range: Regex,
    }
    static P: OnceLock<Patterns> = OnceLock::new();
    let p = P.get_or_init(|| {
        let re = |src: &str| Regex::new(src, Flags::default()).expect("valid pattern");
        Patterns {
            star: re("(?<=(?<!\\\\)(?:\\\\\\\\){0,100})\\*"),
            plus: re("(?<=(?<!\\\\)(?:\\\\\\\\){0,100})(?<![\\?\\*\\+]|\\{[0-9],?[0-9]?\\}?\\})\\+"),
            range: re("(?<=(?<!\\\\)(?:\\\\\\\\){0,100})\\{\\s*([0-9]+)\\s*,\\s*\\}"),
        }
    });
    let finite = remove_block_quotes(pattern);
    let finite = p.star.replace_all(&finite, &format!("{{0,{infinity}}}"));
    let finite = p.plus.replace_all(&finite, &format!("{{1,{infinity}}}"));
    p.range.replace_all(&finite, &format!("{{$1,{infinity}}}"))
}

/// `SrxTextIterator.DEFAULT_MAX_LOOKBEHIND_CONSTRUCT_LENGTH`.
const MAX_LOOKBEHIND_CONSTRUCT_LENGTH: usize = 100;

/// The characters one of which a pattern needs in the text to match, as
/// slots: an ASCII character is its own slot, the others have the slots
/// after 128 in the order of `Segmenter::special`. `None`: nothing is known,
/// the pattern is always run.
type Needs = Option<Box<[u8]>>;

const SLOTS: usize = 192;
const ABSENT: u32 = u32::MAX;

/// Where the characters some rule needs occur in a text: the first and the
/// last position of each slot (`ABSENT` when it does not occur).
struct Occurrences {
    first: [u32; SLOTS],
    last: [u32; SLOTS],
}

impl Occurrences {
    #[inline]
    fn add(&mut self, slot: usize, position: usize) {
        let position = position.min(ABSENT as usize - 1) as u32;
        if self.first[slot] == ABSENT {
            self.first[slot] = position;
        }
        self.last[slot] = position;
    }

    /// Does a needed character occur at all?
    #[inline]
    fn any(&self, needs: &Needs) -> bool {
        needs.as_ref().map_or(true, |slots| slots.iter().any(|&s| self.first[s as usize] != ABSENT))
    }

    /// Does a needed character occur before `position`? (For a pattern that
    /// must match text ending there.)
    #[inline]
    fn any_before(&self, needs: &Needs, position: usize) -> bool {
        needs.as_ref().map_or(true, |slots| slots.iter().any(|&s| (self.first[s as usize] as usize) < position))
    }

    /// Does a needed character occur at `position` or after it? (For a
    /// pattern that must match text starting there.)
    #[inline]
    fn any_from(&self, needs: &Needs, position: usize) -> bool {
        needs.as_ref().map_or(true, |slots| {
            slots.iter().any(|&s| self.last[s as usize] != ABSENT && self.last[s as usize] as usize >= position)
        })
    }
}

struct BreakRule {
    before: Regex,
    /// `None`: an empty pattern.
    after: Option<Regex>,
    /// How many of `Segmenter::exceptions` come before this rule.
    exceptions: usize,
    /// What the before and the after pattern need.
    needs: [Needs; 2],
}

struct ExceptionRule {
    /// `(?<=before)`, with a finite `before`; `None`: an empty pattern.
    before: Option<Regex>,
    after: Option<Regex>,
    needs: [Needs; 2],
    /// The after pattern can run over any length of text (`[^)]*\)`): the
    /// before pattern, whose reach is limited, is then tried first.
    after_unbounded: bool,
}

/// The rules of one language code, ready to split texts (`RuleManager` and
/// `SrxTextIterator`).
pub struct Segmenter {
    breaks: Vec<BreakRule>,
    exceptions: Vec<ExceptionRule>,
    /// The non-ASCII characters some rule needs (see [`Needs`]).
    special: Vec<char>,
}

/// One break rule at work on a text (`RuleMatcher`): the Java `Matcher` of
/// its before pattern, and whether the after pattern follows.
struct RuleMatcher<'r> {
    rule: &'r BreakRule,
    /// Start of the matcher's region: nothing before it is visible.
    from: usize,
    /// Bounds of the previous match of the before pattern.
    first: Option<usize>,
    last: usize,
}

impl RuleMatcher<'_> {
    /// `RuleMatcher.find()`: the next match of the before pattern that the
    /// after pattern follows. False when there is none (`hitEnd()`).
    fn find(&mut self, chars: &[char]) -> bool {
        loop {
            // Matcher.find(): from the end of the previous match, one further
            // when it was empty.
            let mut next = self.last;
            if Some(next) == self.first {
                next += 1;
            }
            let next = next.max(self.from);
            if next > chars.len() {
                return false;
            }
            let Some(m) = self.rule.before.find_chars(&chars[self.from..], next - self.from) else {
                return false;
            };
            let (s, e) = m.spans[0].unwrap_or((0, 0));
            self.first = Some(self.from + s);
            self.last = self.from + e;
            // afterMatcher.region(beforeMatcher.end(), text.length()); lookingAt()
            match &self.rule.after {
                None => return true,
                Some(after) if after.match_at_chars(&chars[self.last..], 0).is_some() => return true,
                _ => {}
            }
        }
    }

    /// `find(start)`: restart in the region that begins at `start`.
    fn find_from(&mut self, start: usize, chars: &[char]) -> bool {
        self.from = start;
        self.first = None;
        self.last = 0;
        self.find(chars)
    }

    #[inline]
    fn start_position(&self) -> usize {
        self.first.unwrap_or(0)
    }

    #[inline]
    fn break_position(&self) -> usize {
        self.last
    }
}

impl Segmenter {
    /// The rules of `language_code` in `document` (`new RuleManager(..)`).
    pub fn new(document: &SrxDocument, language_code: &str) -> Result<Segmenter, String> {
        let compile = |src: &str| {
            // LanguageTool: DEFAULT_PATTERN_FLAGS_PARAMETER = UNICODE_CHARACTER_CLASS
            Regex::new(&format!("(?U){src}"), Flags::default()).map_err(|e| format!("SRX: bad pattern '{src}': {}", e.0))
        };
        let optional = |src: &str| if src.is_empty() { Ok(None) } else { compile(src).map(Some) };
        let mut seg = Segmenter { breaks: Vec::new(), exceptions: Vec::new(), special: Vec::new() };
        for (_, rules) in document.language_rule_list(language_code) {
            for rule in rules {
                // A filter only, taken from the patterns as written.
                let before_needs = optional(&rule.before)?.and_then(|r| seg.needs_of(&r));
                let after = optional(&rule.after)?;
                let after_needs = after.as_ref().and_then(|r| seg.needs_of(r));
                let needs = [before_needs, after_needs];
                if rule.breaking {
                    seg.breaks.push(BreakRule { before: compile(&rule.before)?, after, exceptions: seg.exceptions.len(), needs });
                } else {
                    // RuleManager.createExceptionPatternString
                    let before = finitize(&rule.before, MAX_LOOKBEHIND_CONSTRUCT_LENGTH);
                    let before = if before.is_empty() { None } else { Some(compile(&format!("(?<={before})"))?) };
                    let after_unbounded = after.as_ref().is_some_and(|r| r.max_width().is_none());
                    seg.exceptions.push(ExceptionRule { before, after, needs, after_unbounded });
                }
            }
        }
        Ok(seg)
    }

    /// The slots of the characters `re` requires, if it requires any.
    fn needs_of(&mut self, re: &Regex) -> Needs {
        let mut slots = Vec::new();
        for c in re.required_chars()? {
            let slot = if c.is_ascii() {
                c as usize
            } else {
                match self.special.iter().position(|&x| x == c) {
                    Some(i) => 128 + i,
                    None if self.special.len() < SLOTS - 128 => {
                        self.special.push(c);
                        128 + self.special.len() - 1
                    }
                    None => return None,
                }
            };
            slots.push(slot as u8);
        }
        Some(slots.into_boxed_slice())
    }

    /// Does a no-break rule listed before `rule` match at `position`?
    /// (`!SrxTextIterator.isException`, whose pattern is the alternation of
    /// `(?:(?<=before)(?=after))` over those rules, looked up at the break
    /// position with transparent bounds.)
    fn exception_at(&self, rule: &BreakRule, position: usize, chars: &[char], active: &[usize], seen: &Occurrences) -> bool {
        for &i in active {
            if i >= rule.exceptions {
                break;
            }
            let exception = &self.exceptions[i];
            // What the patterns need must lie on the right side of the position.
            if !seen.any_before(&exception.needs[0], position) || !seen.any_from(&exception.needs[1], position) {
                continue;
            }
            // Both sides must match; the order does not change the outcome,
            // so the cheaper test goes first: what the after pattern can
            // start with, then the side with a limited reach.
            let after = exception.after.as_ref();
            if after.is_some_and(|after| !after.may_start_at(chars, position)) {
                continue;
            }
            let before_fails = || exception.before.as_ref().is_some_and(|before| before.match_at_chars(chars, position).is_none());
            let after_fails = || after.is_some_and(|after| after.match_at_chars(chars, position).is_none());
            let (first, second): (&dyn Fn() -> bool, &dyn Fn() -> bool) =
                if exception.after_unbounded { (&before_fails, &after_fails) } else { (&after_fails, &before_fails) };
            if first() || second() {
                continue;
            }
            return true;
        }
        false
    }

    /// Split `text` into segments (`SrxTextIterator`): the byte ranges of the
    /// segments, which cover the text.
    pub fn split(&self, text: &str) -> Vec<(usize, usize)> {
        let mut seen = Occurrences { first: [ABSENT; SLOTS], last: [ABSENT; SLOTS] };
        let mut chars: Vec<char> = Vec::with_capacity(text.len());
        for c in text.chars() {
            if c.is_ascii() {
                seen.add(c as usize, chars.len());
            } else if let Some(i) = self.special.iter().position(|&x| x == c) {
                seen.add(128 + i, chars.len());
            }
            chars.push(c);
        }
        let len = chars.len();
        // Break positions, as char indices.
        let mut ends: Vec<usize> = Vec::new();
        let mut start = 0;
        let mut end = 0;

        // initMatchers(): the rules that cannot match this text at all are
        // left out straight away.
        let mut matchers: Vec<RuleMatcher> = Vec::new();
        for rule in &self.breaks {
            if !rule.needs.iter().all(|n| seen.any(n)) {
                continue;
            }
            let mut matcher = RuleMatcher { rule, from: 0, first: None, last: 0 };
            if matcher.find(&chars) {
                matchers.push(matcher);
            }
        }
        let mut active_exceptions: Option<Vec<usize>> = None;

        while start < len {
            let mut found = false;
            while !found {
                // getMinMatcher(): the first matcher with the smallest break position
                let mut min: Option<usize> = None;
                for (i, m) in matchers.iter().enumerate() {
                    if min.map_or(true, |j: usize| m.break_position() < matchers[j].break_position()) {
                        min = Some(i);
                    }
                }
                match min {
                    None => {
                        found = true;
                        end = len;
                    }
                    Some(i) => {
                        end = matchers[i].break_position();
                        if end > start {
                            let active = active_exceptions.get_or_insert_with(|| {
                                (0..self.exceptions.len()).filter(|&k| self.exceptions[k].needs.iter().all(|n| seen.any(n))).collect()
                            });
                            found = !self.exception_at(matchers[i].rule, end, &chars, active, &seen);
                            if found {
                                // cutMatchers(): matchers that began before the end of the segment start again there
                                matchers.retain_mut(|m| m.start_position() >= end || m.find_from(end, &chars));
                            }
                        }
                    }
                }
                // moveMatchers(): past the position just handled
                matchers.retain_mut(|m| {
                    while m.break_position() <= end {
                        if !m.find(&chars) {
                            return false;
                        }
                    }
                    true
                });
            }
            ends.push(end);
            start = end;
        }

        // Char positions to byte ranges.
        let mut out = Vec::with_capacity(ends.len());
        let mut byte = 0;
        let mut pos = 0;
        let mut it = text.chars();
        let mut from = 0;
        for end in ends {
            while pos < end {
                byte += it.next().map_or(0, char::len_utf8);
                pos += 1;
            }
            out.push((from, byte));
            from = byte;
        }
        out
    }
}

/// `SRXSentenceTokenizer`: sentence splitting with the rules of
/// `segment.srx` for one language.
pub struct SrxSentenceTokenizer {
    /// Paragraphs end at two line breaks (`<lang>_two`), the default.
    two: Segmenter,
    /// Paragraphs end at every line break (`<lang>_one`).
    one: Segmenter,
}

impl SrxSentenceTokenizer {
    /// `srx` is the content of `segment.srx`, `language` a short code such as
    /// `fr`.
    pub fn new(srx: &str, language: &str) -> Result<SrxSentenceTokenizer, String> {
        let document = SrxDocument::parse(srx)?;
        Ok(SrxSentenceTokenizer {
            two: Segmenter::new(&document, &format!("{language}_two"))?,
            one: Segmenter::new(&document, &format!("{language}_one"))?,
        })
    }

    /// `tokenize(text)` with `setSingleLineBreaksMarksParagraph(false)`, as
    /// LanguageTool does by default: the sentences, which cover the text.
    pub fn tokenize<'t>(&self, text: &'t str) -> Vec<&'t str> {
        self.ranges(text, false).into_iter().map(|(a, b)| &text[a..b]).collect()
    }

    /// Byte ranges of the sentences. `single_line_breaks` is
    /// `setSingleLineBreaksMarksParagraph`.
    pub fn ranges(&self, text: &str, single_line_breaks: bool) -> Vec<(usize, usize)> {
        if single_line_breaks {
            self.one.split(text)
        } else {
            self.two.split(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRX: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<srx xmlns="http://www.lisa.org/srx20" version="2.0">
<header segmentsubflows="yes" cascade="yes"></header>
<body>
<languagerules>
<languagerule languagerulename="Important">
<rule break="no"><beforebreak>\bwww\.</beforebreak><afterbreak>\w</afterbreak></rule>
</languagerule>
<languagerule languagerulename="TwoBreaks">
<rule break="yes"><beforebreak>\r?\n[ \t\n\x0B\f\r]*\r?\n[\t]*</beforebreak><afterbreak></afterbreak></rule>
</languagerule>
<languagerule languagerulename="French">
<!-- a comment -->
<rule break="no"><beforebreak>\b(M|etc)\.\s</beforebreak><afterbreak></afterbreak></rule>
<rule break="no"><beforebreak>\([^\)]*?[\.:]\s</beforebreak><afterbreak>[^\)\r\n]*\)</afterbreak></rule>
<rule break="no"><beforebreak>\p{Ll}.*</beforebreak><afterbreak>p|al</afterbreak></rule>
<rule break="no"><beforebreak>(\!|\?)\s</beforebreak><afterbreak>\p{Ll}</afterbreak></rule>
<rule break="yes"><beforebreak>[\.!?…][»"]?[\s ]+</beforebreak><afterbreak></afterbreak></rule>
<rule break="yes"><beforebreak></beforebreak><afterbreak>&lt;0\}</afterbreak></rule>
</languagerule>
<languagerule languagerulename="Other">
<rule break="yes"><beforebreak>,</beforebreak><afterbreak></afterbreak></rule>
</languagerule>
</languagerules>
<maprules>
<languagemap languagepattern=".*" languagerulename="Important"></languagemap>
<languagemap languagepattern="[a-z]{2,3}_two" languagerulename="TwoBreaks"></languagemap>
<languagemap languagepattern="(FR|fr).*" languagerulename="French"></languagemap>
<languagemap languagepattern="(XX|xx).*" languagerulename="Other"></languagemap>
</maprules>
</body>
</srx>"#;

    fn split(text: &str) -> Vec<&str> {
        let t = SrxSentenceTokenizer::new(SRX, "fr").unwrap();
        let s = t.tokenize(text);
        assert_eq!(s.concat(), text, "the sentences cover the text");
        s
    }

    #[test]
    fn loads_rules_and_language_maps() {
        let doc = SrxDocument::parse(SRX).unwrap();
        assert!(doc.cascade);
        let lists = doc.language_rule_list("fr_two");
        assert_eq!(lists.iter().map(|(n, _)| *n).collect::<Vec<_>>(), ["Important", "TwoBreaks", "French"]);
        let french = lists[2].1;
        assert_eq!(french.len(), 6);
        assert!(!french[0].breaking && french[4].breaking);
        assert_eq!(french[5], SrxRule { breaking: true, before: String::new(), after: "<0\\}".into() });
        assert_eq!(doc.language_rule_list("fr_one").len(), 2);
        assert_eq!(doc.language_rule_list("xx").len(), 2);
    }

    #[test]
    fn finitize_bounds_the_quantifiers() {
        assert_eq!(finitize("a*b+c{2,}", 100), "a{0,100}b{1,100}c{2,100}");
        assert_eq!(finitize("a\\*b\\\\*", 100), "a\\*b\\\\{0,100}");
        assert_eq!(finitize("a*?b++c{1,3}+", 9), "a{0,9}?b{1,9}+c{1,3}+");
        assert_eq!(finitize("\\Qa.b\\E+", 5), "\\a\\.\\b{1,5}");
    }

    #[test]
    fn breaks_after_sentence_punctuation() {
        assert_eq!(split("Il dort. Elle mange ! Vrai ?"), ["Il dort. ", "Elle mange ! ", "Vrai ?"]);
        assert_eq!(split("Il dort.Elle mange."), ["Il dort.Elle mange."]);
        assert_eq!(split("Fin. "), ["Fin. "]);
        assert_eq!(split(""), Vec::<&str>::new());
        assert_eq!(split("É. È."), ["É. ", "È."], "positions are chars, ranges are bytes");
        assert_eq!(split("Oui.\u{202f}Non."), ["Oui.\u{202f}", "Non."], "\\s is Unicode white space");
    }

    #[test]
    fn no_break_rules_hold_the_break_back() {
        assert_eq!(split("Voir M. Dupont. Fin."), ["Voir M. Dupont. ", "Fin."]);
        assert_eq!(split("Des pommes, etc. Puis rien."), ["Des pommes, etc. Puis rien."]);
        assert_eq!(split("Il dit (cf. Hugo) que. Oui."), ["Il dit (cf. Hugo) que. ", "Oui."]);
        assert_eq!(split("Quoi ? dit-il. Rien."), ["Quoi ? dit-il. ", "Rien."]);
        // a lower-case letter earlier on the line, then "p" or "al"
        assert_eq!(split("Il plut. plus tard. Fin."), ["Il plut. plus tard. ", "Fin."]);
        assert_eq!(split("IL PLUT. plus tard."), ["IL PLUT. ", "plus tard."]);
    }

    #[test]
    fn exceptions_look_at_the_right_side_of_the_position() {
        // The parenthesis rule needs a "(" before the position and a ")" after it.
        assert_eq!(split("Un. Deux (cf. Hugo). Trois. Fin"), ["Un. ", "Deux (cf. Hugo). ", "Trois. ", "Fin"]);
        assert_eq!(split("(a. b) Un. Deux"), ["(a. b) Un. ", "Deux"]);
        assert_eq!(split("Un. Deux (a) trois. Fin"), ["Un. ", "Deux (a) trois. ", "Fin"]);
        assert_eq!(split("Un (a. b. c) deux. Fin (x"), ["Un (a. b. c) deux. ", "Fin (x"]);
    }

    #[test]
    fn the_first_break_position_wins() {
        // The paragraph rule ends before the trailing space the sentence rule takes.
        assert_eq!(split("Il boit. \n \n Elle"), ["Il boit. \n \n", " Elle"]);
        assert_eq!(split("Un\n\nDeux\nTrois"), ["Un\n\n", "Deux\nTrois"]);
        assert_eq!(split("a<0}b"), ["a", "<0}b"]);
    }

    #[test]
    fn single_line_breaks_mode_uses_the_other_code() {
        let t = SrxSentenceTokenizer::new(SRX, "fr").unwrap();
        // "fr_one" has no paragraph rule in this file: only the sentence rules apply.
        assert_eq!(t.ranges("Un\n\nDeux. Trois", true), [(0, 10), (10, 15)]);
        assert_eq!(t.ranges("Un\n\nDeux. Trois", false), [(0, 4), (4, 10), (10, 15)]);
    }
}
