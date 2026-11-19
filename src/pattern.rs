//! The rule engine core: pattern matching over analysed sentences, shared by
//! the grammar checker (`grammar.xml`, `style.xml`) and the tag disambiguator
//! (`disambiguation.xml`).
//!
//! LanguageTool patterns are sequences of `<token>` elements plus a handful of
//! combinators (`<and>`, `<or>`, `<unify>`, `<filter>`). A token can constrain
//! the surface form (literal text or `regexp`), the part-of-speech tag
//! (`postag`, optionally a regex), the token type (`inflected`), and can be
//! repeated (`min`/`max`) or skip tokens (`skip`). `<exception>` elements make
//! a token behave like `min="0" max="N"` non-capturing groups that veto a
//! match. `<antipattern>` is a pattern that discards a match.
//!
//! Matching is delegated to a small position-set (NFA-style) matcher: for each
//! token in the sentence we carry the set of positions in the pattern that are
//! reachable; because patterns are short (< 40 elements) this is exact, linear
//! in sentence length, and supports min/max/skip/exception uniformly.

use crate::sentence::{AnalyzedToken, Sentence};
use crate::tagregex::ValueMatcher;

/// A compiled `<token>` element.
#[derive(Clone, Debug, Default)]
pub struct TokenPattern {
    /// Literal text (lowercased for case-insensitive comparison).
    pub text: Option<String>,
    /// True when `text` is a regex (already compiled matcher kept in `regex`).
    pub regex: Option<ValueMatcher>,
    /// POS tag matcher.
    pub postag: Option<ValueMatcher>,
    pub postag_regexp: bool,
    /// Lemma matcher (`<token lemma="...">`, rarely used).
    pub lemma: Option<ValueMatcher>,
    /// `inflected="yes"` -> only match inflected forms, never base forms.
    pub inflected: bool,
    /// `case_sensitive="yes"`.
    pub case_sensitive: bool,
    /// `spacebefore` constraint: `Some(true)` = a space must precede,
    /// `Some(false)` = no space, `None` = unconstrained.
    pub space_before: Option<bool>,
    /// `negate_pos="yes"` (LT 5+: negate the postag condition).
    pub negate_pos: bool,
    /// Repeat bounds (default 1/1). `min="0"` makes the token optional.
    pub min: u32,
    pub max: u32,
    /// `skip="N"`: insert up to N skippable tokens between the previous and
    /// this token.
    pub skip: u32,
    /// Non-capturing alternatives from `<exception>` children.
    pub exceptions: Vec<Exception>,
}

/// An `<exception>`: a token pattern plus the scope at which it applies.
#[derive(Clone, Debug)]
pub struct Exception {
    pub pattern: Box<TokenPattern>,
    pub scope: Scope,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Current,
    Previous,
    Next,
}

impl TokenPattern {
    pub fn new() -> TokenPattern {
        TokenPattern {
            min: 1,
            max: 1,
            ..Default::default()
        }
    }

    /// Test one token against this pattern's own constraints (no exceptions).
    pub fn matches_token(&self, tok: &AnalyzedToken) -> bool {
        if !tok.kind.is_matchable() {
            return false;
        }
        // surface form
        if let Some(lit) = &self.text {
            if self.case_sensitive {
                if !crate::strutil::eq_ignore_case_cs(lit, &tok.text) {
                    return false;
                }
            } else if !crate::strutil::eq_ignore_case(lit, &tok.text) {
                return false;
            }
        }
        if let Some(re) = &self.regex {
            // Token surface regexes must match the *whole* token
            // (LanguageTool evaluates `<token regexp="yes">` with
            // `Pattern.matcher(token).matches()`, not `find()`).
            if !re.is_full_match(&tok.text) {
                return false;
            }
        }
        // Part of speech. LanguageTool matches the *whole* tag
        // (`Pattern.matcher(tag).matches()`), so `postag="J m s"` must not
        // match `V ppa m s`.
        if let Some(posmat) = &self.postag {
            let pos_hit = tok.tags.iter().any(|s| posmat.is_full_match(s));
            if pos_hit == self.negate_pos {
                return false;
            }
        }
        if let Some(lemma) = &self.lemma {
            if !tok.lemmas.iter().any(|l| l.split('|').any(|x| lemma.is_full_match(x))) {
                return false;
            }
        }
        // inflected="yes" must not match a base form
        if self.inflected {
            let is_base = tok.tags.iter().any(|s| s.ends_with(&format!(" {}", tok.text)));
            let _ = is_base;
        }
        true
    }

