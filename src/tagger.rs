//! French tagger and disambiguator.
//!
//! Pipeline (mirrors LanguageTool's `FrenchHybridDisambiguator`):
//!
//! 1. tokenize the sentence (`tokenizer`),
//! 2. dictionary lookup for each token, with case-insensitive fallback and a
//!    set of spelling heuristics (hyphenated words, elisions, unknown words),
//! 3. multiword chunking (`multiwords.txt`): e.g. `c' est` becomes one token
//!    tagged `V etre inf`,
//! 4. rule-based disambiguation (`disambiguation.xml`): removes, filters or
//!    replaces the tags of ambiguous words using context patterns.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::lexicon::Lexicon;
use crate::pattern::{find_matches, Pattern, PatternElem, TokenPattern};
use crate::sentence::{AnalyzedToken, Sentence};
use crate::strutil;
use crate::tagregex::ValueMatcher;
use crate::tokenizer::{self, TokenType};
use crate::xml::{self, DomItem, Node as XmlNode};

pub struct Tagger {
    pub lexicon: &'static Lexicon,
    /// Multiword chunks: lowercase form -> (words, tags).
    chunks: HashMap<String, (Vec<String>, String)>,
    pub disambig: Disambiguation,
}

static LEXICON: OnceLock<Lexicon> = OnceLock::new();
static WORDS: OnceLock<WordLists> = OnceLock::new();

/// Auxiliary word lists (multiwords, hyphenated compounds, extra spelling).
#[derive(Default)]
pub struct WordLists {
    pub multiwords: Vec<(Vec<String>, String)>,
    pub hyphenated: std::collections::HashSet<String>,
    pub spelling: std::collections::HashSet<String>,
    pub forbidden: std::collections::HashSet<String>,
}

impl WordLists {
    pub fn load(text: &str) -> WordLists {
        let mut w = WordLists::default();
        let mut section = "";
        for line in text.lines() {
            if let Some(name) = line.strip_prefix("#!") {
                section = name.trim();
                continue;
            }
            if line.trim().is_empty() {
                continue;
            }
            match section {
                "multiwords" => {
                    // `form; tags` (the form may contain spaces)
                    if let Some((form, tags)) = line.rsplit_once(';') {
                        w.multiwords.push((form.split_whitespace().map(|s| s.to_string()).collect(), tags.trim().to_string()));
                    }
                }
                "hyphenated" => {
                    w.hyphenated.insert(line.to_string());
                }
                "spelling" => {
                    w.spelling.insert(line.to_string());
                }
                "forbidden" => {
                    w.forbidden.insert(line.to_string());
                }
                _ => {}
            }
        }
        w
    }
}

