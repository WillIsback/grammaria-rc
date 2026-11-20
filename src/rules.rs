//! Grammar rules: loading `grammar.xml`/`style.xml`, matching them over
//! analysed sentences and producing matches with suggestions.
//!
//! Each `<rule>` has a `<pattern>`, zero or more `<antipattern>`s, a
//! `<message>` and a list of `<suggestion>`s. Messages and suggestions may
//! contain `<match no="N" .../>` elements that pull the text (or a synthesized
//! form) of captured group `N` and re-case it.
//!
//! The engine runs all rules over each sentence and collects the matches,
//! resolving overlaps by priority and rule order the way LanguageTool does
//! (first match at a given start position wins; overlapping later matches of
//! lower priority are dropped).

use crate::pattern::{find_matches, Match, Pattern, PatternElem, TokenPattern};
use crate::tagger::convert_children_public;
use crate::sentence::Sentence;
use crate::strutil;
use crate::xml::{self, DomItem, Node as XmlNode};

/// One piece of a message/suggestion: literal text or a `<match>` reference.
#[derive(Clone, Debug)]
pub enum Part {
    Text(String),
    /// `<match no="N" .../>`
    Ref(MatchRef),
}

#[derive(Clone, Debug, Default)]
pub struct MatchRef {
    pub no: usize,
    /// Keep only the readings whose postag matches (and replace the tag).
    pub postag: Option<String>,
    pub postag_regexp: bool,
    pub postag_replace: Option<String>,
    pub regexp_match: Option<String>,
    pub regexp_replace: Option<String>,
    pub case_conversion: Option<String>,
    pub include_skipped: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low = 1,
    Medium = 2,
    High = 3,
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub id: String,
    /// Full id including the group id for `<rulegroup>` children.
    pub full_id: String,
    pub sub_id: Option<String>,
    pub name: String,
    pub message: Vec<Part>,
    pub short_message: Option<String>,
    pub suggestions: Vec<Vec<Part>>,
    pub pattern: Pattern,
    pub antipatterns: Vec<Pattern>,
    pub priority: Priority,
    pub category_id: String,
    pub category_name: String,
    /// `type="style"` rules are only reported when style checking is on.
    pub is_style: bool,
    /// `default="off"`.
    pub default_off: bool,
    /// URLs shown with the message.
    pub urls: Vec<String>,
    /// Filters that suppress the whole match (`suppressMatch:true`), e.g.
    /// `FrenchSuppressMisspelledSuggestionsFilter`.
    pub suppress_match: bool,
    /// `tags` attribute values; `picky` rules are off by default in
    /// LanguageTool and only enabled explicitly.
    pub tags: Vec<String>,
    /// Native filter implementations attached to the rule.
    pub filters: Vec<crate::filters::RuleFilter>,
    /// The rule uses a construct the engine does not reproduce faithfully
    /// (e.g. a pattern-level `raw_pos`, `<match no="N"/>` inside a pattern, or
    /// an `<and>` group). Such rules are skipped to protect precision.
    pub unsupported: Option<String>,
    /// Java filter class names referenced by `<filter class="...">`. Filters
    /// that only suppress suggestions (FindSuggestionsFilter etc.) are
    /// cosmetic, but filters that *validate* a match (e.g. postponed adjective
    /// concordance) cannot be reproduced without them: such rules are skipped
    /// to avoid false positives.
    pub java_filters: Vec<String>,
    /// Example sentences: (text, corrections, type).
    pub examples: Vec<Example>,
}

#[derive(Clone, Debug)]
pub struct Example {
    pub text: String,
    pub corrections: Vec<String>,
    /// "corrected" (default) or "incorrect"/"triggers_error".
    pub kind: String,
}

/// A reported problem.
#[derive(Clone, Debug)]
pub struct RuleMatch {
    pub rule_id: String,
    pub full_rule_id: String,
    pub message: String,
    pub short_message: String,
    pub suggestions: Vec<String>,
    /// Byte offsets in the document.
    pub offset: usize,
    pub length: usize,
    pub category_id: String,
    pub category_name: String,
    pub priority: Priority,
    pub is_style: bool,
    pub urls: Vec<String>,
    /// The sentence text the match belongs to.
    pub sentence: String,
}

