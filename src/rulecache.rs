//! Binary cache for compiled rule sets.
//!
//! Parsing `grammar.xml` (9 MB) costs ~200 ms: building a DOM and converting
//! every token into a compiled matcher. Since rule files change only when the
//! assets are rebuilt, we serialize the compiled `RuleSet` once into a compact
//! binary blob that loads in a few milliseconds.
//!
//! Format: a small header, a string table, then the rule records. All integers
//! are little-endian `u32`; strings are `u32` length + UTF-8 bytes.

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

use crate::pattern::{Exception, Pattern, PatternElem, Scope, TokenPattern};
use crate::rules::{Example, MatchRef, Part, Priority, Rule, RuleSet};
use crate::tagregex::ValueMatcher;

pub const MAGIC: &[u8; 8] = b"GRMRULE1";

struct W {
    buf: Vec<u8>,
}

impl W {
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn bool(&mut self, v: bool) {
        self.u8(u8::from(v));
    }
    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.buf.extend_from_slice(s.as_bytes());
    }
    fn opt_str(&mut self, s: &Option<String>) {
        match s {
            Some(x) => {
                self.u8(1);
                self.str(x);
            }
            None => self.u8(0),
        }
    }
    fn matcher(&mut self, m: &ValueMatcher) {
        // 0 = none, 1 = exact, 2 = regex
        match m {
            ValueMatcher::Exact(s) => {
                self.u8(1);
                self.str(s);
            }
            ValueMatcher::Regex(..) | ValueMatcher::LazyRegex(_) => {
                self.u8(2);
                self.str(m.as_source());
            }
        }
    }
    fn opt_matcher(&mut self, m: &Option<ValueMatcher>) {
        match m {
            None => self.u8(0),
            Some(x) => self.matcher(x),
        }
    }
}

struct R<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> R<'a> {
    fn u32(&mut self) -> io::Result<u32> {
        if self.p + 4 > self.b.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "rule cache"));
        }
        let v = u32::from_le_bytes(self.b[self.p..self.p + 4].try_into().unwrap());
        self.p += 4;
        Ok(v)
    }
    fn u8(&mut self) -> io::Result<u8> {
        if self.p >= self.b.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "rule cache"));
        }
        let v = self.b[self.p];
        self.p += 1;
        Ok(v)
    }
    fn bool(&mut self) -> io::Result<bool> {
        Ok(self.u8()? != 0)
    }
    fn str(&mut self) -> io::Result<String> {
        let n = self.u32()? as usize;
        if self.p + n > self.b.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "rule cache"));
        }
        let s = std::str::from_utf8(&self.b[self.p..self.p + n])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
            .to_string();
        self.p += n;
        Ok(s)
    }
    fn opt_str(&mut self) -> io::Result<Option<String>> {
        Ok(if self.bool()? { Some(self.str()?) } else { None })
    }
    fn matcher(&mut self) -> io::Result<ValueMatcher> {
        // `ValueMatcher::new` is cheap for exact strings and memoized for
        // regexes (the global RegexCache compiles each source only once).
        match self.u8()? {
            1 => Ok(ValueMatcher::new_lazy(&self.str()?, false)),
            2 => Ok(ValueMatcher::new_lazy(&self.str()?, true)),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, "bad matcher")),
        }
    }
    fn opt_matcher(&mut self) -> io::Result<Option<ValueMatcher>> {
        match self.u8()? {
            0 => Ok(None),
            _ => {
                self.p -= 1;
                Ok(Some(self.matcher()?))
            }
        }
    }
}

fn write_token(w: &mut W, t: &TokenPattern) {
    w.opt_str(&t.text);
    w.opt_matcher(&t.regex);
    w.opt_matcher(&t.postag);
    w.bool(t.postag_regexp);
    w.opt_matcher(&t.lemma);
    w.bool(t.inflected);
    w.bool(t.case_sensitive);
    match t.space_before {
        None => w.u8(0),
        Some(false) => w.u8(1),
        Some(true) => w.u8(2),
    }
    w.bool(t.negate_pos);
    w.u32(t.min);
    w.u32(t.max);
    w.u32(t.skip);
    w.u32(t.exceptions.len() as u32);
    for e in &t.exceptions {
        write_token(w, &e.pattern);
        w.u8(match e.scope {
            Scope::Current => 0,
            Scope::Previous => 1,
            Scope::Next => 2,
        });
    }
}

fn read_token(r: &mut R) -> io::Result<TokenPattern> {
    let mut t = TokenPattern::new();
    t.text = r.opt_str()?;
    t.regex = r.opt_matcher()?;
    t.postag = r.opt_matcher()?;
    t.postag_regexp = r.bool()?;
    t.lemma = r.opt_matcher()?;
    t.inflected = r.bool()?;
    t.case_sensitive = r.bool()?;
    t.space_before = match r.u8()? {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    };
    t.negate_pos = r.bool()?;
    t.min = r.u32()?;
    t.max = r.u32()?;
    t.skip = r.u32()?;
    let n = r.u32()? as usize;
    for _ in 0..n {
        let pattern = Box::new(read_token(r)?);
        let scope = match r.u8()? {
            1 => Scope::Previous,
            2 => Scope::Next,
            _ => Scope::Current,
        };
        t.exceptions.push(Exception { pattern, scope });
    }
    Ok(t)
}