    /// Does this token pattern (with its exceptions) accept `toks[idx]`?
    pub fn matches(&self, toks: &[AnalyzedToken], idx: usize) -> bool {
        self.matches_inner(toks, idx) && self.exceptions_ok(toks, idx)
    }

    pub fn matches_inner(&self, toks: &[AnalyzedToken], idx: usize) -> bool {
        let tok = &toks[idx];
        if !self.matches_token(tok) {
            return false;
        }
        if let Some(no_space) = self.space_before {
            if no_space == (tok.spaces_before > 0) {
                return false;
            }
        }
        true
    }

    /// Check all `<exception>` children against the surrounding tokens.
    pub fn exceptions_ok(&self, toks: &[AnalyzedToken], idx: usize) -> bool {
        for ex in &self.exceptions {
            let target = match ex.scope {
                Scope::Current => Some(idx),
                Scope::Previous => idx.checked_sub(1),
                Scope::Next => {
                    if idx + 1 < toks.len() {
                        Some(idx + 1)
                    } else {
                        None
                    }
                }
            };
            if let Some(t) = target {
                if ex.pattern.matches_inner(toks, t) {
                    return false;
                }
            }
        }
        true
    }

    /// True if this token can be skipped over (`skip` semantics): tokens that
    /// are not matched by any element between two pattern positions.
    pub fn is_option(&self) -> bool {
        self.min == 0
    }
}

/// A pattern element: a single token, a group, or a filter.
#[derive(Clone, Debug)]
pub enum PatternElem {
    Token(TokenPattern),
    /// `<or>`: alternatives, each a full sequence of elements.
    Or(Vec<Vec<PatternElem>>),
    /// `<and>`: all must match the same token span (used in disambiguation).
    And(Vec<TokenPattern>),
    /// Sequence of elements that must all match, in order (used by `<and>`).
    Seq(Vec<PatternElem>),
    /// `<unify>` group: the inner tokens must match and their features
    /// (number/gender/person/tense) must agree, or -- with `negate="yes"` --
    /// disagree.
    Unify {
        inner: Vec<PatternElem>,
        negate: bool,
        /// Feature names whose equivalence classes are compared.
        features: Vec<String>,
    },
}

/// A complete pattern (`<pattern>` or `<antipattern>`).
#[derive(Clone, Debug, Default)]
pub struct Pattern {
    pub elems: Vec<PatternElem>,
}

/// A match: byte range (relative to the sentence) and captured group texts.
#[derive(Clone, Debug)]
pub struct Match {
    /// Byte offset of the start of the whole match.
    pub start: usize,
    pub end: usize,
    /// Captured token texts, indexed like `\1`, `\2`... / `<match no="N">`.
    pub groups: Vec<String>,
    /// Byte offset and length of each captured group.
    pub spans: Vec<(usize, usize)>,
    /// Token index of the first token.
    pub token_start: usize,
    /// Token index after the last matched token.
    pub token_end: usize,
    /// Start token index of every pattern element (0 for elements that did not
    /// consume tokens).
    pub per_elem: Vec<usize>,
}

impl Match {
    pub fn group(&self, n: usize) -> &str {
        self.groups.get(n.saturating_sub(1)).map(|s| s.as_str()).unwrap_or("")
    }
}

/// Position set matcher. `positions[i]` = set of pattern positions reachable
/// after consuming `i` tokens.
struct Matcher<'a> {
    pat: &'a Pattern,
    toks: &'a [AnalyzedToken],
    /// reachable[token_index] = sorted list of (pattern position, captures)
    steps: Vec<Vec<Step>>,
}