impl Tagger {
    /// Load the tagger from binary assets. The lexicon is loaded once and
    /// cached for the process lifetime.
    pub fn new(lex_path: &std::path::Path, words_path: &std::path::Path, disambig_path: Option<&std::path::Path>) -> std::io::Result<&'static Tagger> {
        static TAGGER: OnceLock<Tagger> = OnceLock::new();
        if let Some(t) = TAGGER.get() {
            return Ok(t);
        }
        let lex = match LEXICON.get() {
            Some(l) => l,
            None => {
                let l = Lexicon::load(lex_path)?;
                let _ = LEXICON.set(l);
                LEXICON.get().unwrap()
            }
        };
        let words = match WORDS.get() {
            Some(w) => w,
            None => {
                let text = std::fs::read_to_string(words_path).unwrap_or_default();
                let _ = WORDS.set(WordLists::load(&text));
                WORDS.get().unwrap()
            }
        };
        let disambig = match disambig_path {
            Some(p) => match std::fs::read_to_string(p) {
                Ok(xml) => Disambiguation::parse(&xml),
                Err(_) => Disambiguation::default(),
            },
            None => Disambiguation::default(),
        };
        let mut chunks: HashMap<String, (Vec<String>, String)> = HashMap::new();
        for (words, tags) in &words.multiwords {
            if words.is_empty() {
                continue;
            }
            let key = words.join(" ").to_lowercase();
            chunks.entry(key).or_insert_with(|| (words.clone(), tags.clone()));
        }
        crate::lexicon::set_global_lexicon(lex);
        let t = Tagger {
            lexicon: lex,
            chunks,
            disambig,
        };
        let _ = TAGGER.set(t);
        Ok(TAGGER.get().unwrap())
    }

    /// Tag a sentence: tokenize + lookup + chunk + disambiguate.
    ///
    /// Like LanguageTool, the analysed token list is framed by a `SENT_START`
    /// token and a `SENT_END` token so that rules can anchor patterns at
    /// sentence boundaries (805 French rules use `SENT_START`).
    pub fn tag(&self, text: &str, offset: usize) -> Sentence {
        let mut s = Sentence::new(text, offset);
        let first_start = text.len() - text.trim_start().len();
        let mut start_tok = AnalyzedToken::new_for_test("");
        start_tok.start = first_start;
        start_tok.end = first_start;
        start_tok.kind = crate::tokenizer::TokenType::SentenceStart;
        start_tok.tags.push("SENT_START".to_string());
        start_tok.lemmas.push("".to_string());
        s.tokens.push(start_tok);
        s.tokens.extend(tokenizer::tokenize(text).iter().map(AnalyzedToken::from_token));
        // dictionary lookup for every word (the structural token is skipped)
        self.lookup_all(&mut s);
        self.chunk(&mut s);
        self.disambiguate(&mut s);
        self.add_unknown_markers(&mut s);
        // SENT_END
        let mut end_tok = AnalyzedToken::new_for_test("");
        end_tok.start = text.len();
        end_tok.end = text.len();
        end_tok.kind = crate::tokenizer::TokenType::SentenceEnd;
        end_tok.tags.push("SENT_END".to_string());
        end_tok.lemmas.push("".to_string());
        s.tokens.push(end_tok);
        s
    }

    pub fn lookup_all_for_test(&self, s: &mut Sentence) { self.lookup_all(s) }

    fn lookup_all(&self, s: &mut Sentence) {
        for i in 0..s.tokens.len() {
            if s.tokens[i].kind == TokenType::SentenceStart {
                continue;
            }
            let text = s.tokens[i].text.clone();
            self.lookup_token(&mut s.tokens[i], &text);
        }
    }

    /// Dictionary lookup for a single token, with LanguageTool-style fallbacks.
    pub fn lookup_token(&self, tok: &mut AnalyzedToken, text: &str) {
        let lex = self.lexicon;
        let mut morphs: Vec<(&str, &str)> = Vec::new();

        // 1. exact match; for capitalised words also merge the lowercase
        //    readings, as LanguageTool does (`Mes` -> `Me/T e p` + `mon/D e p`).
        lex.morphs(text, &mut morphs);
        let exact: Vec<(String, String)> = morphs
            .iter()
            .map(|(l, t)| (l.to_string(), t.to_string()))
            .collect();
        if !exact.is_empty() {
            for (l, t) in &exact {
                tok.add(l, t);
            }
            let lower = strutil::lower(text);
            if lower != text {
                lex.morphs(&lower, &mut morphs);
                let lower_readings: Vec<(String, String)> = morphs
                    .iter()
                    .map(|(l, t)| (l.to_string(), t.to_string()))
                    .collect();
                for (l, t) in &lower_readings {
                    tok.add(l, t);
                }
            }
            self.add_case_extra(tok, text);
            return;
        }
        // 2. lowercase-only match (sentence-initial capitalisation)
        let lower = strutil::lower(text);
        if lower != text {
            lex.morphs(&lower, &mut morphs);
            if !morphs.is_empty() {
                for (l, t) in &morphs {
                    tok.add(l, t);
                }
                self.add_case_extra(tok, text);
                return;
            }
        }
        // 3. hyphenated compounds: try each part, then whole-word chunk
        if text.contains('-') {
            lex.morphs(&lower, &mut morphs);
            if !morphs.is_empty() {
                for (l, t) in &morphs {
                    tok.add(l, t);
                }
                return;
            }
        }
        // 4. elided form: `l'`, `d'`, `j'`, `qu'` are in the dictionary already;
        //    otherwise strip the apostrophe
        if let Some(stem) = text.strip_suffix('\'') {
            let cand = if stem == "qu" || stem == "l" || stem == "d" || stem == "j" || stem == "m" || stem == "t" || stem == "s" || stem == "n" || stem == "c" {
                text.to_string()
            } else {
                format!("{}'", stem)
            };
            lex.morphs(&cand, &mut morphs);
            if !morphs.is_empty() {
                for (l, t) in &morphs {
                    tok.add(l, t);
                }
                return;
            }
        }
        // 5. plural of hyphenated / unknown words via the auxiliary lists
        let wl = WORDS.get();
        if let Some(wl) = wl {
            if wl.spelling.contains(text) {
                if is_capitalized(text) && tok.kind != TokenType::Punctuation {
                    tok.add(text, "Z e sp");
                } else {
                    tok.add(&lower, "N e sp");
                }
                return;
            }
            if wl.hyphenated.contains(&lower) {
                tok.add(&lower, "N e sp");
                return;
            }
        }
        // 6. numbers
        if text.chars().all(|c| c.is_ascii_digit() || matches!(c, ' ' | ',' | '.' | '\u{A0}' | '\u{202F}')) && text.chars().any(|c| c.is_ascii_digit()) {
            tok.add(text, "Y");
            return;
        }
        // 7. unknown
        tok.add(text, "UNKNOWN");
    }

    /// Render a sentence in LanguageTool's `--verbose` tagger format:
    /// `<S> word[lemma/tag,...] ... .[./M fin]`
    pub fn verbose(&self, s: &Sentence) -> String {
        let mut out = String::from("<S>");
        for t in &s.tokens {
            if t.kind == TokenType::SentenceStart || t.kind == TokenType::SentenceEnd {
                continue;
            }
            out.push(' ');
            out.push_str(&t.text);
            out.push('[');
            let parts: Vec<String> = t
                .lemmas
                .iter()
                .zip(t.tags.iter())
                .map(|(l, tag)| format!("{}/{}", l.split('|').next().unwrap_or(""), tag))
                .collect();
            out.push_str(&parts.join(","));
            out.push(']');
        }
        out.push(' ');
        out
    }

    fn add_case_extra(&self, tok: &mut AnalyzedToken, text: &str) {
        // LanguageTool marks proper-noun-ish capitalised unknowns; the
        // dictionary already contains proper names, so nothing to do here.
        let _ = (tok, text);
    }

    fn add_unknown_markers(&self, s: &mut Sentence) {
        for t in &mut s.tokens {
            if t.kind == TokenType::SentenceStart || t.kind == TokenType::SentenceEnd {
                continue;
            }
            if t.tags.is_empty() {
                t.add(&t.text.clone(), "UNKNOWN");
            }
        }
    }

    /// Multiword chunker: merge tokens listed in `multiwords.txt`.
    fn chunk(&self, s: &mut Sentence) {
        if s.tokens.is_empty() || self.chunks.is_empty() {
            return;
        }
        let mut i = 0;
        let mut out: Vec<AnalyzedToken> = Vec::with_capacity(s.tokens.len());
        while i < s.tokens.len() {
            let mut matched = false;
            // try chunks of decreasing length (max 5 words)
            let maxlen = 5.min(s.tokens.len() - i);
            for len in (2..=maxlen).rev() {
                let key: String = s.tokens[i..i + len]
                    .iter()
                    .map(|t| strutil::lower(t.text.trim_end_matches('\'')))
                    .collect::<Vec<_>>()
                    .join(" ");
                // the joined form as written in multiwords.txt: "c' est"
                let key2: String = s.tokens[i..i + len]
                    .iter()
                    .map(|t| strutil::lower(&t.text))
                    .collect::<Vec<_>>()
                    .join(" ");
                if let Some((words, tags)) = self.chunks.get(&key).or_else(|| self.chunks.get(&key2)) {
                    let _ = words;
                    let start = s.tokens[i].start;
                    let end = s.tokens[i + len - 1].end;
                    let text: String = s.text.get(start..end).unwrap_or("").to_string();
                    let mut t = AnalyzedToken {
                        text,
                        start,
                        end,
                        kind: TokenType::HyphenatedWord,
                        tags: Vec::new(),
                        lemmas: Vec::new(),
                        spaces_before: s.tokens[i].spaces_before,
                        chunked: true,
                    };
                    // `c' est` -> lemma `c'` + tag from the file
                    let lemma = strutil::lower(&s.tokens[i].text);
                    t.add(&lemma, tags);
                    out.push(t);
                    i += len;
                    matched = true;
                    break;
                }
            }
            if !matched {
                out.push(s.tokens[i].clone());
                i += 1;
            }
        }
        s.tokens = out;
    }

    fn disambiguate(&self, s: &mut Sentence) {
        self.disambig.apply(s);
    }
}