fn write_pattern(w: &mut W, p: &Pattern) {
    w.u32(p.elems.len() as u32);
    for e in &p.elems {
        match e {
            PatternElem::Token(t) => {
                w.u8(0);
                write_token(w, t);
            }
            PatternElem::Or(alts) => {
                w.u8(1);
                w.u32(alts.len() as u32);
                for a in alts {
                    w.u32(a.len() as u32);
                    for x in a {
                        if let PatternElem::Token(t) = x {
                            write_token(w, t);
                        }
                    }
                }
            }
            PatternElem::Seq(seq) => {
                w.u8(2);
                let pat = Pattern { elems: seq.clone() };
                write_pattern(w, &pat);
            }
            PatternElem::And(ts) => {
                // `<and>`: all tokens must match the same span; encoded as a
                // single token whose sub-constraints are checked by sequence.
                w.u8(4);
                w.u32(ts.len() as u32);
                for t in ts {
                    write_token(w, t);
                }
            }
            PatternElem::Unify { inner, negate, features } => {
                w.u8(3);
                w.bool(*negate);
                w.u32(features.len() as u32);
                for f in features {
                    w.str(f);
                }
                let pat = Pattern { elems: inner.clone() };
                write_pattern(w, &pat);
            }
        }
    }
}

fn read_pattern(r: &mut R) -> io::Result<Pattern> {
    let n = r.u32()? as usize;
    let mut elems = Vec::with_capacity(n);
    for _ in 0..n {
        match r.u8()? {
            0 => elems.push(PatternElem::Token(read_token(r)?)),
            1 => {
                let na = r.u32()? as usize;
                let mut alts = Vec::with_capacity(na);
                for _ in 0..na {
                    let nx = r.u32()? as usize;
                    let mut seq = Vec::with_capacity(nx);
                    for _ in 0..nx {
                        seq.push(PatternElem::Token(read_token(r)?));
                    }
                    alts.push(seq);
                }
                elems.push(PatternElem::Or(alts));
            }
            2 => {
                let p = read_pattern(r)?;
                elems.push(PatternElem::Seq(p.elems));
            }
            4 => {
                let nt = r.u32()? as usize;
                let mut ts = Vec::with_capacity(nt);
                for _ in 0..nt {
                    ts.push(read_token(r)?);
                }
                elems.push(PatternElem::And(ts));
            }
            3 => {
                let negate = r.bool()?;
                let nf = r.u32()? as usize;
                let mut features = Vec::with_capacity(nf);
                for _ in 0..nf {
                    features.push(r.str()?);
                }
                let p = read_pattern(r)?;
                elems.push(PatternElem::Unify {
                    inner: p.elems,
                    negate,
                    features,
                });
            }
            _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "bad elem")),
        }
    }
    Ok(Pattern { elems })
}

fn write_parts(w: &mut W, parts: &[Part]) {
    w.u32(parts.len() as u32);
    for p in parts {
        match p {
            Part::Text(t) => {
                w.u8(0);
                w.str(t);
            }
            Part::Ref(r) => {
                w.u8(1);
                w.u32(r.no as u32);
                w.opt_str(&r.postag);
                w.bool(r.postag_regexp);
                w.opt_str(&r.postag_replace);
                w.opt_str(&r.regexp_match);
                w.opt_str(&r.regexp_replace);
                w.opt_str(&r.case_conversion);
                w.bool(r.include_skipped);
            }
        }
    }
}

fn read_parts(r: &mut R) -> io::Result<Vec<Part>> {
    let n = r.u32()? as usize;
    let mut parts = Vec::with_capacity(n);
    for _ in 0..n {
        match r.u8()? {
            0 => parts.push(Part::Text(r.str()?)),
            _ => parts.push(Part::Ref(MatchRef {
                no: r.u32()? as usize,
                postag: r.opt_str()?,
                postag_regexp: r.bool()?,
                postag_replace: r.opt_str()?,
                regexp_match: r.opt_str()?,
                regexp_replace: r.opt_str()?,
                case_conversion: r.opt_str()?,
                include_skipped: r.bool()?,
            })),
        }
    }
    Ok(parts)
}