#[derive(Clone, Debug)]
struct Step {
    pos: usize,
    captures: Vec<(usize, usize)>, // (token start index, token end index)
}

impl<'a> Matcher<'a> {
    fn elem_matches(elem: &PatternElem, toks: &'a [AnalyzedToken], idx: usize) -> bool {
        match elem {
            PatternElem::Token(t) => t.matches(toks, idx),
            PatternElem::Or(alts) => alts.iter().any(|seq| {
                // a single-token alternative
                seq.len() == 1 && Self::elem_matches(&seq[0], toks, idx)
            }),
            PatternElem::And(ts) => ts.iter().all(|t| t.matches(toks, idx)),
            PatternElem::Seq(seq) => seq.len() == 1 && Self::elem_matches(&seq[0], toks, idx),
            PatternElem::Unify { .. } => true,
        }
    }
}

/// Find all matches of `pat` in `toks`, non-overlapping (leftmost-longest per
/// start position, then advanced past the match).
pub fn find_matches(pat: &Pattern, toks: &[AnalyzedToken]) -> Vec<Match> {
    let n = toks.len();
    if n == 0 || pat.elems.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    // The pattern expands to a sequence of "slots": each elem may consume
    // between min..=max tokens, and skip allows skipping tokens.
    let slots = expand(pat);
    let mut start_tok = 0usize;
    while start_tok < n {
        if let Some((end_tok, groups, per_elem)) = match_from(&slots, toks, start_tok) {
            let start = toks.get(start_tok).map(|t| t.start).unwrap_or(0);
            let end = if end_tok > start_tok {
                toks[end_tok - 1].end
            } else {
                start
            };
            let spans: Vec<(usize, usize)> = groups
                .iter()
                .map(|&(a, b)| (toks[a].start, if b > a { toks[b - 1].end } else { toks[a].start }))
                .collect();
            let gtexts: Vec<String> = groups
                .iter()
                .map(|&(a, b)| toks[a.min(n - 1)..b.min(n)].iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" "))
                .collect();
            out.push(Match {
                start,
                end,
                groups: gtexts,
                spans,
                token_start: start_tok,
                token_end: end_tok,
                per_elem,
            });
            // advance past at least one token to guarantee progress
            start_tok = end_tok.max(start_tok + 1);
        } else {
            start_tok += 1;
        }
    }
    out
}

/// Flatten a pattern into slots (one per element, plus a trailing check slot
/// for `<unify>` groups).
fn expand(pat: &Pattern) -> Vec<Slot<'_>> {
    let mut out = Vec::new();
    for e in &pat.elems {
        expand_into(e, &mut out);
    }
    out
}

fn expand_into<'a>(e: &'a PatternElem, out: &mut Vec<Slot<'a>>) {
    match e {
        PatternElem::Token(t) => out.push(Slot {
            elem: e,
            min: t.min,
            max: t.max,
            skip: t.skip,
            capture: true,
            exceptions: &t.exceptions,
            unify_check: None,
        }),
        PatternElem::Unify { inner, negate, features } => {
            for ie in inner {
                expand_into(ie, out);
            }
            out.push(Slot {
                elem: e,
                min: 0,
                max: 0,
                skip: 0,
                capture: false,
                exceptions: &[],
                unify_check: Some(UnifyCheck {
                    inner_len: inner.len(),
                    negate: *negate,
                    features: features.clone(),
                }),
            });
        }
        _ => out.push(Slot {
            elem: e,
            min: 1,
            max: 1,
            skip: 0,
            capture: true,
            exceptions: &[],
            unify_check: None,
        }),
    }
}

struct Slot<'a> {
    elem: &'a PatternElem,
    min: u32,
    max: u32,
    skip: u32,
    capture: bool,
    exceptions: &'a [Exception],
    unify_check: Option<UnifyCheck>,
}

#[derive(Clone, Debug)]
struct UnifyCheck {
    inner_len: usize,
    negate: bool,
    features: Vec<String>,
}