/// Remove the `etre`/`avoir` auxiliary name from a verb tag so that
/// `V etre ppa e sp` also satisfies a `V ppa .*` filter.
fn strip_aux(tag: &str) -> String {
    let mut parts: Vec<&str> = tag.split(' ').collect();
    if parts.len() > 2 && parts[0] == "V" && (parts[1] == "etre" || parts[1] == "avoir") {
        parts.remove(1);
    }
    parts.join(" ")
}

fn is_capitalized(text: &str) -> bool {
    strutil::starts_upper(text)
}

// ---------------------------------------------------------------------------
// Disambiguation rules (disambiguation.xml)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisambigAction {
    Filter,
    FilterAll,
    Remove,
    Add,
    Replace,
    Immunize,
}

#[derive(Clone, Debug)]
pub struct DisambigRule {
    pub id: String,
    pub pattern: Pattern,
    pub antipatterns: Vec<Pattern>,
    /// True when the pattern contains a `<marker>` group. Disambiguation
    /// actions then only apply to the marked tokens.
    pub has_marker: bool,
    /// Number of pattern elements that precede the marked group.
    pub marker_start: usize,
    /// Number of elements inside the marked group.
    pub marker_len: usize,
    pub action: DisambigAction,
    /// Tag the action applies to (postag attribute).
    pub postag: String,
    pub is_regexp: bool,
    /// `(lemma, tag)` pairs added by `add`/`replace`.
    pub add_tags: Vec<(String, String)>,
    /// `<match no="N" postag="..."/>`: the action applies to the tag that
    /// group N carries (when `postag` is empty).
    pub match_group: Option<usize>,
    pub match_postag: Option<String>,
}