/// A rule set loaded from a grammar XML file.
#[derive(Default)]
pub struct RuleSet {
    pub rules: Vec<Rule>,
    /// Patterns from `<phrases>` reusable fragment definitions.
    pub phrases: std::collections::HashMap<String, Pattern>,
    /// Unification features: feature name -> equivalence type -> (postag, is_regex).
    pub unification: std::collections::HashMap<String, Vec<(String, TokenPattern)>>,
    /// Index of rules whose first pattern element is a plain literal:
    /// lowercase word -> rule indices. Rules whose first element is not a
    /// literal are always tried.
    pub by_literal: std::collections::HashMap<String, Vec<usize>>,
    /// Rule indices that have no literal first element.
    pub generic: Vec<usize>,
    /// Index of rules whose first element constrains the POS tag: tag prefix
    /// letter (first character of the `postag` source) -> rule indices.
    pub by_postag: std::collections::HashMap<char, Vec<usize>>,
}

impl RuleSet {
    /// Build the lookup index used by `check_sentence` (called after loading).
    pub fn build_index(&mut self) {
        self.by_literal.clear();
        self.generic.clear();
        self.by_postag.clear();
        for (i, r) in self.rules.iter().enumerate() {
            match r.pattern.elems.first() {
                Some(PatternElem::Token(t)) => match (if t.has_text() { Some(t.text()) } else { None }, &t.regex) {
                    (Some(lit), _) => {
                        if is_plain_literal(lit) {
                            self.by_literal.entry(crate::strutil::lower(lit)).or_default().push(i);
                        } else {
                            self.generic.push(i);
                        }
                    }
                    // A first element that is a regex: if it is a plain
                    // alternation of literals (`tout|toute|toutes`, `mange`,
                    // `une?`), index every alternative so the rule is only
                    // tried when one of those words occurs.
                    (None, Some(re)) => match alternation_keys(re.as_source()) {
                        Some(keys) if !keys.is_empty() => {
                            for k in keys {
                                self.by_literal.entry(k).or_default().push(i);
                            }
                        }
                        _ => self.generic.push(i),
                    },
                    (None, _) => match &t.postag {
                        Some(pm) => {
                            let src = pm.as_source();
                            // A postag starting with a concrete tag letter (e.g.
                            // `V .*`, `N f s`, `[DJ] .*`) can be bucketed by that
                            // first character; complex regexes stay generic.
                            let first = src.chars().next().unwrap_or('?');
                            if first.is_ascii_uppercase() || (first == '[' && src.chars().nth(1).map(|c| c.is_ascii_uppercase()).unwrap_or(false)) {
                                let key = if first == '[' {
                                    src.chars().nth(1).unwrap_or('?')
                                } else {
                                    first
                                };
                                self.by_postag.entry(key).or_default().push(i);
                            } else {
                                self.generic.push(i);
                            }
                        }
                        None => self.generic.push(i),
                    },
                },
                _ => self.generic.push(i),
            }
        }
    }
}

/// LanguageTool filter classes that *validate* a match (as opposed to merely
/// de-duplicating or suppressing suggestions). Without a re-implementation of
/// the class, such a rule over-fires, so it is disabled.
const VALIDATING_FILTERS: &[&str] = &[
    // Multi-token spelling rules rely on LanguageTool's morfologik speller to
    // decide whether the *joined* form is a misspelling; without that oracle
    // they fire on ordinary text.
    "FR_MULTITOKEN_SPELLING",
    "FrenchMultitokenSpeller",
    "PostponedAdjectiveConcordanceFilter",
    "InterrogativeVerbFilter",
    "DateCheckFilter",
    "DMYDateCheckFilter",
    "NewYearDateFilter",
    "FrenchNumberInWordFilter",
    "MakeContractionsFilter",
    // `FindSuggestionsFilter` looks a word up in the morphological dictionary
    // to find real-word corrections matching a desired postag (`wordFrom:3
    // desiredPostag:V.*(ind|sub).*1.s`). LanguageTool only reports the rule
    // when such a correction exists, so without the dictionary lookup we would
    // fire on ordinary text ("Je vous envoie les documents ci-joint." is clean
    // in LT but matches `SUJET_AUXILIAIRE[1]` for us). It therefore validates.
    "FindSuggestionsFilter",
];

