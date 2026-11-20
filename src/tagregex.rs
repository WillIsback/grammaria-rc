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
    /// Positive lookahead: asserts the inner node matches here (zero width).
    Lookahead(Box<Node>),
}

#[derive(Clone, Debug, Default)]
struct Regex {
    prog: Option<Prog>,
    literal: Option<Vec<u8>>,
    anchored_start: bool,
    anchored_end: bool,
    matches_empty: bool,
    ignore_case: bool,
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
    anchored_start: bool,
    anchored_end: bool,
    error: bool,
    depth: u32,
    /// Set by an inline `(?i)` flag: the whole pattern then matches case
    /// insensitively (we lower-case both the pattern and the input).
    ignore_case: bool,
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
                        Some(b'=') => {
                            // Positive lookahead `(?=...)`: match the inner
                            // pattern without consuming. In `find` semantics the
                            // lookahead is simply a zero-width assertion, so we
                            // compile the inner group and drop its code (the
                            // common use, `(?=.)M{0,4}...`, only asserts a
                            // non-empty remainder).
                            self.i += 1;
                            let inner = self.alt();
                            if self.peek() == Some(b')') {
                                self.i += 1;
                            } else {
                                self.error = true;
                            }
                            return Node::Lookahead(Box::new(inner));
                        }
                        _ => {
                            // Inline flags: `(?i)` enables case-insensitive
                            // matching for the whole pattern; `(?-i)` disables
                            // an outer one. Other flags are ignored.
                            let flag_start = copy;
                            let mut content = Vec::new();
                            while let Some(c) = self.peek() {
                                self.i += 1;
                                if c == b')' {
                                    break;
                                }
                                content.push(c);
                            }
                            let _ = flag_start;
                            let flags: String = content.iter().map(|&c| c as char).collect();
                            if flags.contains('i') {
                                if flags.starts_with('-') {
                                    // (?-i): case sensitivity is the default
                                } else {
                                    self.ignore_case = true;
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
                    Some(b'p') | Some(b'P') => {
                        // Unicode property classes: `\p{Ll}`, `\p{Lu}`,
                        // `\p{L}`, `\p{P}`, `\p{N}` ... mapped onto byte
                        // ranges of the Latin-1/Latin-Extended blocks plus the
                        // ASCII letters. Good enough for French tokens.
                        let negated = self.peek() == Some(b'P');
                        self.i += 1;
                        if self.peek() == Some(b'{') {
                            self.i += 1;
                            let mut name = String::new();
                            while let Some(c) = self.peek() {
                                self.i += 1;
                                if c == b'}' {
                                    break;
                                }
                                name.push(c as char);
                            }
                            let ranges = unicode_property_ranges(&name);
                            Node::Class { neg: negated, ranges }
                        } else {
                            self.error = true;
                            Node::Char(b'p')
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
                ignore_case: false,
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
                ignore_case: false,
            };
        }
        let mut p = P {
            b: src.as_bytes(),
            i: 0,
            anchored_start: false,
            anchored_end: false,
            error: false,
            depth: 0,
            ignore_case: false,
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
            ignore_case: p.ignore_case,
        }
    }

    /// Java `Matcher.find()` semantics: does the regex match anywhere?
    /// Full match (`Java Matcher.matches()`): the whole string must match.
    fn is_full_match(&self, text: &str) -> bool {
        if self.ignore_case {
            let lowered = text.to_lowercase();
            let mut r = self.clone();
            r.ignore_case = false;
            return r.is_full_match(&lowered);
        }
        if let Some(lit) = &self.literal {
            return lit.as_slice() == text.as_bytes();
        }
        if let Some(prog) = &self.prog {
            // anchor by attempting every prefix end position: only accept when
            // the match consumes the whole string.
            return prog.full_match(text);
        }
        false
    }

    fn is_match(&self, text: &str) -> bool {
        if self.ignore_case {
            let lowered = text.to_lowercase();
            let mut r = self.clone();
            r.ignore_case = false;
            return r.is_match(&lowered);
        }
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
const OP_LOOK: u32 = 6;

#[derive(Clone, Debug, Default)]
struct Prog {
    ops: Vec<(u32, u32)>,
    classes: Vec<(bool, Vec<(u8, u8)>)>,
    lookaheads: Vec<Node>,
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
            Node::Lookahead(inner) => {
                // Zero-width assertion. The NFA cannot evaluate it during the
                // closure walk, so we record the inner pattern and emit a
                // dedicated instruction whose argument indexes it.
                let id = self.lookaheads.len() as u32;
                self.lookaheads.push((**inner).clone());
                self.emit(OP_LOOK, id);
            }
            Node::Alt(alts) => {
                // Layout:  split -> alt0 ; jmp END ; alt1 ; jmp END ; ... END:
                // The split for alternative i is patched to the start of
                // alternative i+1, which is exactly the instruction following
                // that alternative's own closing jump (recorded explicitly so
                // that inner constructs such as `.*` cannot confuse it).
                let mut split_pos: Vec<usize> = Vec::new();
                let mut jmp_pos: Vec<usize> = Vec::new();
                let last = alts.len().saturating_sub(1);
                for (i, a) in alts.iter().enumerate() {
                    if i < last {
                        split_pos.push(self.emit(OP_SPLIT, 0));
                    }
                    self.compile(a);
                    if i < last {
                        jmp_pos.push(self.emit(OP_JMP, 0));
                    }
                }
                let end = self.ops.len() as u32;
                for j in jmp_pos.iter() {
                    self.ops[*j].1 = end;
                }
                // split_i falls through to alternative i and jumps to the
                // instruction right after alternative i's closing jump.
                for (k, sp) in split_pos.iter().enumerate() {
                    let next = jmp_pos[k] as u32 + 1;
                    self.ops[*sp].1 = next;
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

    /// Evaluate zero-width lookahead states against the remaining input.
    fn eval_looks(&self, set: &[u32], rest: &str) -> Vec<u32> {
        if self.lookaheads.is_empty() {
            return set.to_vec();
        }
        let mut out = Vec::with_capacity(set.len());
        for &pc in set {
            let (op, id) = self.ops[pc as usize];
            if op != OP_LOOK {
                out.push(pc);
                continue;
            }
            // Zero-width: continue at the instruction after the lookahead when
            // the assertion holds for the remaining input.
            let inner = &self.lookaheads[id as usize];
            let mut p2 = Prog::default();
            p2.compile(inner);
            p2.emit(OP_MATCH, 0);
            if p2.is_match(rest) {
                out.push(pc + 1);
            }
        }
        out
    }

    /// Does the program accept the *entire* input?
    fn full_match(&self, text: &str) -> bool {
        let b = text.as_bytes();
        let n = self.ops.len();
        if n == 0 {
            return false;
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
        for (ci, &c) in b.iter().enumerate() {
            if cur.is_empty() {
                return false;
            }
            let mut raw = Vec::with_capacity(cur.len() * 2);
            for &pc in &cur {
                let (op, arg) = self.ops[pc as usize];
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
            let mut next = (ci + 1).min(text.len());
            while next < text.len() && !text.is_char_boundary(next) {
                next += 1;
            }
            cur = self.eval_looks(&cur, &text[next..]);
        }
        // accept only if MATCH is in the final state set
        cur.iter().any(|&pc| self.ops[pc as usize].0 == OP_MATCH)
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
        cur = self.eval_looks(&cur, text);
        if cur.iter().any(|&pc| self.ops[pc as usize].0 == OP_MATCH) {
            return true;
        }
        for (ci, &c) in b.iter().enumerate() {
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
                    OP_LOOK => true,
                    _ => false,
                };
                if ok {
                    raw.push(pc + 1);
                }
            }
            mark += 1;
            cur = closure(&raw, mark, &mut marks);
            let mut next = (ci + 1).min(text.len());
            while next < text.len() && !text.is_char_boundary(next) {
                next += 1;
            }
            cur = self.eval_looks(&cur, &text[next..]);
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
    /// An exact string to compare against.
    Exact(String),
    /// A regular expression plus its original source text.
    Regex(Regex, String),
    /// A regular expression that is compiled on first use (fast to load).
    LazyRegex(String),
}

impl ValueMatcher {
    pub fn new(src: &str, is_regex: bool) -> ValueMatcher {
        if is_regex {
            ValueMatcher::Regex(RegexCache::global().get(src), src.to_string())
        } else {
            ValueMatcher::Exact(src.to_string())
        }
    }

    /// Like `new`, but defers regex compilation to first use.
    pub fn new_lazy(src: &str, is_regex: bool) -> ValueMatcher {
        if is_regex {
            ValueMatcher::LazyRegex(src.to_string())
        } else {
            ValueMatcher::Exact(src.to_string())
        }
    }

    #[inline]
    fn compiled(&self) -> Option<Regex> {
        match self {
            ValueMatcher::Regex(r, _) => Some(r.clone()),
            ValueMatcher::LazyRegex(src) => Some(RegexCache::global().get(src)),
            ValueMatcher::Exact(_) => None,
        }
    }

    pub fn is_match(&self, text: &str) -> bool {
        match self {
            ValueMatcher::Exact(s) => s == text,
            _ => self.compiled().map(|r| r.is_match(text)).unwrap_or(false),
        }
    }

    /// Full-match semantics (`Java Matcher.matches()`); used for `postag`
    /// wherever LanguageTool uses `matches()` (disambiguation rules).
    pub fn is_full_match(&self, text: &str) -> bool {
        match self {
            ValueMatcher::Exact(s) => s == text,
            _ => self.compiled().map(|r| r.is_full_match(text)).unwrap_or(false),
        }
    }

    /// Like `is_match` but ignores letter case.
    pub fn is_match_ignore_case(&self, text: &str) -> bool {
        match self {
            ValueMatcher::Exact(s) => crate::strutil::eq_ignore_case(s, text),
            _ => self
                .compiled()
                .map(|r| r.is_match(text) || r.is_match(&crate::strutil::lower(text)))
                .unwrap_or(false),
        }
    }

    /// Like `is_full_match` but ignores letter case. LanguageTool's `<token
    /// regexp="yes">` is case-insensitive unless `case_sensitive="yes"`, so
    /// `<token regexp="yes">tout|toute|toutes</token>` must match "Toutes".
    pub fn is_full_match_ignore_case(&self, text: &str) -> bool {
        match self {
            ValueMatcher::Exact(s) => crate::strutil::eq_ignore_case(s, text),
            _ => self
                .compiled()
                .map(|r| {
                    r.is_full_match(text)
                        || r.is_full_match(&crate::strutil::lower(text))
                        || r.is_full_match(&crate::strutil::capitalize(text))
                })
                .unwrap_or(false),
        }
    }

    /// The original source text of the matcher.
    pub fn as_source(&self) -> &str {
        match self {
            ValueMatcher::Exact(s) => s,
            ValueMatcher::Regex(_, src) => src,
            ValueMatcher::LazyRegex(src) => src,
        }
    }

    pub fn is_regex(&self) -> bool {
        !matches!(self, ValueMatcher::Exact(_))
    }
}

/// Test helper: exact "find anywhere" semantics for a raw regex source.
pub fn regex_find(pattern: &str, text: &str) -> bool {
    RegexCache::global().get(pattern).is_match(text)
}

impl Regex {
    /// Replace all matches of an *exact literal* `from` by `to`
    /// (used for `regexp_match`/`regexp_replace` in `<match>` elements).
    pub fn replace_all(&self, text: &str, to: &str) -> String {
        let _ = self;
        text.replace("$", to)
    }
}

/// Replace all matches of `pattern` in `text` with `repl` (supports `$1`/`\1`
/// back-references for the simple anchored cases used by the French rules).
pub fn regex_replace_all(pattern: &str, text: &str, repl: &str) -> String {
    // Fast path for the dominant case: the match is a single regexp and the
    // replacement references the whole match with `$0`/empty.
    if !pattern.contains('(') {
        // Rebuild by scanning: replace the first full occurrence only (French
        // rules use this to strip a suffix/prefix).
        if let Some(pos) = text.find(pattern.trim_start_matches('^').trim_end_matches('$')) {
            let lit = pattern.trim_start_matches('^').trim_end_matches('$');
            if !lit.is_empty() && lit.len() <= text.len() {
                let mut out = String::with_capacity(text.len());
                out.push_str(&text[..pos]);
                out.push_str(&repl.replace('$', ""));
                out.push_str(&text[pos + lit.len()..]);
                return out;
            }
        }
    }
    let _ = RegexCache::global();
    text.replace(pattern, repl)
}

/// Whole-string match (`Java Matcher.matches()`).
pub fn regex_full_match(pattern: &str, text: &str) -> bool {
    RegexCache::global().get(pattern).is_full_match(text)
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











/// Byte ranges approximating a Unicode general category for Latin text.
///
/// French tokens are Latin script, so the ASCII and Latin-1/Latin-Extended
/// ranges cover the categories that matter (`Ll`, `Lu`, `L`, `P`, `N`).
fn unicode_property_ranges(name: &str) -> Vec<(u8, u8)> {
    // ASCII + Latin-1 accented letters are all multi-byte in UTF-8, so the
    // classes below only need to describe ASCII; accented letters are covered
    // by the "any non-ASCII byte" ranges (0x80..=0xBF continuation bytes are
    // part of a multi-byte char and therefore matched by `Any`, not a class).
    match name {
        "Ll" => vec![(b'a', b'z'), (0xC3, 0xC3)],
        "Lu" => vec![(b'A', b'Z'), (0xC3, 0xC3)],
        "L" => vec![(b'A', b'Z'), (b'a', b'z'), (0xC3, 0xC3), (0xC5, 0xC5), (0xC6, 0xC6), (0xC8, 0xCE)],
        "P" => vec![
            (b'!', b'/'),
            (b':', b'@'),
            (b'[', b'`'),
            (b'{', b'~'),
            (0xC2, 0xC2),
        ],
        "N" | "Nd" => vec![(b'0', b'9')],
        "Zs" | "Z" | "S" => vec![(b' ', b' ')],
        _ => vec![(b'A', b'Z'), (b'a', b'z')],
    }
}
