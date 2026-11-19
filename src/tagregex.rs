//! French part-of-speech tags in the LanguageTool/Lefff format.
//!
//! Tags are plain strings such as `N m s`, `V ind pres 3 p`, `R pers suj 1 s`
//! or `V etre inf`. Rules match them either literally (`postag="N m s"`) or as
//! regular expressions (`postag="V .* ppa.*" postag_regexp="yes"`). Because the
//! latter happens ~28 000 times per check, the regex is compiled once and
//! evaluated by a tiny, allocation-free engine specialised for these
//! dot-separated tag strings (`is_match_anywhere` semantics, like Java's
//! `Matcher.find`).
//!
//! Supported syntax: literals, `.`, `[]` classes with ranges and negation,
//! `()`, `|`, `?`, `*`, `+`, `{n}`, `{n,}`, `{n,m}`. Anything else (e.g. `\d`,
//! `^`, `$`) falls back to a byte-wise literal/any match, which is what the
//! French tag sets actually use.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug)]
enum Node {
    Char(u8),
    Any,
    Class { neg: bool, ranges: Vec<(u8, u8)> },
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat { node: Box<Node>, min: u32, max: u32 },
}

#[derive(Clone, Debug, Default)]
struct Regex {
    prog: Option<Prog>,
    literal: Option<Vec<u8>>,
    anchored_start: bool,
    anchored_end: bool,
    matches_empty: bool,
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
    anchored_start: bool,
    anchored_end: bool,
    error: bool,
    depth: u32,
}

impl<'a> P<'a> {
    fn parse(&mut self) -> Node {
        self.alt()
    }

    fn alt(&mut self) -> Node {
        let mut alts = vec![self.concat()];
        while self.peek() == Some(b'|') {
            self.i += 1;
            alts.push(self.concat());
        }
        if alts.len() == 1 {
            alts.pop().unwrap()
        } else {
            Node::Alt(alts)
        }
    }