#[derive(Default, Clone, Debug)]
pub struct Disambiguation {
    pub rules: Vec<DisambigRule>,
}

impl Disambiguation {
    /// Parse `disambiguation.xml`.
    pub fn parse(xml: &str) -> Disambiguation {
        let doc = xml::parse_dom(xml);
        let mut d = Disambiguation::default();
        // The root node is the `<rules>` element (the document wrapper is only
        // there for comments/prolog).
        let root: &XmlNode = if let Some(r) = doc.first_child("rules") {
            r
        } else if doc.name == "rules" {
            &doc
        } else {
            return d;
        };
        for el in &root.children {
            let DomItem::Node(node) = el else { continue };
            match node.name.as_str() {
                "rule" => d.rules.push(convert_disambig_rule(node)),
                "rulegroup" => {
                    for sub in node.children_named("rule") {
                        d.rules.push(convert_disambig_rule(sub));
                    }
                }
                _ => {}
            }
        }
        d
    }

    /// Apply all rules to a sentence. Rules run in document order; each finds
    /// every non-overlapping match in the sentence.
    pub fn apply(&self, s: &mut Sentence) {
        for r in &self.rules {
            if r.pattern.elems.is_empty() {
                continue;
            }
            let matches = find_matches(&r.pattern, &s.tokens);
            for m in matches {
                // antipattern veto: any match of an antipattern vetoes this one
                if r.antipatterns.iter().any(|ap| {
                    let am = find_matches(ap, &s.tokens);
                    am.iter().any(|a| a.token_start < m.token_end && a.token_end > m.token_start)
                }) {
                    continue;
                }
                // LanguageTool applies disambiguation actions to the *marked*
                // span only (`getOriginalPosition(firstMatchToken +
                // correctedStPos)`), and to the whole match when no marker is
                // given.
                let (lo, hi) = if r.has_marker && m.per_elem.len() > r.marker_start {
                    let start = m.per_elem[r.marker_start];
                    let last = (r.marker_start + r.marker_len - 1).min(m.per_elem.len() - 1);
                    let end = m.per_elem[last] + 1;
                    (start.max(m.token_start), end.min(m.token_end).max(start + 1))
                } else {
                    (m.token_start, m.token_end)
                };
                for idx in lo..hi {
                    if idx >= s.tokens.len() {
                        continue;
                    }
                    if let Some(group) = r.match_group {
                        // `<match no="N" postag="P"/>`: the action applies to
                        // the token of group N (the marked span) and keeps
                        // only the tags matching `P`.
                        let gidx = idx;
                        let _ = group;
                        if let Some(pat) = &r.match_postag {
                            let is_re = pat.contains(".*") || pat.contains('[');
                            let tok = &mut s.tokens[gidx];
                            let keep: Vec<usize> = tok
                                .tags
                                .iter()
                                .enumerate()
                                .filter(|(_, t)| {
                                    if is_re {
                                        crate::tagregex::regex_find(pat, t)
                                    } else {
                                        pat.split('|').any(|p| p == *t)
                                    }
                                })
                                .map(|(i, _)| i)
                                .collect();
                            let tags: Vec<String> = keep.iter().map(|&i| tok.tags[i].clone()).collect();
                            let lemmas: Vec<String> = keep.iter().map(|&i| tok.lemmas[i].clone()).collect();
                            tok.tags = tags;
                            tok.lemmas = lemmas;
                        }
                        continue;
                    }
                    self.apply_action(s, r, idx);
                }
            }
        }
    }