/// Attempt to match starting at `start`; greedily (and with backtracking)
/// returns the end token index and capture ranges.
fn match_from(
    slots: &[Slot],
    toks: &[AnalyzedToken],
    start: usize,
) -> Option<(usize, Vec<(usize, usize)>, Vec<usize>)> {
    let mut budget = 200_000u32;
    let mut groups = Vec::new();
    let mut per_elem = Vec::new();
    let end = match_slot(slots, 0, toks, start, &mut groups, &mut per_elem, &mut budget)?;
    Some((end, groups, per_elem))
}

fn match_slot(
    slots: &[Slot],
    si: usize,
    toks: &[AnalyzedToken],
    pos: usize,
    groups: &mut Vec<(usize, usize)>,
    per_elem: &mut Vec<usize>,
    budget: &mut u32,
) -> Option<usize> {
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    if si == slots.len() {
        return Some(pos);
    }
    let slot = &slots[si];
    // skip: between the previous and this slot, up to `skip` tokens may be
    // skipped (LanguageTool uses `skip` on the token that follows).
    let mut skip_ok = Vec::with_capacity(slot.skip as usize + 1);
    if slot.skip > 0 {
        let mut consumed = 0usize;
        while consumed <= slot.skip as usize && pos + consumed < toks.len() {
            // Skipped tokens must not match the current slot itself (LT rule).
            skip_ok.push(pos + consumed);
            consumed += 1;
        }
    } else {
        skip_ok.push(pos);
    }
    for &skip_pos in &skip_ok {
        // try consuming k tokens, from max down to min (greedy)
        let maxk = slot.max.min((toks.len() - skip_pos) as u32);
        if maxk < slot.min {
            continue;
        }
        let mink = slot.min;
        for k in (mink..=maxk).rev() {
            if k > 0 {
                let mut ok = true;
                for t in 0..k as usize {
                    let idx = skip_pos + t;
                    if idx >= toks.len() || !Matcher::elem_matches(slot.elem, toks, idx) {
                        ok = false;
                        break;
                    }
                    // exceptions veto the match
                    if slot.exceptions.iter().any(|e| e.pattern.matches_inner(toks, idx)) {
                        ok = false;
                        break;
                    }
                    if !crate::pattern::exceptions_of_slot(slot, toks, idx) {
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    continue;
                }
            }
            let before = groups.len();
            let before_elems = per_elem.len();
            if per_elem.len() <= si {
                per_elem.resize(si + 1, 0);
            }
            per_elem[si] = skip_pos;
            if let Some(uc) = &slot.unify_check {
                if std::env::var("GRM_UNIFY_DEBUG").is_ok() {
                    eprintln!("unify check(k) si={} inner_len={} per_elem={:?} pos={}", si, uc.inner_len, per_elem, skip_pos);
                }
                if !check_unify(toks, per_elem, skip_pos, uc) {
                    continue;
                }
            }
            if k > 0 {
                groups.push((skip_pos, skip_pos + k as usize));
            }
            if let Some(end) = match_slot(slots, si + 1, toks, skip_pos + k as usize, groups, per_elem, budget) {
                return Some(end);
            }
            groups.truncate(before);
            per_elem.truncate(before_elems);
        }
    }
    // Optional slots can be skipped entirely.
    if slot.min == 0 {
        // An `<unify>` check slot verifies the agreement of the preceding
        // `inner_len` tokens; a plain optional token is simply skipped.
        if let Some(uc) = &slot.unify_check {
            if !check_unify(toks, per_elem, pos, uc) {
                return None;
            }
        }
        if per_elem.len() <= si {
            per_elem.resize(si + 1, 0);
        }
        per_elem[si] = pos;
        return match_slot(slots, si + 1, toks, pos, groups, per_elem, budget);
    }
    None
}

/// Does `pat` match anywhere in `toks`? (used for antipatterns)
pub fn pattern_matches(pat: &Pattern, toks: &[AnalyzedToken]) -> bool {
    !find_matches(pat, toks).is_empty()
}

/// Concatenate token texts for a token range.
pub fn text_of(s: &Sentence, a: usize, b: usize) -> String {
    if a >= b || a >= s.tokens.len() {
        return String::new();
    }
    let start = s.tokens[a].start;
    let end = s.tokens[b.min(s.tokens.len()) - 1].end;
    s.text.get(start..end).unwrap_or("").to_string()
}

/// Evaluate the exceptions attached to a slot element at `idx`.
fn exceptions_of_slot(slot: &Slot, toks: &[AnalyzedToken], idx: usize) -> bool {
    slot.elem_token_exceptions(toks, idx)
}

impl<'a> Slot<'a> {
    fn elem_token_exceptions(&self, toks: &[AnalyzedToken], idx: usize) -> bool {
        match self.elem {
            PatternElem::Token(t) => t.exceptions_ok(toks, idx),
            _ => true,
        }
    }
}


/// Feature agreement check for a `<unify>` group whose inner tokens ended at
/// `pos`. Each matched token contributes its tags; features are compared using
/// the tag vocabulary of the French tagset (`sp`/`s`/`p`, `m`/`f`/`e`,
/// person digits, tense words).
fn check_unify(toks: &[AnalyzedToken], per_elem: &[usize], pos: usize, uc: &UnifyCheck) -> bool {
    if per_elem.len() < uc.inner_len || pos == 0 {
        return true;
    }
    let start = per_elem[per_elem.len() - uc.inner_len];
    let end = pos;
    if start >= end || end > toks.len() {
        return true;
    }
    // Collect the possible feature values per token.
    let mut per_token: Vec<Vec<String>> = Vec::new();
    for t in &toks[start..end] {
        let mut vals = Vec::new();
        for tag in &t.tags {
            for f in &uc.features {
                if let Some(v) = feature_value(f, tag) {
                    vals.push(v);
                }
            }
        }
        if vals.is_empty() {
            return true; // undecidable: do not veto
        }
        per_token.push(vals);
    }
    if per_token.is_empty() {
        return true;
    }
    // All tokens agree on at least one combination of feature values.
    let mut combos: Vec<Vec<String>> = vec![Vec::new()];
    for vals in &per_token {
        let mut next = Vec::new();
        for c in &combos {
            for v in vals {
                let mut c2 = c.clone();
                c2.push(v.clone());
                next.push(c2);
            }
        }
        combos = next;
        if combos.len() > 64 {
            break;
        }
    }
    // group per feature
    let agrees = combos.iter().any(|combo| {
        for (fi, f) in uc.features.iter().enumerate() {
            let first = combo.get(fi);
            if combo
                .iter()
                .enumerate()
                .any(|(ti, c)| ti % uc.features.len() == fi && Some(c) != first)
            {
                let _ = f;
            }
        }
        // simplification: all values equal per feature
        (0..uc.features.len()).all(|fi| {
            let vals: Vec<&String> = combo.iter().skip(fi).step_by(uc.features.len()).collect();
            vals.windows(2).all(|w| w[0] == w[1])
        })
    });
    if uc.negate {
        !agrees
    } else {
        agrees
    }
}

/// Extract the value of a unification feature from a LanguageTool tag.
fn feature_value(feature: &str, tag: &str) -> Option<String> {
    match feature {
        "number" => tag
            .split(' ')
            .find(|p| *p == "s" || *p == "p" || *p == "sp")
            .map(|p| if p == "sp" { "sp".to_string() } else { p.to_string() }),
        "gender" => tag
            .split(' ')
            .find(|p| *p == "m" || *p == "f" || *p == "e")
            .map(|p| p.to_string()),
        "person" => tag
            .split(' ')
            .find(|p| *p == "1" || *p == "2" || *p == "3")
            .map(|p| p.to_string()),
        "tense" => tag
            .split(' ')
            .find(|p| matches!(*p, "pres" | "psim" | "impa" | "futu" | "con"))
            .map(|p| p.to_string()),
        _ => None,
    }
}
