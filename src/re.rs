//! Regular expressions with `java.util.regex` semantics.
//!
//! LanguageTool's rule files are written against Java's regex engine: tokens
//! and part-of-speech tags are tested with `Matcher.matches()` (whole string)
//! or `find()`, suggestions are rewritten with `replaceAll`, and the patterns
//! rely on look-around, back-references, lazy and possessive quantifiers and
//! inline flags. This module implements that dialect with a backtracking
//! matcher over `char`s, so that a rule behaves here as it does in
//! LanguageTool. Inputs are short (a token, a tag, a sentence), which is the
//! regime a backtracking engine is good at.
//!
//! Supported: literals and escapes (`\t`, `\n`, `\xhh`, `\uhhhh`, `\Q...\E`),
//! `.`, character classes with ranges, negation, nesting and `&&`
//! intersection, `\d \w \s \h \v` and their negations, `\p{..}`/`\P{..}`
//! (general categories, `Is..`/`java..` names and POSIX names), anchors
//! (`^ $ \A \z \Z \b \B`), groups (capturing, named, `(?:..)`, atomic
//! `(?>..)`), alternation, greedy/lazy/possessive quantifiers, back-references
//! (`\1`, `\k<name>`), look-ahead and look-behind, and the flags `i u s m x`
//! (inline `(?i)` and scoped `(?i:..)`).
//!
//! As in Java, `(?i)` alone folds ASCII letters only; Unicode folding needs
//! the `u` flag ([`Flags::unicode_case`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Compile-time flags (`Pattern.CASE_INSENSITIVE`, ...).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Flags {
    pub case_insensitive: bool,
    pub unicode_case: bool,
    pub dotall: bool,
    pub multiline: bool,
    pub comments: bool,
    /// `UNICODE_CHARACTER_CLASS`: `\w`, `\d`, `\s` and the POSIX classes
    /// follow Unicode instead of ASCII.
    pub unicode_class: bool,
}

impl Flags {
    /// `CASE_INSENSITIVE | UNICODE_CASE`, LanguageTool's default for tokens.
    pub const IGNORE_CASE: Flags = Flags {
        case_insensitive: true,
        unicode_case: true,
        dotall: false,
        multiline: false,
        comments: false,
        unicode_class: false,
    };

    /// `IGNORE_CASE` plus `UNICODE_CHARACTER_CLASS` (LanguageTool's `<regexp>`).
    pub const IGNORE_CASE_UNICODE_CLASS: Flags = Flags {
        case_insensitive: true,
        unicode_case: true,
        dotall: false,
        multiline: false,
        comments: false,
        unicode_class: true,
    };