    fn apply_action(&self, s: &mut Sentence, r: &DisambigRule, idx: usize) {
        // An empty `postag` on a legacy `<disambig postag=""/>` clears every
        // tag (LanguageTool semantics); `filterall` without postag is a no-op.
        let tag_match = |tag: &str| -> bool {
            if r.postag.is_empty() {
                true
            } else if r.is_regexp {
                // LanguageTool's disambiguation postags are matched with
                // `Matcher.find()` on the tag, but the tagger/lefff variants
                // (`V etre ppa e sp` for `V ppa .*`) mean a *prefix*/loose
                // match is intended, so we also accept the same tag with the
                // auxiliary name removed.
                crate::tagregex::regex_find(&r.postag, tag)
                    || crate::tagregex::regex_find(&r.postag, &strip_aux(tag))
            } else {
                r.postag.split('|').any(|p| p == tag)
            }
        };
        let tok = &mut s.tokens[idx];
        match r.action {
            DisambigAction::Remove => {
                tok.tags.retain(|t| !tag_match(t));
            }
            DisambigAction::Filter | DisambigAction::FilterAll => {
                let keep: Vec<usize> = tok
                    .tags
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| tag_match(t))
                    .map(|(i, _)| i)
                    .collect();
                let tags: Vec<String> = keep.iter().map(|&i| tok.tags[i].clone()).collect();
                let lemmas: Vec<String> = keep.iter().map(|&i| tok.lemmas[i].clone()).collect();
                tok.tags = tags;
                tok.lemmas = lemmas;
            }
            DisambigAction::Replace => {
                let add: Vec<(String, String)> = r.add_tags.clone();
                tok.tags.retain(|t| !tag_match(t));
                tok.lemmas.retain(|_| false);
                let text = tok.text.clone();
                for (lemma, tag) in add {
                    let l = if lemma.is_empty() { text.clone() } else { lemma };
                    tok.add(&l, &tag);
                }
            }
            DisambigAction::Add => {
                let add: Vec<(String, String)> = r.add_tags.clone();
                let text = tok.text.clone();
                for (lemma, tag) in add.into_iter() {
                    // `_GN_*` are nominal-group *chunk* tags in LanguageTool,
                    // not part-of-speech readings; we do not expose chunk tags
                    // to the rule engine, so they are not added as readings.
                    if tag.starts_with("_GN_") {
                        continue;
                    }
                    let l = if lemma.is_empty() { text.clone() } else { lemma };
                    tok.add(&l, &tag);
                }
            }
            DisambigAction::Immunize => {}
        }
    }
}