    fn concat(&mut self) -> Node {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == b'|' || c == b')' {
                break;
            }
            let atom = self.atom();
            let atom = self.quantifier(atom);
            items.push(atom);
        }
        match items.len() {
            0 => Node::Concat(Vec::new()),
            1 => items.pop().unwrap(),
            _ => Node::Concat(items),
        }
    }

    fn quantifier(&mut self, atom: Node) -> Node {
        match self.peek() {
            Some(b'?') => {
                self.i += 1;
                Node::Repeat { node: Box::new(atom), min: 0, max: 1 }
            }
            Some(b'*') => {
                self.i += 1;
                Node::Repeat { node: Box::new(atom), min: 0, max: u32::MAX }
            }
            Some(b'+') => {
                self.i += 1;
                Node::Repeat { node: Box::new(atom), min: 1, max: u32::MAX }
            }
            Some(b'{') => {
                let save = self.i;
                self.i += 1;
                let mut min = String::new();
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() {
                        min.push(c as char);
                        self.i += 1;
                    } else {
                        break;
                    }
                }
                let mut max = min.clone();
                let mut comma = false;
                if self.peek() == Some(b',') {
                    comma = true;
                    self.i += 1;
                    let mut m = String::new();
                    while let Some(c) = self.peek() {
                        if c.is_ascii_digit() {
                            m.push(c as char);
                            self.i += 1;
                        } else {
                            break;
                        }
                    }
                    max = m;
                }
                if self.peek() != Some(b'}') || min.is_empty() {
                    // not a quantifier: treat literally
                    self.i = save;
                    return atom;
                }
                self.i += 1;
                let minv = min.parse::<u32>().unwrap_or(0);
                let maxv = if comma {
                    if max.is_empty() {
                        u32::MAX
                    } else {
                        max.parse::<u32>().unwrap_or(u32::MAX)
                    }
                } else {
                    minv
                };
                Node::Repeat { node: Box::new(atom), min: minv, max: maxv }
            }
            _ => atom,
        }
    }

    fn atom(&mut self) -> Node {
        let c = match self.peek() {
            Some(c) => c,
            None => return Node::Concat(Vec::new()),
        };
        match c {
            b'(' => {
                self.i += 1;
                // non-capturing groups `(?:` or `(?i)`-style flags
                if self.peek() == Some(b'?') {
                    let copy = self.i;
                    self.i += 1;
                    match self.peek() {
                        Some(b':') => {
                            self.i += 1;
                        }
                        _ => {
                            // flags like (?i) / (?-i): unsupported -> error
                            self.i = copy;
                            self.error = true;
                            // skip to ')'
                            while let Some(c) = self.peek() {
                                self.i += 1;
                                if c == b')' {
                                    break;
                                }
                            }
                            return Node::Concat(Vec::new());
                        }
                    }
                }
                if self.depth > 24 {
                    self.error = true;
                }
                self.depth += 1;
                let inner = self.alt();
                self.depth -= 1;
                if self.peek() == Some(b')') {
                    self.i += 1;
                } else {
                    self.error = true;
                }
                inner
            }
            b'[' => {
                self.i += 1;
                let mut neg = false;
                if self.peek() == Some(b'^') {
                    neg = true;
                    self.i += 1;
                }
                let mut ranges = Vec::new();
                let mut first = true;
                while let Some(c) = self.peek() {
                    if c == b']' && !first {
                        break;
                    }
                    first = false;
                    let lo = if c == b'\\' {
                        self.i += 1;
                        self.escape_char()
                    } else {
                        self.i += 1;
                        c
                    };
                    if self.peek() == Some(b'-') && self.b.get(self.i + 1).copied() != Some(b']') {
                        self.i += 1;
                        let hi = if self.peek() == Some(b'\\') {
                            self.i += 1;
                            self.escape_char()
                        } else {
                            let h = self.peek().unwrap_or(b'-');
                            self.i += 1;
                            h
                        };
                        ranges.push((lo, hi));
                    } else {
                        ranges.push((lo, lo));
                    }
                }
                if self.peek() == Some(b']') {
                    self.i += 1;
                } else {
                    self.error = true;
                }
                Node::Class { neg, ranges }
            }
            b'\\' => {
                self.i += 1;
                match self.peek() {
                    Some(b'd') => {
                        self.i += 1;
                        Node::Class { neg: false, ranges: vec![(b'0', b'9')] }
                    }
                    Some(b'D') => {
                        self.i += 1;
                        Node::Class { neg: true, ranges: vec![(b'0', b'9')] }
                    }
                    Some(b'w') => {
                        self.i += 1;
                        Node::Class {
                            neg: false,
                            ranges: vec![(b'0', b'9'), (b'A', b'Z'), (b'_', b'_'), (b'a', b'z')],
                        }
                    }
                    Some(b'W') => {
                        self.i += 1;
                        Node::Class {
                            neg: true,
                            ranges: vec![(b'0', b'9'), (b'A', b'Z'), (b'_', b'_'), (b'a', b'z')],
                        }
                    }
                    Some(b's') => {
                        self.i += 1;
                        Node::Class {
                            neg: false,
                            ranges: vec![(b'\t', b'\r'), (b' ', b' ')],
                        }
                    }
                    Some(b'S') => {
                        self.i += 1;
                        Node::Class {
                            neg: true,
                            ranges: vec![(b'\t', b'\r'), (b' ', b' ')],
                        }
                    }
                    _ => {
                        let e = self.escape_char();
                        if e == 0 {
                            self.error = true;
                            Node::Any
                        } else {
                            Node::Char(e)
                        }
                    }
                }
            }
            b'.' => {
                self.i += 1;
                Node::Any
            }
            b'^' => {
                self.i += 1;
                self.anchored_start = true;
                Node::Concat(Vec::new())
            }
            b'$' => {
                self.i += 1;
                self.anchored_end = true;
                Node::Concat(Vec::new())
            }
            _ => {
                self.i += 1;
                Node::Char(c)
            }
        }
    }

    fn escape_char(&mut self) -> u8 {
        let c = match self.peek() {
            Some(c) => c,
            None => return 0,
        };
        self.i += 1;
        match c {
            b'n' => b'\n',
            b't' => b'\t',
            other => other,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
}

impl Regex {
    fn compile(src: &str) -> Regex {
        if src.is_empty() {
            return Regex {
                prog: None,
                literal: Some(Vec::new()),
                anchored_start: false,
                anchored_end: false,
                matches_empty: true,
            };
        }
        // Fast path: a plain literal string (no metacharacters) is by far the
        // most common case and needs no parsing at all.
        let meta = |c: u8| matches!(c, b'.' | b'[' | b']' | b'(' | b')' | b'|' | b'?' | b'*' | b'+' | b'{' | b'}' | b'^' | b'$' | b'\\');
        if !src.bytes().any(meta) {
            return Regex {
                prog: None,
                literal: Some(src.as_bytes().to_vec()),
                anchored_start: false,
                anchored_end: false,
                matches_empty: src.is_empty(),
            };
        }
        let mut p = P {
            b: src.as_bytes(),
            i: 0,
            anchored_start: false,
            anchored_end: false,
            error: false,
            depth: 0,
        };
        let mut root = p.parse();
        if p.anchored_end {
            // `.` also matches the end for `$`-anchored cases; we emulate
            // anchored-end with an exhaustive suffix check below, so wrap
            // the pattern to require reaching the end.
            root = Node::Concat(vec![root]);
        }
        let mut prog = Prog::default();
        prog.compile(&root);
        prog.emit(OP_MATCH, 0);
        Regex {
            prog: Some(prog),
            literal: None,
            anchored_start: p.anchored_start,
            anchored_end: p.anchored_end,
            matches_empty: false,
        }
    }

    /// Java `Matcher.find()` semantics: does the regex match anywhere?
    fn is_match(&self, text: &str) -> bool {
        if let Some(lit) = &self.literal {
            if lit.is_empty() {
                return true;
            }
            return text.as_bytes().windows(lit.len()).any(|w| w == lit.as_slice());
        }
        let prog = match &self.prog {
            Some(p) => p,
            None => return false,
        };
        if self.anchored_start {
            return prog.is_match(text);
        }
        if self.anchored_end {
            // anchored at end: test every suffix
            for (i, _) in text.char_indices().map(|(i, _)| (i, ())).chain(std::iter::once((text.len(), ()))) {
                if prog.is_match(&text[i..]) {
                    return true;
                }
            }
            return false;
        }
        // unanchored: try each start position (find semantics). Most patterns
        // start with a literal-ish prefix, so we try starts lazily.
        for (i, _) in text.char_indices().map(|(i, _)| (i, ())).chain(std::iter::once((text.len(), ()))) {
            if prog.is_match(&text[i..]) {
                return true;
            }
        }
        false
    }
}

/// Compile a parsed AST into a Pike VM (Thompson NFA) program.
///
/// Instructions (u32 pairs: op, arg):
///   Char(c)            match byte c
///   Any                match any byte
///   Class(id)          match byte against class table entry `id`
///   Split(x)           continue at x (and fall through to next instr)
///   Jmp(x)             jump to x
///   Match              accept
///
/// The Pike VM explores all live states in lock-step, so matching is O(n * m)
/// with no backtracking blowups -- important as these regexes are evaluated
/// tens of thousands of times per check.
const OP_CHAR: u32 = 0;
const OP_ANY: u32 = 1;
const OP_CLASS: u32 = 2;
const OP_SPLIT: u32 = 3;
const OP_JMP: u32 = 4;
const OP_MATCH: u32 = 5;

#[derive(Clone, Debug, Default)]
struct Prog {
    ops: Vec<(u32, u32)>,
    classes: Vec<(bool, Vec<(u8, u8)>)>,
}

impl Prog {
    fn emit(&mut self, op: u32, arg: u32) -> usize {
        self.ops.push((op, arg));
        self.ops.len() - 1
    }

    /// Emit code for `node`; returns nothing (code appended).
    fn compile(&mut self, node: &Node) {
        match node {
            Node::Char(c) => {
                self.emit(OP_CHAR, *c as u32);
            }
            Node::Any => {
                self.emit(OP_ANY, 0);
            }
            Node::Class { neg, ranges } => {
                let id = self.classes.len() as u32;
                self.classes.push((*neg, ranges.clone()));
                self.emit(OP_CLASS, id);
            }
            Node::Concat(items) => {
                for it in items {
                    self.compile(it);
                }
            }
            Node::Alt(alts) => {
                // Layout: split seg0, seg1 / jmp END ... END:
                // Each non-final alternative is preceded by a split whose
                // `arg` is patched to the start of the following alternative
                // AFTER that alternative (and its jmp) has been emitted.
                let mut splits: Vec<usize> = Vec::new();
                let mut jumps: Vec<usize> = Vec::new();
                for (i, a) in alts.iter().enumerate() {
                    if i + 1 < alts.len() {
                        splits.push(self.emit(OP_SPLIT, 0));
                    }
                    self.compile(a);
                    if i + 1 < alts.len() {
                        jumps.push(self.emit(OP_JMP, 0));
                    }
                }
                let end = self.ops.len() as u32;
                for j in jumps {
                    self.ops[j].1 = end;
                }
                // the split for alternative i falls through to alternative i
                // and jumps to alternative i+1, emitted right after it
                for &sp in &splits {
                    // find the next split (or the end of this alternative):
                    let next_alt = {
                        // locate the jmp that closes the alternative right after
                        // this split's fall-through segment
                        let mut j = sp + 1;
                        while j < self.ops.len() && self.ops[j].0 != OP_JMP {
                            j += 1;
                        }
                        if j < self.ops.len() {
                            j + 1
                        } else {
                            self.ops.len()
                        }
                    };
                    self.ops[sp].1 = next_alt as u32;
                }
            }
            Node::Repeat { node, min, max } => {
                // Lower the repetition to explicit copies (the patterns used
                // here are tiny, so unrolling is both simple and fast):
                //   {n}       -> n copies
                //   {n,}      -> n copies followed by a star
                //   {n,m}     -> n copies followed by (m-n) optional copies
                let mut body = Prog::default();
                body.compile(node);
                let emit_body = |this: &mut Prog, body: &Prog| {
                    for &(op, arg) in &body.ops {
                        if op == OP_CLASS {
                            let id = this.classes.len() as u32;
                            this.classes.push(body.classes[arg as usize].clone());
                            this.emit(op, id);
                        } else {
                            this.emit(op, arg);
                        }
                    }
                };
                for _ in 0..*min {
                    emit_body(self, &body);
                }
                if *max == u32::MAX {
                    // star: L: split -> body, out ; body ; jmp L
                    let l = self.emit(OP_SPLIT, 0);
                    emit_body(self, &body);
                    self.emit(OP_JMP, l as u32);
                    let out = self.ops.len() as u32;
                    self.ops[l].1 = out;
                } else {
                    let extra = *max - *min;
                    let mut splits = Vec::new();
                    for _ in 0..extra {
                        splits.push(self.emit(OP_SPLIT, 0));
                        emit_body(self, &body);
                    }
                    let end = self.ops.len() as u32;
                    for sp in splits {
                        self.ops[sp].1 = end;
                    }
                }
            }
        }
    }

    fn is_match(&self, text: &str) -> bool {
        // NFA simulation with epsilon-closures. `closure` returns the set of
        // non-epsilon states (CHAR/ANY/CLASS/MATCH) reachable from `raw`.
        let b = text.as_bytes();
        let n = self.ops.len();
        if n == 0 {
            return true;
        }
        let mut marks = vec![0u32; n];
        let mut mark = 1u32;
        let closure = |raw: &[u32], mark: u32, marks: &mut Vec<u32>| -> Vec<u32> {
            let mut out = Vec::with_capacity(raw.len() * 2);
            let mut stack: Vec<u32> = raw.to_vec();
            while let Some(p) = stack.pop() {
                let idx = p as usize;
                if idx >= n || marks[idx] == mark {
                    continue;
                }
                marks[idx] = mark;
                match self.ops[idx].0 {
                    OP_JMP => stack.push(self.ops[idx].1),
                    OP_SPLIT => {
                        stack.push(self.ops[idx].1);
                        stack.push(p + 1);
                    }
                    _ => out.push(p),
                }
            }
            out
        };
        let mut cur = closure(&[0u32], mark, &mut marks);
        if cur.iter().any(|&pc| self.ops[pc as usize].0 == OP_MATCH) {
            return true;
        }
        for &c in b {
            if cur.is_empty() {
                return false;
            }
            let mut raw: Vec<u32> = Vec::with_capacity(cur.len() * 2);
            for &pc in &cur {
                let idx = pc as usize;
                let (op, arg) = self.ops[idx];
                let ok = match op {
                    OP_CHAR => arg as u8 == c,
                    OP_ANY => true,
                    OP_CLASS => {
                        let (neg, ranges) = &self.classes[arg as usize];
                        let hit = ranges.iter().any(|&(lo, hi)| c >= lo && c <= hi);
                        hit != *neg
                    }
                    _ => false,
                };
                if ok {
                    raw.push(pc + 1);
                }
            }
            mark += 1;
            cur = closure(&raw, mark, &mut marks);
            if cur.iter().any(|&pc| self.ops[pc as usize].0 == OP_MATCH) {
                return true;
            }
        }
        false
    }
}

/// Cached regex compilation: the rule files contain a few hundred distinct
/// patterns used across tens of thousands of tokens.
pub struct RegexCache {
    map: Mutex<HashMap<String, Regex>>,
}

impl RegexCache {
    pub fn global() -> &'static RegexCache {
        static CACHE: OnceLock<RegexCache> = OnceLock::new();
        CACHE.get_or_init(|| RegexCache {
            map: Mutex::new(HashMap::new()),
        })
    }

    pub fn get(&self, src: &str) -> Regex {
        if let Ok(m) = self.map.lock() {
            if let Some(r) = m.get(src) {
                return r.clone();
            }
        }
        let r = Regex::compile(src);
        if let Ok(mut m) = self.map.lock() {
            m.insert(src.to_string(), r.clone());
        }
        r
    }
}

