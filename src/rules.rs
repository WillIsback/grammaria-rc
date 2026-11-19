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
use crate::sentence::{AnalyzedToken, Sentence};
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
                pat.text = None;
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
                    let group_off = node.attr("default") == Some("off");
                    for (i, sub) in node.children_named("rule").enumerate() {
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
                                let group_off = n.attr("default") == Some("off");
                                for (i, sub) in n.children_named("rule").enumerate() {
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
    let default_off = node.attr("default") == Some("off");
    let urls: Vec<String> = node.children_named("url").map(|u| u.text().trim().to_string()).collect();
    let suppress_match = node.children_named("filter").any(|f| {
        f.attr("args").map(|a| a.contains("suppressMatch:true")).unwrap_or(false)
    });
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
        for rule in &self.rules {
            if rule.is_style && !style_checking {
                continue;
            }
            if rule.default_off || rule.suppress_match {
                continue;
            }
            let matches = find_matches(&rule.pattern, &s.tokens);
            for m in matches {
                // antipatterns veto overlapping matches
                if rule.antipatterns.iter().any(|ap| {
                    find_matches(ap, &s.tokens)
                        .iter()
                        .any(|a| a.token_start < m.token_end && a.token_end > m.token_start)
                }) {
                    continue;
                }
                // resolve overlaps: a match starting at the same token as an
                // existing one is dropped unless it has a higher priority.
                if out.iter().any(|e| {
                    let es = e.offset.saturating_sub(s.offset);
                    let ee = es + e.length;
                    es == m.start && e.priority >= rule.priority
                }) {
                    continue;
                }
                if let Some(rm) = self.render_match(rule, s, &m) {
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