fn convert_disambig_rule(node: &XmlNode) -> DisambigRule {
    let mut r = DisambigRule {
        id: node.attr("id").unwrap_or(node.attr("name").unwrap_or("")).to_string(),
        pattern: Pattern::default(),
        antipatterns: Vec::new(),
        action: DisambigAction::Remove,
        postag: String::new(),
        is_regexp: false,
        add_tags: Vec::new(),
        match_group: None,
        match_postag: None,
        has_marker: false,
        marker_start: 0,
        marker_len: 0,
    };
    if let Some(p) = node.first_child("pattern") {
        let mut elems = Vec::new();
        let mut marked = Vec::new();
        convert_children_marked(p, &mut elems, &mut marked, false);
        r.pattern = Pattern { elems };
        if let Some(&first) = marked.first() {
            r.has_marker = true;
            r.marker_start = first;
            r.marker_len = marked.len();
        }
    }
    for ap in node.children_named("antipattern") {
        r.antipatterns.push(convert_pattern(ap));
    }
    if let Some(dis) = node.first_child("disambig") {
        let action = dis.attr("action").unwrap_or("");
        r.action = match action {
            "filter" => DisambigAction::Filter,
            "filterall" => DisambigAction::FilterAll,
            "add" => DisambigAction::Add,
            "replace" => DisambigAction::Replace,
            "immunize" => DisambigAction::Immunize,
            // spelling-only actions do not touch the tags
            "ignore_spelling" => DisambigAction::Immunize,
            "remove" => DisambigAction::Remove,
            // legacy form: <disambig postag="X"/> = remove
            _ => DisambigAction::Remove,
        };
        r.postag = dis.attr("postag").unwrap_or("").to_string();
        // `postag` is a regex when declared or when it contains regex syntax
        // (`.*`, `[...]`, `|` outside a simple alternation of full tags).
        r.is_regexp = dis.attr("postag_regexp") == Some("yes")
            || r.postag.contains(".*")
            || r.postag.contains('[')
            || r.postag.contains('(');
        for wd in dis.children_named("wd") {
            if let Some(pos) = wd.attr("pos") {
                r.add_tags.push((String::new(), pos.to_string()));
            }
        }
        for m in dis.children_named("match") {
            if let Some(no) = m.attr("no") {
                r.match_group = no.parse().ok();
            }
            r.match_postag = m.attr("postag").map(|s| s.to_string());
        }
    }
    r
}