    pub const UNICODE_CLASS: Flags = Flags {
        case_insensitive: false,
        unicode_case: false,
        dotall: false,
        multiline: false,
        comments: false,
        unicode_class: true,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Greed {
    Greedy,
    Lazy,
    Possessive,
}

#[derive(Clone, Debug)]
enum ClassItem {
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
    HSpace(bool),
    VSpace(bool),
    Prop(Prop, bool),
    /// Nested class (union member), already carrying its own negation.
    Set(Box<ClassSet>),
}

#[derive(Clone, Debug, Default)]
struct ClassSet {
    neg: bool,
    items: Vec<ClassItem>,
    /// `&&` operands: the char must also belong to each of them.
    and: Vec<ClassSet>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prop {
    Letter,
    Upper,
    Lower,
    TitleOrOtherLetter,
    Mark,
    Number,
    DecimalDigit,
    Punct,
    Symbol,
    Separator,
    SpaceSeparator,
    Control,
    Alnum,
    AsciiPunct,
    Graph,
    Print,
    Blank,
    XDigit,
    White,
    Latin,
    Greek,
    Cyrillic,
    /// `\w` under `UNICODE_CHARACTER_CLASS`: letters, marks, digits,
    /// connector punctuation.
    UnicodeWord,
    Any,
    None,
}

#[derive(Clone, Debug)]
enum Node {
    Empty,
    /// A literal char; `ci`: 0 exact, 1 ASCII folding, 2 Unicode folding.
    Char(char, u8),
    Any(bool),
    Class(Box<ClassSet>, u8),
    LineStart(bool),
    LineEnd(bool),
    InputStart,
    InputEnd,
    InputEndBeforeNl,
    WordB(bool),
    Group(Box<Node>, Option<usize>),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat(Box<Node>, u32, u32, Greed),
    Look { node: Box<Node>, ahead: bool, neg: bool },
    Backref(usize, u8),
    Atomic(Box<Node>),
}

/// A compiled regular expression.
#[derive(Debug)]
pub struct Regex {
    node: Node,
    groups: usize,
    names: Vec<(String, usize)>,
    /// When the whole pattern is an alternation of literals: the strings (as
    /// written, or case-folded when `fold` is set), for a hash lookup instead
    /// of a match.
    literals: Option<std::collections::HashSet<String>>,
    fold: u8,
    inline_flags: bool,
    src: String,
}

/// Capture positions of a match, as char indices into the input.
#[derive(Clone, Debug)]
pub struct Captures {
    pub spans: Vec<Option<(usize, usize)>>,
}

#[derive(Debug)]
pub struct Error(pub String);

struct Parser<'a> {
    c: Vec<char>,
    i: usize,
    flags: Flags,
    groups: usize,
    names: Vec<(String, usize)>,
    /// The pattern changes flags inline (`(?i)`, `(?-i:...)`).
    inline_flags: bool,
    _src: &'a str,
}

fn ci_of(f: Flags) -> u8 {
    if !f.case_insensitive {
        0
    } else if f.unicode_case {
        2
    } else {
        1
    }
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }

    fn eat(&mut self, ch: char) -> bool {
        if self.peek() == Some(ch) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn looking_at(&self, s: &str) -> bool {
        let mut j = self.i;
        for ch in s.chars() {
            if self.c.get(j) != Some(&ch) {
                return false;
            }
            j += 1;
        }
        true
    }

    fn err<T>(&self, msg: &str) -> Result<T, Error> {
        Err(Error(format!("{msg} at {} in /{}/", self.i, self._src)))
    }

    fn alt(&mut self) -> Result<Node, Error> {
        let mut alts = vec![self.concat()?];
        while self.eat('|') {
            alts.push(self.concat()?);
        }
        Ok(if alts.len() == 1 { alts.pop().unwrap() } else { Node::Alt(alts) })
    }

    fn skip_comments(&mut self) {
        if !self.flags.comments {
            return;
        }
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => self.i += 1,
                Some('#') => {
                    while let Some(c) = self.peek() {
                        self.i += 1;
                        if c == '\n' {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }

    fn concat(&mut self) -> Result<Node, Error> {
        let mut items = Vec::new();
        loop {
            self.skip_comments();
            match self.peek() {
                None | Some('|') | Some(')') => break,
                _ => {}
            }
            let atoms = self.atom()?;
            for atom in atoms {
                let atom = self.quantifier(atom)?;
                items.push(atom);
            }
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().unwrap(),
            _ => Node::Concat(items),
        })
    }

    fn quantifier(&mut self, atom: Node) -> Result<Node, Error> {
        self.skip_comments();
        let (min, max) = match self.peek() {
            Some('?') => {
                self.i += 1;
                (0, 1)
            }
            Some('*') => {
                self.i += 1;
                (0, u32::MAX)
            }
            Some('+') => {
                self.i += 1;
                (1, u32::MAX)
            }
            Some('{') => {
                // `{n}`, `{n,}`, `{n,m}`; anything else is a literal brace.
                let save = self.i;
                self.i += 1;
                let mut a = String::new();
                while let Some(c) = self.peek().filter(char::is_ascii_digit) {
                    a.push(c);
                    self.i += 1;
                }
                let mut b = a.clone();
                let mut open = false;
                if self.eat(',') {
                    b.clear();
                    while let Some(c) = self.peek().filter(char::is_ascii_digit) {
                        b.push(c);
                        self.i += 1;
                    }
                    open = b.is_empty();
                }
                if a.is_empty() || !self.eat('}') {
                    self.i = save;
                    return Ok(atom);
                }
                let min: u32 = a.parse().unwrap_or(0);
                let max: u32 = if open { u32::MAX } else { b.parse().unwrap_or(min) };
                (min, max)
            }
            _ => return Ok(atom),
        };
        let greed = if self.eat('?') {
            Greed::Lazy
        } else if self.eat('+') {
            Greed::Possessive
        } else {
            Greed::Greedy
        };
        Ok(Node::Repeat(Box::new(atom), min, max, greed))
    }

    /// One atom. A `\Q..\E` run yields several literal nodes, and only the
    /// last one takes a following quantifier.
    fn atom(&mut self) -> Result<Vec<Node>, Error> {
        let ci = ci_of(self.flags);
        let c = match self.peek() {
            Some(c) => c,
            None => return Ok(vec![Node::Empty]),
        };
        self.i += 1;
        let node = match c {
            '.' => Node::Any(self.flags.dotall),
            '^' => Node::LineStart(self.flags.multiline),
            '$' => Node::LineEnd(self.flags.multiline),
            '[' => Node::Class(Box::new(self.class()?), ci),
            '(' => return self.group().map(|n| vec![n]),
            '\\' => return self.escape(),
            _ => Node::Char(c, ci),
        };
        Ok(vec![node])
    }

    fn group(&mut self) -> Result<Node, Error> {
        if !self.eat('?') {
            self.groups += 1;
            let idx = self.groups;
            let saved = self.flags;
            let inner = self.alt()?;
            self.flags = saved;
            if !self.eat(')') {
                return self.err("missing )");
            }
            return Ok(Node::Group(Box::new(inner), Some(idx)));
        }
        // (?:..) (?=..) (?!..) (?<=..) (?<!..) (?>..) (?<name>..) (?flags) (?flags:..)
        let saved = self.flags;
        let node = if self.eat(':') {
            let inner = self.alt()?;
            Node::Group(Box::new(inner), None)
        } else if self.eat('=') {
            Node::Look { node: Box::new(self.alt()?), ahead: true, neg: false }
        } else if self.eat('!') {
            Node::Look { node: Box::new(self.alt()?), ahead: true, neg: true }
        } else if self.looking_at("<=") {
            self.i += 2;
            Node::Look { node: Box::new(self.alt()?), ahead: false, neg: false }
        } else if self.looking_at("<!") {
            self.i += 2;
            Node::Look { node: Box::new(self.alt()?), ahead: false, neg: true }
        } else if self.eat('>') {
            Node::Atomic(Box::new(self.alt()?))
        } else if self.eat('<') {
            let mut name = String::new();
            while let Some(c) = self.peek() {
                self.i += 1;
                if c == '>' {
                    break;
                }
                name.push(c);
            }
            self.groups += 1;
            let idx = self.groups;
            self.names.push((name, idx));
            let inner = self.alt()?;
            Node::Group(Box::new(inner), Some(idx))
        } else {
            // flags
            self.inline_flags = true;
            let mut on = true;
            let mut f = self.flags;
            loop {
                match self.peek() {
                    Some('-') => on = false,
                    Some('i') => f.case_insensitive = on,
                    Some('u') => f.unicode_case = on,
                    Some('s') => f.dotall = on,
                    Some('m') => f.multiline = on,
                    Some('x') => f.comments = on,
                    Some('d') | Some('U') => {}
                    Some(')') => {
                        // `(?i)`: applies to the rest of the enclosing group.
                        self.i += 1;
                        self.flags = f;
                        return Ok(Node::Empty);
                    }
                    Some(':') => {
                        self.i += 1;
                        self.flags = f;
                        let inner = self.alt()?;
                        self.flags = saved;
                        if !self.eat(')') {
                            return self.err("missing )");
                        }
                        return Ok(Node::Group(Box::new(inner), None));
                    }
                    _ => return self.err("unknown group flag"),
                }
                self.i += 1;
            }
        };
        self.flags = saved;
        if !self.eat(')') {
            return self.err("missing )");
        }
        Ok(node)
    }

    fn hex(&mut self, n: usize) -> Result<char, Error> {
        let mut v = 0u32;
        for _ in 0..n {
            match self.peek().and_then(|c| c.to_digit(16)) {
                Some(d) => {
                    v = v * 16 + d;
                    self.i += 1;
                }
                None => return self.err("bad hex escape"),
            }
        }
        char::from_u32(v).ok_or_else(|| Error("bad code point".into()))
    }

    /// Escapes shared by the top level and character classes. Returns `None`
    /// when the escape is not a single char or shorthand class.
    fn simple_escape(&mut self, c: char) -> Result<Option<ClassItem>, Error> {
        let one = |ch: char| Ok(Some(ClassItem::Range(ch, ch)));
        match c {
            'd' if self.flags.unicode_class => Ok(Some(ClassItem::Prop(Prop::DecimalDigit, false))),
            'D' if self.flags.unicode_class => Ok(Some(ClassItem::Prop(Prop::DecimalDigit, true))),
            'w' if self.flags.unicode_class => Ok(Some(ClassItem::Prop(Prop::UnicodeWord, false))),
            'W' if self.flags.unicode_class => Ok(Some(ClassItem::Prop(Prop::UnicodeWord, true))),
            's' if self.flags.unicode_class => Ok(Some(ClassItem::Prop(Prop::White, false))),
            'S' if self.flags.unicode_class => Ok(Some(ClassItem::Prop(Prop::White, true))),
            'd' => Ok(Some(ClassItem::Digit(false))),
            'D' => Ok(Some(ClassItem::Digit(true))),
            'w' => Ok(Some(ClassItem::Word(false))),
            'W' => Ok(Some(ClassItem::Word(true))),
            's' => Ok(Some(ClassItem::Space(false))),
            'S' => Ok(Some(ClassItem::Space(true))),
            'h' => Ok(Some(ClassItem::HSpace(false))),
            'H' => Ok(Some(ClassItem::HSpace(true))),
            'v' => Ok(Some(ClassItem::VSpace(false))),
            'V' => Ok(Some(ClassItem::VSpace(true))),
            't' => one('\t'),
            'n' => one('\n'),
            'r' => one('\r'),
            'f' => one('\u{c}'),
            'a' => one('\u{7}'),
            'e' => one('\u{1b}'),
            '0' => {
                let mut v = 0u32;
                let mut n = 0;
                while n < 3 {
                    match self.peek().and_then(|c| c.to_digit(8)) {
                        Some(d) if v * 8 + d <= 0o377 => {
                            v = v * 8 + d;
                            self.i += 1;
                            n += 1;
                        }
                        _ => break,
                    }
                }
                one(char::from_u32(v).unwrap_or('\0'))
            }
            'x' => {
                if self.eat('{') {
                    let mut v = 0u32;
                    while let Some(d) = self.peek().and_then(|c| c.to_digit(16)) {
                        v = v * 16 + d;
                        self.i += 1;
                    }
                    if !self.eat('}') {
                        return self.err("bad \\x{..}");
                    }
                    one(char::from_u32(v).unwrap_or('\0'))
                } else {
                    let ch = self.hex(2)?;
                    one(ch)
                }
            }
            'u' => {
                let ch = self.hex(4)?;
                one(ch)
            }
            'p' | 'P' => {
                let neg = c == 'P';
                let name = if self.eat('{') {
                    let mut s = String::new();
                    while let Some(ch) = self.peek() {
                        self.i += 1;
                        if ch == '}' {
                            break;
                        }
                        s.push(ch);
                    }
                    s
                } else {
                    let ch = self.peek().unwrap_or('L');
                    self.i += 1;
                    ch.to_string()
                };
                let (name, neg) = match name.strip_prefix('^') {
                    Some(n) => (n.to_string(), !neg),
                    None => (name, neg),
                };
                Ok(Some(ClassItem::Prop(prop_by_name(&name), neg)))
            }
            _ => Ok(None),
        }
    }

    fn escape(&mut self) -> Result<Vec<Node>, Error> {
        let ci = ci_of(self.flags);
        let c = match self.peek() {
            Some(c) => c,
            None => return self.err("trailing backslash"),
        };
        self.i += 1;
        let node = match c {
            'b' => Node::WordB(false),
            'B' => Node::WordB(true),
            'A' => Node::InputStart,
            'z' => Node::InputEnd,
            'Z' => Node::InputEndBeforeNl,
            'G' => Node::Empty,
            'Q' => {
                let mut out = Vec::new();
                while self.i < self.c.len() && !self.looking_at("\\E") {
                    out.push(Node::Char(self.c[self.i], ci));
                    self.i += 1;
                }
                if self.looking_at("\\E") {
                    self.i += 2;
                }
                if out.is_empty() {
                    out.push(Node::Empty);
                }
                return Ok(out);
            }
            'E' => Node::Empty,
            'k' => {
                if !self.eat('<') {
                    return self.err("bad \\k");
                }
                let mut name = String::new();
                while let Some(ch) = self.peek() {
                    self.i += 1;
                    if ch == '>' {
                        break;
                    }
                    name.push(ch);
                }
                match self.names.iter().find(|(n, _)| *n == name) {
                    Some((_, idx)) => Node::Backref(*idx, ci),
                    None => return self.err("unknown group name"),
                }
            }
            '1'..='9' => {
                // Java takes further digits while the number stays a valid group.
                let mut n = c.to_digit(10).unwrap() as usize;
                while let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
                    let m = n * 10 + d as usize;
                    if m > self.groups {
                        break;
                    }
                    n = m;
                    self.i += 1;
                }
                Node::Backref(n, ci)
            }
            _ => match self.simple_escape(c)? {
                Some(ClassItem::Range(a, b)) if a == b => Node::Char(a, ci),
                Some(item) => Node::Class(Box::new(ClassSet { neg: false, items: vec![item], and: Vec::new() }), ci),
                None => Node::Char(c, ci),
            },
        };
        Ok(vec![node])
    }

    /// Parse a class body after `[`, up to and including the closing `]`.
    fn class(&mut self) -> Result<ClassSet, Error> {
        let mut set = ClassSet::default();
        if self.eat('^') {
            set.neg = true;
        }
        let mut first = true;
        loop {
            let c = match self.peek() {
                Some(c) => c,
                None => return self.err("missing ]"),
            };
            if c == ']' && !first {
                self.i += 1;
                break;
            }
            first = false;
            if self.looking_at("&&") {
                self.i += 2;
                // Right operand: either a bracketed class or the rest of this one.
                let rhs = if self.eat('[') {
                    let s = self.class()?;
                    // Further items after the nested class still belong to the
                    // intersection operand; collect them.
                    let mut rest = self.class_rest()?;
                    if rest.items.is_empty() && rest.and.is_empty() {
                        s
                    } else {
                        rest.items.push(ClassItem::Set(Box::new(s)));
                        rest
                    }
                } else {
                    self.class_rest()?
                };
                set.and.push(rhs);
                break;
            }
            self.i += 1;
            let lo = match c {
                '[' => {
                    let nested = self.class()?;
                    set.items.push(ClassItem::Set(Box::new(nested)));
                    continue;
                }
                '\\' => {
                    let e = match self.peek() {
                        Some(e) => e,
                        None => return self.err("trailing backslash"),
                    };
                    self.i += 1;
                    if e == 'Q' {
                        while self.i < self.c.len() && !self.looking_at("\\E") {
                            let ch = self.c[self.i];
                            set.items.push(ClassItem::Range(ch, ch));
                            self.i += 1;
                        }
                        if self.looking_at("\\E") {
                            self.i += 2;
                        }
                        continue;
                    }
                    match self.simple_escape(e)? {
                        Some(ClassItem::Range(a, _)) => a,
                        Some(item) => {
                            set.items.push(item);
                            continue;
                        }
                        None => e,
                    }
                }
                _ => c,
            };
            // range?
            if self.peek() == Some('-') && self.c.get(self.i + 1).is_some_and(|&n| n != ']' && n != '[') {
                self.i += 1;
                let mut hi = self.c[self.i];
                self.i += 1;
                if hi == '\\' {
                    let e = self.peek().unwrap_or('\\');
                    self.i += 1;
                    hi = match self.simple_escape(e)? {
                        Some(ClassItem::Range(a, _)) => a,
                        _ => e,
                    };
                }
                set.items.push(ClassItem::Range(lo, hi));
            } else {
                set.items.push(ClassItem::Range(lo, lo));
            }
        }
        Ok(set)
    }

    /// The remainder of a class body (used for the right side of `&&`).
    fn class_rest(&mut self) -> Result<ClassSet, Error> {
        // Re-enter the class parser without the opening bracket; an immediate
        // `]` closes an empty remainder.
        if self.eat(']') {
            return Ok(ClassSet::default());
        }
        let mut set = self.class_body_no_first_rule()?;
        set.neg = false;
        Ok(set)
    }

    fn class_body_no_first_rule(&mut self) -> Result<ClassSet, Error> {
        // `class()` treats a leading `]` as a literal; here the remainder is
        // never empty (checked by the caller), so the same code applies.
        self.class()
    }
}

fn prop_by_name(name: &str) -> Prop {
    let n = name.strip_prefix("Is").or_else(|| name.strip_prefix("In")).unwrap_or(name);
    let n = n.strip_prefix("gc=").or_else(|| n.strip_prefix("general_category=")).unwrap_or(n);
    match n {
        "L" | "Letter" | "Alphabetic" | "Alpha" | "javaLetter" | "javaAlphabetic" | "IsAlphabetic" => Prop::Letter,
        "Lu" | "Upper" | "Uppercase" | "javaUpperCase" | "UppercaseLetter" => Prop::Upper,
        "Ll" | "Lower" | "Lowercase" | "javaLowerCase" | "LowercaseLetter" => Prop::Lower,
        "Lt" | "Lm" | "Lo" => Prop::TitleOrOtherLetter,
        "M" | "Mn" | "Mc" | "Me" | "Mark" => Prop::Mark,
        "N" | "Nl" | "No" | "Number" => Prop::Number,
        "Nd" | "Digit" | "javaDigit" => Prop::DecimalDigit,
        "P" | "Pc" | "Pd" | "Ps" | "Pe" | "Pi" | "Pf" | "Po" | "Punctuation" => Prop::Punct,
        "Punct" => Prop::AsciiPunct,
        "S" | "Sm" | "Sc" | "Sk" | "So" | "Symbol" => Prop::Symbol,
        "Z" | "Zl" | "Zp" | "Separator" => Prop::Separator,
        "Zs" => Prop::SpaceSeparator,
        "C" | "Cc" | "Cf" | "Cntrl" | "Control" => Prop::Control,
        "Alnum" | "javaLetterOrDigit" => Prop::Alnum,
        "Graph" => Prop::Graph,
        "Print" => Prop::Print,
        "Blank" => Prop::Blank,
        "XDigit" => Prop::XDigit,
        "Space" | "White_Space" | "WhiteSpace" | "javaWhitespace" | "javaSpaceChar" => Prop::White,
        "Latin" => Prop::Latin,
        "Greek" => Prop::Greek,
        "Cyrillic" => Prop::Cyrillic,
        "Any" | "all" => Prop::Any,
        _ => Prop::None,
    }
}

/// General category P* for the characters that occur in French text.
fn is_punct(c: char) -> bool {
    if c.is_ascii() {
        return matches!(c, '!' | '"' | '#' | '%' | '&' | '\'' | '(' | ')' | '*' | ',' | '-' | '.' | '/' | ':' | ';' | '?' | '@' | '[' | '\\' | ']' | '_' | '{' | '}');
    }
    matches!(c,
        '\u{a1}' | '\u{a7}' | '\u{ab}' | '\u{b6}' | '\u{b7}' | '\u{bb}' | '\u{bf}'
        | '\u{2010}'..='\u{2027}' | '\u{2030}'..='\u{2043}' | '\u{2045}'..='\u{2051}'
        | '\u{2053}'..='\u{205e}' | '\u{207d}' | '\u{207e}' | '\u{208d}' | '\u{208e}'
        | '\u{2308}'..='\u{230b}' | '\u{2329}' | '\u{232a}' | '\u{2768}'..='\u{2775}'
        | '\u{27e6}'..='\u{27ef}' | '\u{2983}'..='\u{2998}' | '\u{2e00}'..='\u{2e2e}'
        | '\u{3001}'..='\u{3003}' | '\u{3008}'..='\u{3011}' | '\u{fe50}'..='\u{fe52}'
        | '\u{ff01}'..='\u{ff03}' | '\u{ff05}'..='\u{ff0a}' | '\u{ff0c}'..='\u{ff0f}'
        | '\u{ff1a}' | '\u{ff1b}' | '\u{ff1f}' | '\u{ff20}' | '\u{37e}' | '\u{387}'
        | '\u{55a}'..='\u{55f}' | '\u{589}' | '\u{58a}' | '\u{5be}' | '\u{5c0}' | '\u{5c3}'
        | '\u{60c}' | '\u{61b}' | '\u{61f}' | '\u{66a}'..='\u{66d}')
}

/// General category S* (math, currency, modifier, other symbols).
fn is_symbol(c: char) -> bool {
    if c.is_ascii() {
        return matches!(c, '$' | '+' | '<' | '=' | '>' | '^' | '`' | '|' | '~');
    }
    matches!(c,
        '\u{a2}'..='\u{a6}' | '\u{a8}' | '\u{a9}' | '\u{ac}' | '\u{ae}'..='\u{b1}' | '\u{b4}' | '\u{b8}'
        | '\u{d7}' | '\u{f7}' | '\u{2c2}'..='\u{2c5}' | '\u{2d2}'..='\u{2df}' | '\u{2044}' | '\u{2052}'
        | '\u{207a}'..='\u{207c}' | '\u{208a}'..='\u{208c}' | '\u{20a0}'..='\u{20bf}'
        | '\u{2100}'..='\u{214f}' | '\u{2190}'..='\u{2307}' | '\u{230c}'..='\u{2328}'
        | '\u{232b}'..='\u{23ff}' | '\u{2500}'..='\u{2767}' | '\u{2794}'..='\u{27bf}'
        | '\u{2b00}'..='\u{2bff}' | '\u{1f000}'..='\u{1faff}' | '\u{fe62}'..='\u{fe66}')
}

fn is_mark(c: char) -> bool {
    matches!(c, '\u{300}'..='\u{36f}' | '\u{483}'..='\u{489}' | '\u{1ab0}'..='\u{1aff}' | '\u{1dc0}'..='\u{1dff}' | '\u{20d0}'..='\u{20ff}' | '\u{fe00}'..='\u{fe0f}' | '\u{fe20}'..='\u{fe2f}')
}

fn prop_matches(p: Prop, c: char) -> bool {
    match p {
        Prop::Letter => c.is_alphabetic(),
        Prop::Upper => c.is_uppercase(),
        Prop::Lower => c.is_lowercase(),
        Prop::TitleOrOtherLetter => c.is_alphabetic() && !c.is_uppercase() && !c.is_lowercase(),
        Prop::Mark => is_mark(c),
        Prop::Number => c.is_numeric(),
        Prop::DecimalDigit => c.is_ascii_digit() || (c.is_numeric() && c.to_digit(10).is_some()),
        Prop::Punct => is_punct(c),
        Prop::AsciiPunct => c.is_ascii_punctuation(),
        Prop::Symbol => is_symbol(c),
        Prop::Separator => c == ' ' || c == '\u{a0}' || c == '\u{1680}' || ('\u{2000}'..='\u{200a}').contains(&c) || c == '\u{2028}' || c == '\u{2029}' || c == '\u{202f}' || c == '\u{205f}' || c == '\u{3000}',
        Prop::SpaceSeparator => c == ' ' || c == '\u{a0}' || c == '\u{1680}' || ('\u{2000}'..='\u{200a}').contains(&c) || c == '\u{202f}' || c == '\u{205f}' || c == '\u{3000}',
        Prop::Control => c.is_control() || matches!(c, '\u{ad}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{feff}'),
        Prop::Alnum => c.is_alphanumeric(),
        Prop::Graph => c.is_ascii_graphic(),
        Prop::Print => c.is_ascii_graphic() || c == ' ',
        Prop::Blank => c == ' ' || c == '\t',
        Prop::XDigit => c.is_ascii_hexdigit(),
        Prop::White => c.is_whitespace(),
        Prop::Latin => c.is_alphabetic() && (c as u32) < 0x250 || ('\u{1e00}'..='\u{1eff}').contains(&c),
        Prop::Greek => ('\u{370}'..='\u{3ff}').contains(&c) || ('\u{1f00}'..='\u{1fff}').contains(&c),
        Prop::Cyrillic => ('\u{400}'..='\u{52f}').contains(&c),
        Prop::UnicodeWord => c.is_alphanumeric() || c == '_' || is_mark(c) || c == '\u{200c}' || c == '\u{200d}' || c == '\u{203f}' || c == '\u{2040}',

        Prop::Any => true,
        Prop::None => false,
    }
}

fn is_java_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r')
}

fn is_hspace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\u{a0}' | '\u{1680}' | '\u{180e}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}

fn is_vspace(c: char) -> bool {
    matches!(c, '\n' | '\u{b}' | '\u{c}' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

fn is_line_term(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// Java's `Character.isLetterOrDigit`, which `\b` is built on.
fn is_word_boundary_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || is_mark(c)
}

/// Single-char case folding as Java does it: compare upper cases, then lower
/// cases of those (`String.regionMatches(ignoreCase)`).
#[inline]
pub fn fold(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    let mut up = c.to_uppercase();
    let u = match (up.next(), up.next()) {
        (Some(u), None) => u,
        _ => c,
    };
    let mut lo = u.to_lowercase();
    match (lo.next(), lo.next()) {
        (Some(l), None) => l,
        _ => u,
    }
}

#[inline]
fn chars_eq(a: char, b: char, ci: u8) -> bool {
    if a == b {
        return true;
    }
    match ci {
        0 => false,
        1 => a.is_ascii() && b.is_ascii() && a.eq_ignore_ascii_case(&b),
        _ => fold(a) == fold(b),
    }
}

fn item_matches(item: &ClassItem, c: char) -> bool {
    match item {
        ClassItem::Range(a, b) => *a <= c && c <= *b,
        ClassItem::Digit(neg) => c.is_ascii_digit() != *neg,
        ClassItem::Word(neg) => (c.is_ascii_alphanumeric() || c == '_') != *neg,
        ClassItem::Space(neg) => is_java_space(c) != *neg,
        ClassItem::HSpace(neg) => is_hspace(c) != *neg,
        ClassItem::VSpace(neg) => is_vspace(c) != *neg,
        ClassItem::Prop(p, neg) => prop_matches(*p, c) != *neg,
        ClassItem::Set(s) => set_matches_exact(s, c),
    }
}

fn set_matches_exact(set: &ClassSet, c: char) -> bool {
    let mut hit = set.items.iter().any(|it| item_matches(it, c));
    if hit != set.neg {
        hit = set.and.iter().all(|s| set_matches_exact(s, c));
        hit
    } else {
        false
    }
}

/// Does `f` hold for `c` or, when matching case-insensitively, for one of its
/// case variants?
fn any_variant(c: char, ci: u8, f: impl Fn(char) -> bool) -> bool {
    if f(c) {
        return true;
    }
    match ci {
        0 => false,
        1 => c.is_ascii_alphabetic() && (f(c.to_ascii_lowercase()) || f(c.to_ascii_uppercase())),
        _ => {
            let folded = fold(c);
            if folded != c && f(folded) {
                return true;
            }
            let mut up = c.to_uppercase();
            match (up.next(), up.next()) {
                (Some(u), None) if u != c => f(u),
                _ => false,
            }
        }
    }
}

/// As in Java, case-insensitivity applies to the positive members of a class
/// and the negation comes last: `(?i)[^a-z]` does not match `F`.
fn set_matches(set: &ClassSet, c: char, ci: u8) -> bool {
    let hit = set.items.iter().any(|item| match item {
        ClassItem::Set(inner) => set_matches(inner, c, ci),
        ClassItem::Range(a, b) => any_variant(c, ci, |x| *a <= x && x <= *b),
        ClassItem::Prop(p, neg) => any_variant(c, ci, |x| prop_matches(*p, x)) != *neg,
        other => item_matches(other, c),
    });
    if hit != set.neg {
        set.and.iter().all(|s| set_matches(s, c, ci))
    } else {
        false
    }
}

struct Matcher<'a> {
    input: &'a [char],
    caps: Vec<Option<(usize, usize)>>,
    /// Backtracking budget: a pathological pattern gives up (no match)
    /// instead of running for ever.
    steps: u32,
}

const STEP_LIMIT: u32 = 2_000_000;

impl<'a> Matcher<'a> {
    fn run(&mut self, node: &Node, pos: usize, k: &mut dyn FnMut(&mut Matcher<'a>, usize) -> bool) -> bool {
        self.steps += 1;
        if self.steps > STEP_LIMIT {
            return false;
        }
        match node {
            Node::Empty => k(self, pos),
            Node::Char(c, ci) => match self.input.get(pos) {
                Some(&x) if chars_eq(x, *c, *ci) => k(self, pos + 1),
                _ => false,
            },
            Node::Any(dotall) => match self.input.get(pos) {
                Some(&x) if *dotall || !is_line_term(x) => k(self, pos + 1),
                _ => false,
            },
            Node::Class(set, ci) => match self.input.get(pos) {
                Some(&x) if set_matches(set, x, *ci) => k(self, pos + 1),
                _ => false,
            },
            Node::LineStart(multi) => {
                if pos == 0 || (*multi && is_line_term(self.input[pos - 1]) && pos < self.input.len()) {
                    k(self, pos)
                } else {
                    false
                }
            }
            Node::LineEnd(multi) => {
                let n = self.input.len();
                let at_end = pos == n
                    || (pos + 1 == n && is_line_term(self.input[pos]))
                    || (pos + 2 == n && self.input[pos] == '\r' && self.input[pos + 1] == '\n');
                if at_end || (*multi && pos < n && is_line_term(self.input[pos])) {
                    k(self, pos)
                } else {
                    false
                }
            }
            Node::InputStart => pos == 0 && k(self, pos),
            Node::InputEnd => pos == self.input.len() && k(self, pos),
            Node::InputEndBeforeNl => {
                let n = self.input.len();
                (pos == n || (pos + 1 == n && is_line_term(self.input[pos]))) && k(self, pos)
            }
            Node::WordB(neg) => {
                let before = pos > 0 && is_word_boundary_char(self.input[pos - 1]);
                let after = pos < self.input.len() && is_word_boundary_char(self.input[pos]);
                if (before != after) != *neg {
                    k(self, pos)
                } else {
                    false
                }
            }
            Node::Group(inner, None) => self.run(inner, pos, k),
            Node::Group(inner, Some(idx)) => {
                let idx = *idx;
                let saved = self.caps[idx];
                let ok = self.run(inner, pos, &mut |m, end| {
                    let prev = m.caps[idx];
                    m.caps[idx] = Some((pos, end));
                    if k(m, end) {
                        true
                    } else {
                        m.caps[idx] = prev;
                        false
                    }
                });
                if !ok {
                    self.caps[idx] = saved;
                }
                ok
            }
            Node::Concat(items) => self.seq(items, pos, k),
            Node::Alt(alts) => {
                for a in alts {
                    if self.run(a, pos, k) {
                        return true;
                    }
                }
                false
            }
            Node::Repeat(inner, min, max, greed) => match greed {
                Greed::Greedy => self.greedy(inner, *min, *max, 0, pos, k),
                Greed::Lazy => self.lazy(inner, *min, *max, 0, pos, k),
                Greed::Possessive => {
                    let mut end = None;
                    let saved = self.caps.clone();
                    if self.greedy(inner, *min, *max, 0, pos, &mut |_, e| {
                        end = Some(e);
                        true
                    }) {
                        if k(self, end.unwrap()) {
                            return true;
                        }
                    }
                    self.caps = saved;
                    false
                }
            },
            Node::Atomic(inner) => {
                let mut end = None;
                let saved = self.caps.clone();
                if self.run(inner, pos, &mut |_, e| {
                    end = Some(e);
                    true
                }) && k(self, end.unwrap())
                {
                    return true;
                }
                self.caps = saved;
                false
            }
            Node::Look { node, ahead: true, neg } => {
                let saved = self.caps.clone();
                let hit = self.run(node, pos, &mut |_, _| true);
                if *neg {
                    self.caps = saved;
                    !hit && k(self, pos)
                } else if hit {
                    if k(self, pos) {
                        true
                    } else {
                        self.caps = saved;
                        false
                    }
                } else {
                    false
                }
            }
            Node::Look { node, ahead: false, neg } => {
                let saved = self.caps.clone();
                let mut hit = false;
                for start in (0..=pos).rev() {
                    if self.run(node, start, &mut |_, e| e == pos) {
                        hit = true;
                        break;
                    }
                }
                if *neg {
                    self.caps = saved;
                    !hit && k(self, pos)
                } else if hit {
                    if k(self, pos) {
                        true
                    } else {
                        self.caps = saved;
                        false
                    }
                } else {
                    false
                }
            }
            Node::Backref(idx, ci) => match self.caps.get(*idx).copied().flatten() {
                // Java: a back-reference to a group that did not participate fails.
                None => false,
                Some((a, b)) => {
                    let len = b - a;
                    if pos + len > self.input.len() {
                        return false;
                    }
                    for i in 0..len {
                        if !chars_eq(self.input[a + i], self.input[pos + i], *ci) {
                            return false;
                        }
                    }
                    k(self, pos + len)
                }
            },
        }
    }

    fn seq(&mut self, items: &[Node], pos: usize, k: &mut dyn FnMut(&mut Matcher<'a>, usize) -> bool) -> bool {
        match items.split_first() {
            None => k(self, pos),
            Some((first, rest)) => {
                if rest.is_empty() {
                    self.run(first, pos, k)
                } else {
                    self.run(first, pos, &mut |m, p| m.seq(rest, p, k))
                }
            }
        }
    }

    fn greedy(&mut self, inner: &Node, min: u32, max: u32, count: u32, pos: usize, k: &mut dyn FnMut(&mut Matcher<'a>, usize) -> bool) -> bool {
        if count < max {
            let ok = self.run(inner, pos, &mut |m, p| {
                // An empty iteration past the minimum cannot make progress.
                if p == pos && count >= min {
                    return false;
                }
                m.greedy(inner, min, max, count + 1, p, k)
            });
            if ok {
                return true;
            }
        }
        count >= min && k(self, pos)
    }

    fn lazy(&mut self, inner: &Node, min: u32, max: u32, count: u32, pos: usize, k: &mut dyn FnMut(&mut Matcher<'a>, usize) -> bool) -> bool {
        if count >= min && k(self, pos) {
            return true;
        }
        if count < max {
            return self.run(inner, pos, &mut |m, p| {
                if p == pos && count >= min {
                    return false;
                }
                m.lazy(inner, min, max, count + 1, p, k)
            });
        }
        false
    }
}

/// If `node` is an alternation of plain literals with a uniform folding mode,
/// list them (used to replace matching by a set lookup).
fn literal_alternatives(node: &Node, out: &mut Vec<(String, u8)>) -> bool {
    fn literal(node: &Node, s: &mut String, ci: &mut Option<u8>) -> bool {
        match node {
            Node::Empty => true,
            Node::Char(c, f) => {
                if ci.is_some_and(|x| x != *f) {
                    return false;
                }
                *ci = Some(*f);
                s.push(*c);
                true
            }
            Node::Concat(items) => items.iter().all(|n| literal(n, s, ci)),
            Node::Group(inner, _) => literal(inner, s, ci),
            _ => false,
        }
    }
    match node {
        Node::Alt(alts) => alts.iter().all(|a| literal_alternatives(a, out)),
        Node::Group(inner, _) => literal_alternatives(inner, out),
        _ => {
            let mut s = String::new();
            let mut ci = None;
            if literal(node, &mut s, &mut ci) {
                out.push((s, ci.unwrap_or(0)));
                true
            } else {
                false
            }
        }
    }
}

/// Fold a string for a case-insensitive literal lookup.
fn fold_str(s: &str, ci: u8) -> String {
    match ci {
        1 => s.to_ascii_lowercase(),
        _ => s.chars().map(fold).collect(),
    }
}

impl Regex {
    pub fn new(src: &str, flags: Flags) -> Result<Regex, Error> {
        let mut p = Parser { c: src.chars().collect(), i: 0, flags, groups: 0, names: Vec::new(), inline_flags: false, _src: src };
        let node = p.alt()?;
        if p.i < p.c.len() {
            return p.err("unbalanced )");
        }
        // Literal alternation fast path. Groups make no difference to
        // `matches`, and callers needing captures use `captures`.
        let mut lits = Vec::new();
        let (literals, fold) = if literal_alternatives(&node, &mut lits) && !lits.is_empty() {
            let ci = lits[0].1;
            if lits.iter().all(|(_, f)| *f == ci) {
                let set = lits
                    .into_iter()
                    .map(|(s, _)| if ci == 0 { s } else { fold_str(&s, ci) })
                    .collect();
                (Some(set), ci)
            } else {
                (None, 0)
            }
        } else {
            (None, 0)
        };
        Ok(Regex { node, groups: p.groups, names: p.names, literals, fold, inline_flags: p.inline_flags, src: src.to_string() })
    }

    pub fn source(&self) -> &str {
        &self.src
    }

    pub fn group_count(&self) -> usize {
        self.groups
    }

    pub fn group_index(&self, name: &str) -> Option<usize> {
        self.names.iter().find(|(n, _)| n == name).map(|(_, i)| *i)
    }

    /// The finite set of strings this pattern can match, when it is built only
    /// from literals, groups, alternation and `?` (LanguageTool's
    /// `StringMatcher.getPossibleRegexpValues`). `None` when the pattern uses
    /// anything else, or when the set would be large.
    pub fn possible_values(&self) -> Option<Vec<String>> {
        fn expand(node: &Node) -> Option<Vec<String>> {
            const LIMIT: usize = 2000;
            match node {
                Node::Empty => Some(vec![String::new()]),
                Node::Char(c, _) => Some(vec![c.to_string()]),
                Node::Group(inner, _) => expand(inner),
                Node::Alt(alts) => {
                    let mut out = Vec::new();
                    for a in alts {
                        out.extend(expand(a)?);
                        if out.len() > LIMIT {
                            return None;
                        }
                    }
                    Some(out)
                }
                Node::Concat(items) => {
                    let mut out = vec![String::new()];
                    for it in items {
                        let part = expand(it)?;
                        if out.len() * part.len() > LIMIT {
                            return None;
                        }
                        let mut next = Vec::with_capacity(out.len() * part.len());
                        for a in &out {
                            for b in &part {
                                next.push(format!("{a}{b}"));
                            }
                        }
                        out = next;
                    }
                    Some(out)
                }
                Node::Repeat(inner, 0, 1, Greed::Greedy) => {
                    let mut out = vec![String::new()];
                    out.extend(expand(inner)?);
                    Some(out)
                }
                _ => None,
            }
        }
        // With inline flags the values would not all compare the same way.
        if self.inline_flags {
            return None;
        }
        let mut v = expand(&self.node)?;
        v.sort();
        v.dedup();
        Some(v)
    }

    /// `Matcher.matches()`: the whole input must match.
    pub fn is_full_match(&self, text: &str) -> bool {
        if let Some(set) = &self.literals {
            return match self.fold {
                0 => set.contains(text),
                ci => set.contains(fold_str(text, ci).as_str()),
            };
        }
        let chars: Vec<char> = text.chars().collect();
        self.full_match_chars(&chars).is_some()
    }

    pub fn full_match_chars(&self, chars: &[char]) -> Option<Captures> {
        let mut m = Matcher { input: chars, caps: vec![None; self.groups + 1], steps: 0 };
        let n = chars.len();
        if m.run(&self.node, 0, &mut |_, e| e == n) {
            m.caps[0] = Some((0, n));
            Some(Captures { spans: m.caps })
        } else {
            None
        }
    }

    /// `Matcher.find()` from `start`: the leftmost match at or after it.
    pub fn find_chars(&self, chars: &[char], start: usize) -> Option<Captures> {
        for s in start..=chars.len() {
            let mut m = Matcher { input: chars, caps: vec![None; self.groups + 1], steps: 0 };
            let mut end = None;
            if m.run(&self.node, s, &mut |_, e| {
                end = Some(e);
                true
            }) {
                m.caps[0] = Some((s, end.unwrap()));
                return Some(Captures { spans: m.caps });
            }
        }
        None
    }

    /// `Matcher.find()`: does the pattern match anywhere in `text`?
    pub fn is_match(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        self.find_chars(&chars, 0).is_some()
    }

    /// `Matcher.lookingAt()`: a match anchored at `start` only.
    pub fn match_at_chars(&self, chars: &[char], start: usize) -> Option<Captures> {
        let mut m = Matcher { input: chars, caps: vec![None; self.groups + 1], steps: 0 };
        let mut end = None;
        if m.run(&self.node, start, &mut |_, e| {
            end = Some(e);
            true
        }) {
            m.caps[0] = Some((start, end.unwrap()));
            Some(Captures { spans: m.caps })
        } else {
            None
        }
    }

    /// All non-overlapping matches, as char ranges with their captures.
    pub fn find_all(&self, chars: &[char]) -> Vec<Captures> {
        let mut out = Vec::new();
        let mut pos = 0;
        while pos <= chars.len() {
            match self.find_chars(chars, pos) {
                Some(c) => {
                    let (s, e) = c.spans[0].unwrap();
                    pos = if e == s { e + 1 } else { e };
                    out.push(c);
                }
                None => break,
            }
        }
        out
    }

    /// `Matcher.replaceAll(replacement)`, with `$1`/`${name}` references and
    /// `\` escapes in the replacement.
    pub fn replace_all(&self, text: &str, replacement: &str) -> String {
        self.replace(text, replacement, true)
    }

    /// `Matcher.replaceFirst(replacement)`.
    pub fn replace_first(&self, text: &str, replacement: &str) -> String {
        self.replace(text, replacement, false)
    }

    fn replace(&self, text: &str, replacement: &str, all: bool) -> String {
        let chars: Vec<char> = text.chars().collect();
        let repl: Vec<char> = replacement.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        let mut pos = 0;
        while pos <= chars.len() {
            let Some(c) = self.find_chars(&chars, pos) else { break };
            let (s, e) = c.spans[0].unwrap();
            out.extend(&chars[last..s]);
            let mut i = 0;
            while i < repl.len() {
                match repl[i] {
                    '\\' if i + 1 < repl.len() => {
                        out.push(repl[i + 1]);
                        i += 2;
                    }
                    '$' if i + 1 < repl.len() && repl[i + 1].is_ascii_digit() => {
                        let mut n = repl[i + 1].to_digit(10).unwrap() as usize;
                        i += 2;
                        while i < repl.len() && repl[i].is_ascii_digit() && n * 10 + (repl[i] as usize - '0' as usize) <= self.groups {
                            n = n * 10 + (repl[i] as usize - '0' as usize);
                            i += 1;
                        }
                        if let Some(Some((a, b))) = c.spans.get(n) {
                            out.extend(&chars[*a..*b]);
                        }
                    }
                    '$' if i + 1 < repl.len() && repl[i + 1] == '{' => {
                        let close = repl[i..].iter().position(|&x| x == '}').map(|p| p + i).unwrap_or(repl.len());
                        let name: String = repl[i + 2..close].iter().collect();
                        if let Some(Some((a, b))) = self.group_index(&name).and_then(|n| c.spans.get(n)) {
                            out.extend(&chars[*a..*b]);
                        }
                        i = (close + 1).min(repl.len());
                    }
                    ch => {
                        out.push(ch);
                        i += 1;
                    }
                }
            }
            last = e;
            pos = if e == s { e + 1 } else { e };
            if !all {
                break;
            }
        }
        if last <= chars.len() {
            out.extend(&chars[last..]);
        }
        out
    }
}

impl Captures {
    /// Text of group `n`, or `None` when it did not participate.
    pub fn get<'t>(&self, chars: &'t [char], n: usize) -> Option<String> {
        self.spans.get(n).copied().flatten().map(|(a, b)| chars[a..b].iter().collect())
    }
}

/// Compiled patterns are shared: rule files repeat the same expressions
/// thousands of times.
pub fn compile(src: &str, flags: Flags) -> Result<Arc<Regex>, Error> {
    static CACHE: OnceLock<Mutex<HashMap<(String, Flags), Arc<Regex>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(r) = cache.lock().unwrap().get(&(src.to_string(), flags)) {
        return Ok(r.clone());
    }
    let r = Arc::new(Regex::new(src, flags)?);
    cache.lock().unwrap().insert((src.to_string(), flags), r.clone());
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full(p: &str, t: &str) -> bool {
        Regex::new(p, Flags::default()).unwrap().is_full_match(t)
    }
    fn full_i(p: &str, t: &str) -> bool {
        Regex::new(p, Flags::IGNORE_CASE).unwrap().is_full_match(t)
    }
    fn find(p: &str, t: &str) -> bool {
        Regex::new(p, Flags::default()).unwrap().is_match(t)
    }

    #[test]
    fn literals_alternation_and_classes() {
        assert!(full("tout|toute|toutes", "toutes"));
        assert!(!full("tout|toute|toutes", "Toutes"));
        assert!(full_i("tout|toute|toutes", "Toutes"));
        assert!(full_i("école", "ÉCOLE"));
        assert!(full("[mts]on", "ton"));
        assert!(!full("[mts]on", "bon"));
        assert!(full("[^aeiou]+", "xyz"));
        assert!(full("N [mfe] (s|p|sp)", "N f sp"));
        assert!(full("V .* ppa.*", "V etre ppa m s"));
        assert!(!full("V .* ppa.*", "V ppa m s"));
        assert!(full(".*[iu]e?s?", "vertues"));
        assert!(full("\\d+([,.]\\d+)?", "12,5"));
        assert!(full("[a-zà-ÿ&&[^aeiou]]+", "brçt"));
        assert!(!full("[a-z&&[^aeiou]]+", "bat"));
        assert!(full("\\p{Lu}\\p{Ll}+", "État"));
        assert!(full("\\p{L}+", "œuvre"));
        assert!(!full("\\P{L}+", "a"));
        assert!(full("[\\p{L}\\-]+", "peut-être"));
    }

    #[test]
    fn matches_is_whole_string_and_find_is_anywhere() {
        assert!(!full("pour", "pourquoi"));
        assert!(find("pour", "pourquoi"));
        assert!(find("^(?:i\\.e\\.)$", "i.e."));
        assert!(!find("^(?:i\\.e\\.)$", "x.i.e."));
        assert!(full("a|ab", "ab"), "matches() must backtrack into the alternation");
        assert!(full("(a|ab)(c|bcd)?", "abcd"));
    }

    #[test]
    fn quantifiers() {
        assert!(full("a{2}", "aa"));
        assert!(!full("a{3}", "aa"));
        assert!(full("a{2,}", "aaaa"));
        assert!(full("a{1,2}b", "aab"));
        assert!(full("x{", "x{"));
        assert!(full("a+?b", "aaab"));
        assert!(!full("a++a", "aaa"), "possessive does not give back");
        assert!(full("(?>a+)b", "aab"));
        assert!(full("(a*)*b", "aab"), "empty iterations terminate");
    }

    #[test]
    fn flags() {
        assert!(full("(?i)paris", "PARIS"));
        assert!(!full("(?i)école", "École"), "(?i) alone folds ASCII only, as in Java");
        assert!(full("(?iu)école", "École"));
        assert!(full("a(?i:b)c", "aBc"));
        assert!(!full("a(?i:b)c", "aBC"));
        assert!(full("(?i)a(?-i)b", "Ab"));
        assert!(!full("(?i)a(?-i)b", "AB"));
        assert!(!full("a.b", "a\nb"));
        assert!(full("(?s)a.b", "a\nb"));
    }

    #[test]
    fn lookaround_backrefs_and_boundaries() {
        assert!(full("(?!non).*", "oui"));
        assert!(!full("(?!non).*", "nonobstant"));
        assert!(find("(?<=l')homme", "l'homme"));
        assert!(!find("(?<!l')homme", "l'homme"));
        assert!(find("(?<![a-z])homme", "un homme"));
        assert!(full("(\\w)\\1", "ee"));
        assert!(!full("(\\w)\\1", "ea"));
        assert!(full("(?<x>a)\\k<x>", "aa"));
        assert!(find("\\bété\\b", "cet été chaud"));
        assert!(!find("\\bété\\b", "répétée"));
        assert!(find("\\Bet", "chouette"));
    }

    #[test]
    fn replace() {
        let r = Regex::new("(.*)s$", Flags::default()).unwrap();
        assert_eq!(r.replace_all("chats", "$1"), "chat");
        let r = Regex::new("a", Flags::default()).unwrap();
        assert_eq!(r.replace_all("banana", "o"), "bonono");
        assert_eq!(r.replace_first("banana", "o"), "bonana");
        let r = Regex::new("(?i)(é)", Flags::default()).unwrap();
        assert_eq!(r.replace_all("été", "[$1]"), "[é]t[é]");
        let r = Regex::new("x*", Flags::default()).unwrap();
        assert_eq!(r.replace_all("ab", "-"), "-a-b-");
        let r = Regex::new("(\\d+)\\.(\\d+)", Flags::default()).unwrap();
        assert_eq!(r.replace_all("1.5 et 2.25", "$2\\$$1"), "5$1 et 25$2");
    }

    #[test]
    fn unicode_character_class() {
        let ascii = Regex::new("\\w+", Flags::default()).unwrap();
        let uni = Regex::new("\\w+", Flags::UNICODE_CLASS).unwrap();
        assert!(!ascii.is_full_match("été"));
        assert!(uni.is_full_match("été"));
        assert!(Regex::new("a\\sb", Flags::UNICODE_CLASS).unwrap().is_full_match("a\u{a0}b"));
        assert!(!Regex::new("a\\sb", Flags::default()).unwrap().is_full_match("a\u{a0}b"));
    }

    #[test]
    fn possible_values() {
        let pv = |p: &str| Regex::new(p, Flags::default()).unwrap().possible_values();
        assert_eq!(pv("une?"), Some(vec!["un".to_string(), "une".to_string()]));
        assert_eq!(pv("(le|la)s?").map(|v| v.len()), Some(4));
        assert_eq!(pv("tout|toute"), Some(vec!["tout".to_string(), "toute".to_string()]));
        assert_eq!(pv("[mts]on"), None);
        assert_eq!(pv("a+"), None);
        assert_eq!(pv("a.c"), None);
        assert_eq!(pv("(?-i)VIE"), None);
    }

    #[test]
    fn quoting_and_escapes() {
        assert!(full("\\Qa.b\\E", "a.b"));
        assert!(!full("\\Qa.b\\E", "axb"));
        assert!(full("\\u00e9t\\xe9", "été"));
        assert!(full("[\\]\\[]+", "[]"));
        assert!(full("[a\\-z]+", "a-z"));
        assert!(full("[-a]+", "-a"));
        assert!(full("\\.", "."));
    }
    #[test]
    fn case_insensitive_negated_class() {
        let ci = Flags { case_insensitive: true, unicode_case: true, ..Flags::default() };
        let re = Regex::new("[^a-z].*|qui", ci).unwrap();
        assert!(!re.is_full_match("faiblesse"));
        assert!(!re.is_full_match("Faiblesse"));
        assert!(re.is_full_match("3e"));
        assert!(re.is_full_match("QUI"));
        assert!(Regex::new("[^é]", ci).unwrap().is_full_match("e"));
        assert!(!Regex::new("[^é]", ci).unwrap().is_full_match("É"));
    }
}