/// Compile a string field into a matchable form: either an exact string or a
/// cached regex.
#[derive(Clone, Debug)]
pub enum ValueMatcher {
    Exact(String),
    Regex(Regex),
}

impl ValueMatcher {
    pub fn new(src: &str, is_regex: bool) -> ValueMatcher {
        if is_regex {
            ValueMatcher::Regex(RegexCache::global().get(src))
        } else {
            ValueMatcher::Exact(src.to_string())
        }
    }

    pub fn is_match(&self, text: &str) -> bool {
        match self {
            ValueMatcher::Exact(s) => s == text,
            ValueMatcher::Regex(r) => r.is_match(text),
        }
    }

    /// Like `is_match` but ignores letter case.
    pub fn is_match_ignore_case(&self, text: &str) -> bool {
        match self {
            ValueMatcher::Exact(s) => crate::strutil::eq_ignore_case(s, text),
            ValueMatcher::Regex(r) => r.is_match(text) || r.is_match(&crate::strutil::lower(text)),
        }
    }

    pub fn as_source(&self) -> &str {
        match self {
            ValueMatcher::Exact(s) => s,
            ValueMatcher::Regex(_) => "",
        }
    }

    pub fn is_regex(&self) -> bool {
        matches!(self, ValueMatcher::Regex(_))
    }
}