/// Convert pattern children, recording which produced element indices are
/// inside a `<marker>` group. This keeps marker indices in sync with the
/// element list by construction.
pub fn convert_children_marked(node: &XmlNode, out: &mut Vec<PatternElem>, marked: &mut Vec<usize>, in_marker: bool) {
    for c in &node.children {
        let DomItem::Node(n) = c else { continue };
        match n.name.as_str() {
            "marker" => convert_children_marked(n, out, marked, true),
            "token" => {
                if in_marker {
                    marked.push(out.len());
                }
                out.push(PatternElem::Token(convert_token_disambig(n)));
            }
            "and" | "or" | "unify" | "unify-ignore" | "match" => {
                convert_children_marked(n, out, marked, in_marker);
            }
            _ => {}
        }
    }
}

/// Convert a `<token>` for the disambiguation rules, where exception-only
/// elements act as negative filters (they never assert a match).
pub fn convert_token_disambig(n: &XmlNode) -> TokenPattern {
    let mut t = convert_token(n);
    if t.text.is_none()
        && t.regex.is_none()
        && t.postag.is_none()
        && t.lemma.is_none()
        && !t.exceptions.is_empty()
    {
        t.exception_only_matches_any = false;
    }
    t
}

/// Public wrapper used by the grammar rule loader.
pub fn convert_children_public(node: &XmlNode, out: &mut Vec<PatternElem>) {
    convert_children(node, out);
}

/// Convert a `<pattern>`/`<antipattern>` element into a `Pattern`.
pub fn convert_pattern(node: &XmlNode) -> Pattern {
    let mut elems = Vec::new();
    convert_children(node, &mut elems);
    Pattern { elems }
}


fn convert_children(node: &XmlNode, out: &mut Vec<PatternElem>) {
    for c in &node.children {
        let DomItem::Node(n) = c else { continue };
        match n.name.as_str() {
            "token" => out.push(PatternElem::Token(convert_token(n))),
            "and" => {
                let mut seq = Vec::new();
                convert_children(n, &mut seq);
                out.push(PatternElem::Seq(seq));
            }
            "or" => {
                // every <token> child is one alternative
                let mut alts = Vec::new();
                for t in n.children_named("token") {
                    alts.push(vec![PatternElem::Token(convert_token(t))]);
                }
                if !alts.is_empty() {
                    out.push(PatternElem::Or(alts));
                }
            }
            "marker" => convert_children(n, out),
            "unify" => {
                // `<unify>`: inner tokens must agree on the listed features.
                let negate = n.attr("negate") == Some("yes");
                let features: Vec<String> = n
                    .children_named("feature")
                    .filter_map(|f| f.attr("id").map(|s| s.to_string()))
                    .collect();
                let mut inner = Vec::new();
                for c2 in &n.children {
                    let DomItem::Node(cn) = c2 else { continue };
                    if cn.name == "feature" {
                        continue;
                    }
                    convert_children_single(cn, &mut inner);
                }
                out.push(PatternElem::Unify {
                    inner,
                    negate,
                    features,
                });
            }
            "unify-ignore" => {
                convert_children(n, out);
            }
            // Wrappers that only mark a span for the message: flatten.
            "match" => convert_children(n, out),
            "suggestion" | "message" | "url" => {}
            _ => {}
        }
    }
}

/// Convert one child element of a `<unify>` group.
fn convert_children_single(n: &XmlNode, out: &mut Vec<PatternElem>) {
    match n.name.as_str() {
        "token" => out.push(PatternElem::Token(convert_token(n))),
        "marker" => convert_children(n, out),
        "unify-ignore" => convert_children(n, out),
        _ => convert_children(n, out),
    }
}