/// Expand a first-token regex into the set of literal keys it can match, when
/// the pattern is a simple alternation of literal words with optional trailing
/// `s`/`?`. Returns `None` when the regex is too complex to index (metacharacters
/// such as `.`, `[`, `\d`), in which case the rule stays in the generic bucket.
///
/// Examples: `tout|toute|toutes` -> {tout, toute, toutes};
/// `une?` -> {un, une}; `[mts]on` -> None (character class).
fn alternation_keys(src: &str) -> Option<Vec<String>> {
    let complex = |s: &str| {
        s.bytes().any(|c| {
            matches!(
                c,
                b'.' | b'[' | b']' | b'(' | b')' | b'*' | b'+' | b'{' | b'}' | b'^' | b'$' | b'\\'
            )
        })
    };
    let mut keys = Vec::new();
    for alt in src.split('|') {
        if alt.is_empty() {
            return None;
        }
        // Strip a trailing `?` (optional last character) or `s?`.
        let base = alt.strip_suffix('?').unwrap_or(alt);
        if base.is_empty() || complex(base) {
            return None;
        }
        keys.push(crate::strutil::lower(base));
        // `une?` also matches `un`.
        if alt.ends_with('?') && !base.is_empty() {
            let mut chars: Vec<char> = base.chars().collect();
            chars.pop();
            if !chars.is_empty() {
                keys.push(crate::strutil::lower(&chars.iter().collect::<String>()));
            }
        }
    }
    keys.retain(|k| !k.is_empty());
    keys.sort();
    keys.dedup();
    Some(keys)
}

/// True when a literal contains no regex-ish characters.
fn is_plain_literal(s: &str) -> bool {
    !s.bytes().any(|c| matches!(c, b'.' | b'[' | b']' | b'(' | b')' | b'|' | b'?' | b'*' | b'+' | b'{' | b'}' | b'^' | b'$' | b'\\'))
}