impl RuleSet {
    /// Serialize to the binary cache format.
    pub fn to_cache(&self) -> Vec<u8> {
        let mut w = W {
            buf: Vec::with_capacity(1 << 20),
        };
        w.buf.extend_from_slice(MAGIC);
        w.u32(self.rules.len() as u32);
        for r in &self.rules {
            w.str(&r.id);
            w.str(&r.full_id);
            w.opt_str(&r.sub_id);
            w.str(&r.name);
            write_parts(&mut w, &r.message);
            w.opt_str(&r.short_message);
            w.u32(r.suggestions.len() as u32);
            for s in &r.suggestions {
                write_parts(&mut w, s);
            }
            write_pattern(&mut w, &r.pattern);
            w.u32(r.antipatterns.len() as u32);
            for ap in &r.antipatterns {
                write_pattern(&mut w, ap);
            }
            w.u8(match r.priority {
                Priority::Low => 1,
                Priority::Medium => 2,
                Priority::High => 3,
            });
            w.str(&r.category_id);
            w.str(&r.category_name);
            w.bool(r.is_style);
            w.bool(r.default_off);
            w.bool(r.suppress_match);
            w.u32(r.tags.len() as u32);
            for t in &r.tags {
                w.str(t);
            }
            w.u32(r.java_filters.len() as u32);
            for f in &r.java_filters {
                w.str(f);
            }
            w.u32(r.filters.len() as u32);
            for f in &r.filters {
                w.str(&f.class);
                w.str(&f.args.to_args_string());
            }
            w.opt_str(&r.unsupported);
            w.u32(r.urls.len() as u32);
            for u in &r.urls {
                w.str(u);
            }
            w.u32(r.examples.len() as u32);
            for e in &r.examples {
                w.str(&e.text);
                w.u32(e.corrections.len() as u32);
                for c in &e.corrections {
                    w.str(c);
                }
                w.str(&e.kind);
            }
        }
        w.buf
    }

    /// Load from the binary cache format.
    pub fn from_cache(bytes: &[u8]) -> io::Result<RuleSet> {
        if bytes.len() < 12 || &bytes[..8] != MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad rule cache"));
        }
        let mut r = R { b: bytes, p: 8 };
        let n = r.u32()? as usize;
        let mut set = RuleSet::default();
        set.rules.reserve(n);
        for _ in 0..n {
            let id = r.str()?;
            let full_id = r.str()?;
            let sub_id = r.opt_str()?;
            let name = r.str()?;
            let message = read_parts(&mut r)?;
            let short_message = r.opt_str()?;
            let ns = r.u32()? as usize;
            let mut suggestions = Vec::with_capacity(ns);
            for _ in 0..ns {
                suggestions.push(read_parts(&mut r)?);
            }
            let pattern = read_pattern(&mut r)?;
            let nap = r.u32()? as usize;
            let mut antipatterns = Vec::with_capacity(nap);
            for _ in 0..nap {
                antipatterns.push(read_pattern(&mut r)?);
            }
            let priority = match r.u8()? {
                1 => Priority::Low,
                3 => Priority::High,
                _ => Priority::Medium,
            };
            let category_id = r.str()?;
            let category_name = r.str()?;
            let is_style = r.bool()?;
            let default_off = r.bool()?;
            let suppress_match = r.bool()?;
            let nt = r.u32()? as usize;
            let mut tags = Vec::with_capacity(nt);
            for _ in 0..nt {
                tags.push(r.str()?);
            }
            let nf = r.u32()? as usize;
            let mut java_filters = Vec::with_capacity(nf);
            for _ in 0..nf {
                java_filters.push(r.str()?);
            }
            let nfi = r.u32()? as usize;
            let mut filters = Vec::with_capacity(nfi);
            for _ in 0..nfi {
                let class = r.str()?;
                let args = crate::filters::FilterArgs::parse(&r.str()?);
                filters.push(crate::filters::RuleFilter { class, args });
            }
            let unsupported = r.opt_str()?;
            let nu = r.u32()? as usize;
            let mut urls = Vec::with_capacity(nu);
            for _ in 0..nu {
                urls.push(r.str()?);
            }
            let ne = r.u32()? as usize;
            let mut examples = Vec::with_capacity(ne);
            for _ in 0..ne {
                let text = r.str()?;
                let nc = r.u32()? as usize;
                let mut corrections = Vec::with_capacity(nc);
                for _ in 0..nc {
                    corrections.push(r.str()?);
                }
                let kind = r.str()?;
                examples.push(Example {
                    text,
                    corrections,
                    kind,
                });
            }
            set.rules.push(Rule {
                id,
                full_id,
                sub_id,
                name,
                message,
                short_message,
                suggestions,
                pattern,
                antipatterns,
                priority,
                category_id,
                category_name,
                is_style,
                default_off,
                suppress_match,
                tags,
                java_filters,
                filters,
                unsupported,
                urls,
                examples,
            });
        }
        set.build_index();
        Ok(set)
    }

    /// Save the cache to `path`.
    pub fn save_cache<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let tmp = path.as_ref().with_extension("tmp");
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&self.to_cache())?;
        f.flush()?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Load a cache file (or `None` if it does not exist / is invalid).
    pub fn load_cache<P: AsRef<Path>>(path: P) -> Option<RuleSet> {
        let mut f = fs::File::open(path).ok()?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).ok()?;
        RuleSet::from_cache(&buf).ok()
    }
}