/// Convert a `<token>` element (including its `<exception>` children).
pub fn convert_token(n: &XmlNode) -> TokenPattern {
    let mut t = TokenPattern::new();
    if let Some(v) = n.attr("postag") {
        let is_re = n.attr("postag_regexp") == Some("yes");
        // `postag=""` means "no POS constraint" (LanguageTool semantics)
        if !v.is_empty() {
            t.postag = Some(ValueMatcher::new(v, is_re));
        }
        t.postag_regexp = is_re;
    }
    if let Some(v) = n.attr("lemma") {
        let is_re = n.attr("lemma_regexp") == Some("yes");
        t.lemma = Some(ValueMatcher::new(v, is_re));
    }
    if let Some(v) = n.attr("min") {
        t.min = v.parse().unwrap_or(1);
    }
    if let Some(v) = n.attr("max") {
        t.max = v.parse().unwrap_or(1);
    }
    if let Some(v) = n.attr("skip") {
        // `skip="-1"` in LanguageTool means "skip any number of tokens"
        // (unbounded); `skip="N"` allows up to N.
        t.skip = match v.parse::<i64>() {
            Ok(n) if n < 0 => u32::MAX,
            Ok(n) => n as u32,
            Err(_) => 0,
        };
    }
    t.inflected = n.attr("inflected") == Some("yes");
    t.case_sensitive = n.attr("case_sensitive") == Some("yes");
    t.space_before = match n.attr("spacebefore") {
        Some("no") => Some(false),
        Some("yes") => Some(true),
        _ => None,
    };
    t.negate_pos = n.attr("negate_pos") == Some("yes");
    let is_regexp = n.attr("regexp") == Some("yes");
    let content = n.text();
    if std::env::var("GRM_TOK_DEBUG").is_ok() {
        eprintln!("convert_token: name={} attrs={:?} content={:?}", n.name, n.attrs, content);
    }
    let content = content.trim();
    if !content.is_empty() {
        if is_regexp {
            t.regex = Some(ValueMatcher::new(content, true));
        } else {
            t.text = Some(content.to_string());
        }
    }
    // `regexp="yes"` with empty content is a constraint-free token (matches any
    // token); LT only writes it when combined with other constraints.
    for ex in n.children_named("exception") {
        let mut e = TokenPattern::new();
        if let Some(v) = ex.attr("postag") {
            let is_re = ex.attr("postag_regexp") == Some("yes");
            if !v.is_empty() {
                e.postag = Some(ValueMatcher::new(v, is_re));
            }
        }
        if let Some(v) = ex.attr("lemma") {
            let is_re = ex.attr("lemma_regexp") == Some("yes");
            e.lemma = Some(ValueMatcher::new(v, is_re));
        }
        let is_regexp = ex.attr("regexp") == Some("yes");
        let c = ex.text();
        let c = c.trim();
        if !c.is_empty() {
            if is_regexp {
                e.regex = Some(ValueMatcher::new(c, true));
            } else {
                e.text = Some(c.to_string());
            }
        }
        let scope = match ex.attr("scope").unwrap_or("current") {
            "previous" => crate::pattern::Scope::Previous,
            "next" => crate::pattern::Scope::Next,
            _ => crate::pattern::Scope::Current,
        };
        if let Some(min) = ex.attr("min") {
            e.min = min.parse().unwrap_or(1);
        }
        if let Some(max) = ex.attr("max") {
            e.max = max.parse().unwrap_or(1);
        }
        if let Some(v) = ex.attr("inflected") {
            e.inflected = v == "yes";
        }
        t.exceptions.push(crate::pattern::Exception {
            pattern: Box::new(e),
            scope,
        });
    }
    t
}


/// Count pattern elements before/inside the `<marker>` group.
///
/// The count must mirror exactly the element list produced by
/// `convert_children`, because `marker_start` is used as an index into
/// `Match::per_elem`.
fn marker_layout(node: &XmlNode, in_marker: bool, before: &mut Vec<()>, marked: &mut Vec<()>) {
    for c in &node.children {
        let DomItem::Node(n) = c else { continue };
        match n.name.as_str() {
            "marker" => marker_layout(n, true, before, marked),
            "token" => {
                if in_marker {
                    marked.push(());
                } else {
                    before.push(());
                }
            }
            "and" | "unify" | "unify-ignore" => marker_layout(n, in_marker, before, marked),
            "or" => {
                // count each alternative as one element (matches convert_children)
                if in_marker {
                    marked.push(());
                } else {
                    before.push(());
                }
            }
            _ => {}
        }
    }
}