impl RuleSet {
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Parse a grammar XML document.
    pub fn parse(xml: &str, is_style_file: bool) -> RuleSet {
        let doc = xml::parse_dom(xml);
        let mut set = RuleSet::default();
        let root: &XmlNode = if let Some(r) = doc.first_child("rules") {
            r
        } else if doc.name == "rules" {
            &doc
        } else {
            return set;
        };
        // unification definitions
        for unif in root.children_named("unification") {
            let feature = unif.attr("feature").unwrap_or("").to_string();
            let mut eqs = Vec::new();
            for eq in unif.children_named("equivalence") {
                let ty = eq.attr("type").unwrap_or("").to_string();
                let mut pat = TokenPattern::new();
                if let Some(tok) = eq.first_child("token") {
                    pat = crate::tagger::convert_token(tok);
                }
                pat.text_sym = crate::sym::Sym::EMPTY;
                eqs.push((ty, pat));
            }
            set.unification.insert(feature, eqs);
        }
        // reusable phrases
        for phrases in root.children_named("phrases") {
            for ph in phrases.children_named("phrase") {
                if let Some(id) = ph.attr("id") {
                    let mut elems = Vec::new();
                    crate::tagger::convert_children_public(ph, &mut elems);
                    set.phrases.insert(id.to_string(), Pattern { elems });
                }
            }
        }
        for el in &root.children {
            let DomItem::Node(node) = el else { continue };
            match node.name.as_str() {
                "rule" => {
                    if let Some(r) = convert_rule(node, None, "", "", is_style_file) {
                        set.rules.push(r);
                    }
                }
                "rulegroup" => {
                    let gid = node.attr("id").unwrap_or("").to_string();
                    let gname = node.attr("name").unwrap_or("").to_string();
                    let group_off = matches!(node.attr("default"), Some("off") | Some("temp_off"));
                    for (i, sub) in node.children_named("rule").enumerate() {
                        // A nested <rule> carrying its own `id` is a standalone
                        // rule that merely lives inside the group element.
                        if sub.attr("id").map(|x| !x.is_empty()).unwrap_or(false) {
                            if let Some(mut r) = convert_rule_at(sub, None, &gname, &gname, is_style_file, i + 1) {
                                if group_off {
                                    r.default_off = true;
                                }
                                set.rules.push(r);
                            }
                            continue;
                        }
                        if let Some(mut r) = convert_rule_at(sub, Some(&gid), &gname, &gname, is_style_file, i + 1) {
                            // attributes like default="off" declared on the
                            // rulegroup apply to all its rules
                            if group_off {
                                r.default_off = true;
                            }
                            set.rules.push(r);
                        }
                    }
                }
                "category" => {
                    let cid = node.attr("id").unwrap_or("").to_string();
                    let cname = node.attr("name").unwrap_or("").to_string();
                    for item in &node.children {
                        let DomItem::Node(n) = item else { continue };
                        match n.name.as_str() {
                            "rule" => {
                                if let Some(r) = convert_rule(n, None, &cid, &cname, is_style_file) {
                                    set.rules.push(r);
                                }
                            }
                            "rulegroup" => {
                                let gid = n.attr("id").unwrap_or("").to_string();
                                let group_off = matches!(n.attr("default"), Some("off") | Some("temp_off"));
                                for (i, sub) in n.children_named("rule").enumerate() {
                                    if sub.attr("id").map(|x| !x.is_empty()).unwrap_or(false) {
                                        if let Some(mut r) = convert_rule_at(sub, None, &cid, &cname, is_style_file, i + 1) {
                                            if group_off {
                                                r.default_off = true;
                                            }
                                            set.rules.push(r);
                                        }
                                        continue;
                                    }
                                    if let Some(mut r) = convert_rule_at(sub, Some(&gid), &cid, &cname, is_style_file, i + 1) {
                                        if group_off {
                                            r.default_off = true;
                                        }
                                        set.rules.push(r);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        set.build_index();
        set
    }
}

fn convert_rule_at(
    node: &XmlNode,
    group: Option<&str>,
    category_id: &str,
    category_name: &str,
    is_style_file: bool,
    rule_index: usize,
) -> Option<Rule> {
    convert_rule_impl(node, group, category_id, category_name, is_style_file, rule_index)
}

fn convert_rule(
    node: &XmlNode,
    group: Option<&str>,
    category_id: &str,
    category_name: &str,
    is_style_file: bool,
) -> Option<Rule> {
    convert_rule_impl(node, group, category_id, category_name, is_style_file, 1)
}

fn convert_rule_impl(
    node: &XmlNode,
    group: Option<&str>,
    category_id: &str,
    category_name: &str,
    is_style_file: bool,
    rule_index: usize,
) -> Option<Rule> {
    let pattern_node = node.first_child("pattern")?;
    let mut elems = Vec::new();
    convert_children_public(pattern_node, &mut elems);
    if elems.is_empty() {
        return None;
    }
    let pattern = Pattern { elems };
    let mut antipatterns = Vec::new();
    for ap in node.children_named("antipattern") {
        let mut e = Vec::new();
        convert_children_public(ap, &mut e);
        if !e.is_empty() {
            antipatterns.push(Pattern { elems: e });
        }
    }
    // A `<rule>` inside a `<rulegroup>` has no id of its own: its rule id is
    // the group id.
    let own_id = node.attr("id").unwrap_or("").to_string();
    let id = if own_id.is_empty() {
        group.map(|g| g.to_string()).unwrap_or_default()
    } else {
        own_id
    };
    let name = node.attr("name").unwrap_or("").to_string();
    let (category_id, category_name) = match node.first_child("category") {
        Some(c) => (
            c.attr("id").unwrap_or(category_id).to_string(),
            c.attr("name").unwrap_or(category_name).to_string(),
        ),
        None => (category_id.to_string(), category_name.to_string()),
    };
    let message_node = node.first_child("message");
    let message = message_node.map(convert_parts).unwrap_or_default();
    let short_message = message_node.and_then(|m| m.first_child("short")).map(|s| s.text().trim().to_string());
    let mut suggestions = Vec::new();
    for s in node.children_named("suggestion") {
        suggestions.push(convert_parts(s));
    }
    let priority = match node.attr("priority") {
        Some("low") => Priority::Low,
        Some("high") => Priority::High,
        _ => Priority::Medium,
    };
    let is_style = is_style_file || node.attr("type") == Some("style");
    // `default="off"` and `default="temp_off"` both mean the rule is not
    // active by default in LanguageTool.
    let default_off = matches!(node.attr("default"), Some("off") | Some("temp_off"));
    let urls: Vec<String> = node.children_named("url").map(|u| u.text().trim().to_string()).collect();
    let suppress_match = node.children_named("filter").any(|f| {
        f.attr("args").map(|a| a.contains("suppressMatch:true")).unwrap_or(false)
    });
    let tags: Vec<String> = node
        .attr("tags")
        .map(|t| t.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_default();
    let java_filters: Vec<String> = node
        .children_named("filter")
        .filter_map(|f| f.attr("class").map(|c| c.rsplit('.').next().unwrap_or(c).to_string()))
        .collect();
    let mut unsupported: Option<String> = None;
    if let Some(p) = node.first_child("pattern") {
        if p.attr("raw_pos") == Some("yes") {
            unsupported = Some("raw_pos".into());
        }
        // `<match>` inside a pattern, `<and>` groups and `<unify>` with
        // in-pattern *additions* are only partially supported.
        if p.descendants_named("match").next().is_some() {
            unsupported = Some("pattern-match".into());
        }
    }
    let filters: Vec<crate::filters::RuleFilter> = node
        .children_named("filter")
        .filter_map(|f| {
            f.attr("class").map(|c| crate::filters::RuleFilter {
                class: c.to_string(),
                args: crate::filters::FilterArgs::parse(f.attr("args").unwrap_or("")),
            })
        })
        .collect();
    let sub_id = node
        .attr("subId")
        .map(|s| s.to_string())
        .or_else(|| group.map(|_| rule_index.to_string()));
    let mut examples = Vec::new();
    let ex_nodes: Vec<&XmlNode> = node.children_named("example").collect();
    for ex in ex_nodes {
        let text = ex.text();
        let corrections = ex
            .attr("correction")
            .map(|c| c.split('|').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default();
        examples.push(Example {
            text: text.trim().to_string(),
            corrections,
            kind: ex.attr("type").unwrap_or("corrected").to_string(),
        });
    }
    let full_id = match group {
        Some(g) => match sub_id.as_deref() {
            Some(sid) => format!("{g}[{sid}]"),
            None => g.to_string(),
        },
        None => id.clone(),
    };
    Some(Rule {
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
        urls,
        suppress_match,
        tags,
        java_filters,
        filters,
        unsupported,
        examples,
    })
}

/// Convert the children of a `<message>`/`<suggestion>` into parts.
pub fn convert_parts(node: &XmlNode) -> Vec<Part> {
    let mut parts = Vec::new();
    for c in &node.children {
        match c {
            DomItem::Text(t) => {
                if !t.is_empty() {
                    parts.push(Part::Text(t.to_string()));
                }
            }
            DomItem::Node(n) => match n.name.as_str() {
                "match" => {
                    parts.push(Part::Ref(convert_match_ref(n)));
                }
                "suggestion" => {
                    if let Some(inner) = n.first_child("match") {
                        parts.push(Part::Ref(convert_match_ref(inner)));
                    }
                }
                "short" | "url" => {}
                _ => {
                    let inner = convert_parts(n);
                    parts.extend(inner);
                }
            },
        }
    }
    parts
}

fn convert_match_ref(n: &XmlNode) -> MatchRef {
    MatchRef {
        no: n.attr("no").and_then(|s| s.parse().ok()).unwrap_or(1),
        postag: n.attr("postag").map(|s| s.to_string()),
        postag_regexp: n.attr("postag_regexp") == Some("yes"),
        postag_replace: n.attr("postag_replace").map(|s| s.to_string()),
        regexp_match: n.attr("regexp_match").map(|s| s.to_string()),
        regexp_replace: n.attr("regexp_replace").map(|s| s.to_string()),
        case_conversion: n.attr("case_conversion").map(|s| s.to_string()),
        include_skipped: n.attr("include_skipped") == Some("yes"),
    }
}

impl RuleSet {
    /// Check one sentence, returning the matches found in it.
    pub fn check_sentence(&self, s: &Sentence, style_checking: bool) -> Vec<RuleMatch> {
        let mut out: Vec<RuleMatch> = Vec::new();
        // Candidate rules: those whose first pattern element is a literal
        // present in the sentence, plus every rule that starts with a
        // non-literal element (cheap membership test, like LanguageTool's
        // Aho-Corasick pre-filter).
        let mut candidates: Vec<usize> = Vec::with_capacity(self.generic.len() * 2);
        if self.by_literal.is_empty() {
            candidates.extend(0..self.rules.len());
        } else {
            candidates.extend(self.generic.iter().copied());
            for t in &s.tokens {
                let lower = crate::strutil::lower(&t.text);
                if let Some(v) = self.by_literal.get(&lower) {
                    candidates.extend(v.iter().copied());
                }
                // POS-bucketed rules: check the first letter of every tag.
                for tag in &t.tags {
                    if let Some(c) = tag.chars().next() {
                        if let Some(v) = self.by_postag.get(&c) {
                            candidates.extend(v.iter().copied());
                        }
                    }
                }
            }
            candidates.sort_unstable();
            candidates.dedup();
        }
        for &ri in &candidates {
            let rule = &self.rules[ri];
            if rule.is_style && !style_checking {
                continue;
            }
            if rule.default_off || rule.suppress_match {
                continue;
            }
            // `picky` rules are disabled by default in LanguageTool.
            if !style_checking && rule.tags.iter().any(|t| t == "picky") {
                continue;
            }
            // Rules whose match validity depends on a Java filter (or on the
            // spell checker) that we cannot reproduce would produce false
            // positives: skip them.
            if rule.java_filters.iter().any(|f| VALIDATING_FILTERS.contains(&f.as_str())) {
                continue;
            }
            if rule.id.starts_with("FR_MULTITOKEN_SPELLING") {
                continue;
            }
            if rule.unsupported.is_some() {
                continue;
            }
            let matches = find_matches(&rule.pattern, &s.tokens);
            if matches.is_empty() {
                continue;
            }
            // Antipattern matches are computed at most once per rule.
            let mut ap_spans: Option<Vec<(usize, usize)>> = None;
            for m in &matches {
                if !rule.antipatterns.is_empty() {
                    let spans = ap_spans.get_or_insert_with(|| {
                        let mut v = Vec::new();
                        for ap in &rule.antipatterns {
                            for a in find_matches(ap, &s.tokens) {
                                v.push((a.token_start, a.token_end));
                            }
                        }
                        v
                    });
                    if spans.iter().any(|&(a, b)| a < m.token_end && b > m.token_start) {
                        continue;
                    }
                }
                // resolve overlaps: a match starting at the same token as an
                // existing one is dropped unless it has a higher priority.
                if out.iter().any(|e| {
                    let es = e.offset.saturating_sub(s.offset);
                    es == m.start && e.priority >= rule.priority
                }) {
                    continue;
                }
                // Run native filters: they validate the match and may add or
                // replace the suggestions.
                // Native filters: a filter returning "not an error" discards
                // the match; a filter returning "error" may supply its own
                // (better) suggestions, which replace the static ones.
                let mut extra: Vec<String> = Vec::new();
                let mut has_extra = false;
                let mut keep = true;
                for f in &rule.filters {
                    let outcome = f.apply(s, m);
                    if !outcome.is_error {
                        keep = false;
                        break;
                    }
                    if outcome.has_suggestions {
                        extra = outcome.suggestions;
                        has_extra = true;
                    }
                }
                if !keep {
                    continue;
                }
                // Never report a zero-width match (e.g. one anchored only on
                // SENT_START/SENT_END): the user cannot act on it.
                if m.end <= m.start {
                    continue;
                }
                if let Some(mut rm) = self.render_match(rule, s, m) {
                    if has_extra {
                        rm.suggestions = extra;
                    }
                    out.push(rm);
                }
            }
        }
        out
    }

    fn render_match(&self, rule: &Rule, s: &Sentence, m: &Match) -> Option<RuleMatch> {
        let message = self.render_parts(&rule.message, s, m);
        let short = rule
            .short_message
            .clone()
            .unwrap_or_else(|| message.clone());
        let mut suggestions = Vec::new();
        for sug in &rule.suggestions {
            let t = self.render_parts(sug, s, m);
            if !t.is_empty() && t != m_text(s, m) {
                suggestions.push(t);
            }
        }
        Some(RuleMatch {
            rule_id: rule.id.clone(),
            full_rule_id: rule.full_id.clone(),
            message,
            short_message: short,
            suggestions,
            offset: s.offset + m.start,
            length: m.end.saturating_sub(m.start),
            category_id: rule.category_id.clone(),
            category_name: rule.category_name.clone(),
            priority: rule.priority,
            is_style: rule.is_style,
            urls: rule.urls.clone(),
            sentence: s.text.trim().to_string(),
        })
    }

    fn render_parts(&self, parts: &[Part], s: &Sentence, m: &Match) -> String {
        let mut out = String::new();
        for p in parts {
            match p {
                Part::Text(t) => out.push_str(&substitute_refs(t, s, m)),
                Part::Ref(r) => {
                    let text = match_text(s, m, r);
                    out.push_str(&apply_case_conversion(&text, r.case_conversion.as_deref(), &match_text(s, m, &MatchRef { no: r.no, include_skipped: true, ..Default::default() })));
                }
            }
        }
        out.trim().to_string()
    }
}

/// Replace `\1`..`\9` in message text by the corresponding captured group.
fn substitute_refs(text: &str, s: &Sentence, m: &Match) -> String {
    if !text.contains('\\') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == '\\' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let n = bytes[i + 1] as usize - '0' as usize;
            if let Some(&(a, b)) = m.spans.get(n.saturating_sub(1)) {
                out.push_str(s.text.get(a..b).unwrap_or(""));
            }
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

fn m_text(s: &Sentence, m: &Match) -> String {
    s.text.get(m.start..m.end).unwrap_or("").to_string()
}

/// Resolve `<match no="N"/>` against the sentence.
fn match_text(s: &Sentence, m: &Match, r: &MatchRef) -> String {
    if let Some(&(a, b)) = m.spans.get(r.no.saturating_sub(1)) {
        let base = s.text.get(a..b).unwrap_or("").to_string();
        let mut text = base;
        if let Some(rm) = &r.regexp_match {
            if let Some(repl) = &r.regexp_replace {
                text = crate::tagregex::regex_replace_all(rm, &text, repl);
            }
        }
        if let Some(conv) = &r.case_conversion {
            let _ = conv;
            text = apply_case_conversion(&text, Some(conv.as_str()), &text);
        }
        text
    } else {
        String::new()
    }
}

/// Apply a `case_conversion` attribute (`alllower`, `allupper`,
/// `startlower`, `startupper`, `preserve`).
pub fn apply_case_conversion(text: &str, mode: Option<&str>, original: &str) -> String {
    match mode {
        Some("alllower") => strutil::lower(text),
        Some("allupper") => strutil::upper(text),
        Some("startlower") => strutil::decapitalize(text),
        Some("startupper") => strutil::capitalize(text),
        Some("preserve") => {
            // keep the capitalisation pattern of `original`
            if strutil::starts_upper(original) && !strutil::starts_upper(text) {
                strutil::capitalize(text)
            } else if !strutil::starts_upper(original) && strutil::starts_upper(text) {
                strutil::decapitalize(text)
            } else {
                text.to_string()
            }
        }
        _ => text.to_string(),
    }
}

/// Check a whole document: split into sentences, tag, and run all rules.
pub fn check_text(
    tagger: &crate::tagger::Tagger,
    rules: &RuleSet,
    text: &str,
    style_checking: bool,
) -> Vec<RuleMatch> {
    let mut out = Vec::new();
    for (a, b) in crate::tokenizer::sentence_boundaries(text) {
        let s = tagger.tag(&text[a..b], a);
        if s.tokens.is_empty() {
            continue;
        }
        out.extend(rules.check_sentence(&s, style_checking));
    }
    out.sort_by_key(|m| m.offset);
    out
}