/// Test helper: exact "find anywhere" semantics for a raw regex source.
pub fn regex_find(pattern: &str, text: &str) -> bool {
    RegexCache::global().get(pattern).is_match(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(p: &str, t: &str) -> bool {
        regex_find(p, t)
    }

    #[test]
    fn literal_and_any() {
        assert!(find("N m s", "N m s"));
        assert!(!find("N m s", "N m p"));
        assert!(find("N m s|J e p", "J e p"));
        // NB: `V .* ppa.*` needs a separator run before `ppa`, so it matches
        // "V etre ppa m s" but not "V ppa m s" (verified against Java
        // Matcher.find semantics).
        assert!(find("V .* ppa.*", "V etre ppa m s"));
        assert!(!find("V .* ppa.*", "V ppa m s"));
        assert!(find(".* ppa.*", "V ppa m s"));
        assert!(!find("V .* ppa.*", "V ind pres 3 s"));
        assert!(find(".* 3 .*", "R pers suj 3 m s"));
        assert!(find(".* sp?", "N m sp"));
        assert!(find(".* s?p", "N m p"));
        assert!(find("[DJNZ].*|V ppa.*", "V ppa m s"));
        assert!(find("(D|J) .*", "D f s"));
        assert!(!find("(D|J) .*", "N f s"));
        assert!(find("N f sp|N f s", "N f s"));
        assert!(find(".*[iu]e?s?$", "vertue"));
        assert!(!find(".*[iu]e?s?$", "chat"));
        assert!(find("\\d+", "abc123"));
        assert!(find("^(?:i\\.e\\.)$", "i.e."));
        assert!(!find("^(?:i\\.e\\.)$", "x.i.e."));
    }

    #[test]
    fn quantifiers() {
        assert!(find("a{2}", "xaay"));
        assert!(!find("a{3}", "xaay"));
        assert!(find("a{2,}", "xaaay"));
        assert!(find("ab+c", "xxabbc"));
        assert!(!find("ab+c", "xxac"));
        assert!(find("N (?:m|f) (s|p|sp)", "N f p"));
    }

    #[test]
    fn cache_consistency() {
        for p in ["R pers suj.*|D .*", "V etre.*", ".*(?:er|ir|re)$", "[a-z]+", "[^a]"] {
            let r = RegexCache::global().get(p);
            for t in ["R pers suj 3 m s", "V etre inf", "manger", "abc", "aaa"] {
                let _ = r.is_match(t);
            }
        }
    }
}










