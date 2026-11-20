//! A small, dependency-free, streaming XML pull parser.
//!
//! LanguageTool's rule files are plain XML (some with a DOCTYPE that declares
//! internal entities). Pulling in a full XML stack (quick-xml/roxmltree) would
//! add hundreds of kilobytes of binary, so Grammaria ships with this minimal
//! parser instead. It supports exactly the subset needed to read
//! `grammar.xml`, `style.xml` and `disambiguation.xml`:
//!
//! * elements, attributes, text (with entity decoding),
//! * CDATA sections, comments, processing instructions,
//! * a DOCTYPE with internal `<!ENTITY name "value">` / `<!ENTITY name 'value'>`
//!   declarations (values may reference earlier entities) and `<!-- -->`
//!   comments -- LanguageTool uses these heavily to avoid repeating words,
//! * the five predefined XML entities and numeric character references.
//!
//! The parser is deliberately forgiving: malformed input never panics, it just
//! terminates the event stream.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Start {
        name: String,
        attrs: Vec<(String, String)>,
        /// `true` for `<name .../>` -- no matching `End` event follows.
        self_closing: bool,
    },
    End {
        name: String,
    },
    Text(String),
    Eof,
}

impl Event {
    /// Look up an attribute value by name.
    pub fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
        attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

pub struct Parser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    /// Stack of open element names.
    stack: Vec<String>,
    /// Internal DTD entities (already fully expanded).
    entities: HashMap<String, String>,
}

fn is_name_char(c: u8) -> bool {
    !c.is_ascii_whitespace()
        && c != b'>'
        && c != b'/'
        && c != b'='
        && c != b'<'
        && c != b'"'
        && c != b'\''
}

impl<'a> Parser<'a> {
    pub fn new(src: &'a str) -> Parser<'a> {
        let mut p = Parser {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            stack: Vec::new(),
            entities: HashMap::new(),
        };
        p.skip_prolog();
        p
    }

    /// Parse the DOCTYPE prolog (and any leading comments) collecting entity
    /// declarations.
    fn skip_prolog(&mut self) {
        loop {
            self.skip_ws();
            if self.pos >= self.bytes.len() {
                return;
            }
            if self.starts_with("<?") {
                self.pos = self.find_from(self.pos, "?>").map(|i| i + 2).unwrap_or(self.bytes.len());
            } else if self.starts_with("<!--") {
                match self.find_from(self.pos + 4, "-->") {
                    Some(i) => self.pos = i + 3,
                    None => self.pos = self.bytes.len(),
                }
            } else if self.starts_with("<!DOCTYPE") {
                self.parse_doctype();
            } else {
                return;
            }
        }
    }

    fn parse_doctype(&mut self) {
        // Find the matching '>' accounting for an internal subset.
        let start = self.pos;
        let mut i = self.pos + "<!DOCTYPE".len();
        let mut depth = 0i32;
        let mut in_quote: Option<u8> = None;
        while i < self.bytes.len() {
            let c = self.bytes[i];
            if let Some(q) = in_quote {
                if c == q {
                    in_quote = None;
                }
            } else {
                match c {
                    b'"' | b'\'' => in_quote = Some(c),
                    b'[' => depth += 1,
                    b']' => depth -= 1,
                    b'>' if depth <= 0 => break,
                    _ => {}
                }
            }
            i += 1;
        }
        let subset = &self.src[start..i.min(self.src.len())];
        self.parse_entities(subset);
        self.pos = (i + 1).min(self.bytes.len());
    }

    fn parse_entities(&mut self, doctype: &str) {
        // Scan `<!ENTITY name "value">` declarations inside the internal subset.
        let rest = doctype;
        // Pre-scan: collect raw declarations first, then expand in declaration
        // order (later declarations may reference earlier ones).
        let mut raw: Vec<(String, String)> = Vec::new();
        let mut i = 0;
        while let Some(idx) = rest[i..].find("<!ENTITY") {
            let s = i + idx + "<!ENTITY".len();
            // name
            let mut j = s;
            while j < rest.len() && rest.as_bytes()[j].is_ascii_whitespace() {
                j += 1;
            }
            let nstart = j;
            while j < rest.len() && is_name_char(rest.as_bytes()[j]) {
                j += 1;
            }
            let name = rest[nstart..j].to_string();
            while j < rest.len() && rest.as_bytes()[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= rest.len() {
                break;
            }
            let q = rest.as_bytes()[j];
            if q == b'"' || q == b'\'' {
                let vstart = j + 1;
                let mut k = vstart;
                while k < rest.len() && rest.as_bytes()[k] != q {
                    k += 1;
                }
                let value = rest[vstart..k].to_string();
                raw.push((name, value));
                // skip to the end of the declaration
                let close = rest[k..].find('>').map(|x| k + x).unwrap_or(rest.len());
                i = close + 1;
            } else {
                // external entity or other form: skip to '>'
                let close = rest[j..].find('>').map(|x| j + x).unwrap_or(rest.len());
                i = close + 1;
            }
        }
        for (name, value) in raw {
            let expanded = self.decode_entities(&value);
            self.entities.insert(name, expanded);
        }
    }

    /// Decode entity references in a string (named + numeric).
    pub fn decode_entities(&self, s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'&' {
                if let Some(semi) = s[i..].find(';') {
                    let ent = &s[i + 1..i + semi];
                    if let Some(dec) = decode_one(ent, &self.entities) {
                        out.push_str(&dec);
                        i += semi + 1;
                        continue;
                    }
                }
            }
            // copy the UTF-8 char
            let ch_len = utf8_len(b[i]);
            out.push_str(&s[i..(i + ch_len).min(s.len())]);
            i += ch_len;
        }
        out
    }

    fn starts_with(&self, s: &str) -> bool {
        self.bytes[self.pos..].starts_with(s.as_bytes())
    }

    fn find_from(&self, from: usize, pat: &str) -> Option<usize> {
        self.src[from.min(self.src.len())..].find(pat).map(|i| i + from)
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    /// Return the next event.
    pub fn next_event(&mut self) -> Event {
        loop {
            if self.pos >= self.bytes.len() {
                return Event::Eof;
            }
            if self.bytes[self.pos] != b'<' {
                // text up to next '<'
                let end = self.find_from(self.pos, "<").unwrap_or(self.bytes.len());
                let raw = &self.src[self.pos..end];
                self.pos = end;
                if raw.trim().is_empty() && raw.is_empty() {
                    continue;
                }
                return Event::Text(self.decode_entities(raw));
            }
            // We are at '<'
            if self.starts_with("<!--") {
                match self.find_from(self.pos + 4, "-->") {
                    Some(i) => self.pos = i + 3,
                    None => self.pos = self.bytes.len(),
                }
                continue;
            }
            if self.starts_with("<![CDATA[") {
                let start = self.pos + 9;
                match self.find_from(start, "]]>") {
                    Some(i) => {
                        let text = self.src[start..i].to_string();
                        self.pos = i + 3;
                        return Event::Text(text);
                    }
                    None => {
                        let text = self.src[start..].to_string();
                        self.pos = self.bytes.len();
                        return Event::Text(text);
                    }
                }
            }
            if self.starts_with("<?") {
                match self.find_from(self.pos + 2, "?>") {
                    Some(i) => self.pos = i + 2,
                    None => self.pos = self.bytes.len(),
                }
                continue;
            }
            if self.starts_with("<!") {
                // DOCTYPE or other declaration
                let mut depth = 0i32;
                let mut i = self.pos + 2;
                let mut in_quote: Option<u8> = None;
                while i < self.bytes.len() {
                    let c = self.bytes[i];
                    if let Some(q) = in_quote {
                        if c == q {
                            in_quote = None;
                        }
                    } else {
                        match c {
                            b'"' | b'\'' => in_quote = Some(c),
                            b'[' => depth += 1,
                            b']' => depth -= 1,
                            b'>' if depth <= 0 => break,
                            _ => {}
                        }
                    }
                    i += 1;
                }
                self.pos = (i + 1).min(self.bytes.len());
                continue;
            }
            if self.starts_with("</") {
                // end tag
                let start = self.pos + 2;
                let mut i = start;
                while i < self.bytes.len() && is_name_char(self.bytes[i]) {
                    i += 1;
                }
                let name = self.src[start..i].to_string();
                if let Some(close) = self.find_from(i, ">") {
                    self.pos = close + 1;
                } else {
                    self.pos = self.bytes.len();
                }
                self.stack.pop();
                return Event::End { name };
            }
            // start tag
            let start = self.pos + 1;
            let mut i = start;
            while i < self.bytes.len() && is_name_char(self.bytes[i]) {
                i += 1;
            }
            let name = self.src[start..i].to_string();
            let mut attrs: Vec<(String, String)> = Vec::new();
            let mut self_closing = false;
            loop {
                while i < self.bytes.len() && self.bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                if i >= self.bytes.len() {
                    break;
                }
                if self.bytes[i] == b'>' {
                    i += 1;
                    break;
                }
                if self.bytes[i] == b'/' {
                    self_closing = true;
                    i += 1;
                    continue;
                }
                let astart = i;
                while i < self.bytes.len() && is_name_char(self.bytes[i]) {
                    i += 1;
                }
                let aname = self.src[astart..i].to_string();
                while i < self.bytes.len() && self.bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                let mut aval = String::new();
                if i < self.bytes.len() && self.bytes[i] == b'=' {
                    i += 1;
                    while i < self.bytes.len() && self.bytes[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    if i < self.bytes.len() && (self.bytes[i] == b'"' || self.bytes[i] == b'\'') {
                        let q = self.bytes[i];
                        i += 1;
                        let vstart = i;
                        while i < self.bytes.len() && self.bytes[i] != q {
                            i += 1;
                        }
                        aval = self.decode_entities(&self.src[vstart..i]);
                        i += 1;
                    } else {
                        let vstart = i;
                        while i < self.bytes.len() && !self.bytes[i].is_ascii_whitespace() && self.bytes[i] != b'>' {
                            i += 1;
                        }
                        aval = self.decode_entities(&self.src[vstart..i]);
                    }
                }
                if !aname.is_empty() {
                    attrs.push((aname, aval));
                }
            }
            self.pos = i;
            if !self_closing {
                self.stack.push(name.clone());
            }
            return Event::Start {
                name,
                attrs,
                self_closing,
            };
        }
    }

    /// Name of the currently open element (innermost).
    pub fn current(&self) -> Option<&str> {
        self.stack.last().map(|s| s.as_str())
    }

    pub fn stack(&self) -> &[String] {
        &self.stack
    }
}

fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else if b >> 3 == 0b1111_0 {
        4
    } else {
        1
    }
}

fn decode_one(ent: &str, entities: &HashMap<String, String>) -> Option<String> {
    if let Some(rest) = ent.strip_prefix('#') {
        let n = if let Some(hex) = rest.strip_prefix('x').or_else(|| rest.strip_prefix('X')) {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            rest.parse::<u32>().ok()?
        };
        return char::from_u32(n).map(|c| c.to_string());
    }
    match ent {
        "lt" => Some("<".into()),
        "gt" => Some(">".into()),
        "amp" => Some("&".into()),
        "quot" => Some("\"".into()),
        "apos" => Some("'".into()),
        other => entities.get(other).cloned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        let doc = r#"<?xml version="1.0"?>
<!DOCTYPE rules [ <!ENTITY a "x|y"> <!ENTITY b "&a;|z"> <!-- comment -->]>
<rules><rule id="1" name="n &amp; m"><pattern><token/></pattern>text &b; &lt;3</rule></rules>"#;
        let mut p = Parser::new(doc);
        let mut starts = Vec::new();
        let mut texts = Vec::new();
        loop {
            match p.next_event() {
                Event::Start { name, attrs, .. } => {
                    if name == "rule" {
                        assert_eq!(Event::attr(&attrs, "name"), Some("n & m"));
                    }
                    starts.push(name);
                }
                Event::Text(t) => texts.push(t),
                Event::End { .. } => {}
                Event::Eof => break,
            }
        }
        assert_eq!(starts, vec!["rules", "rule", "pattern", "token"]);
        assert!(texts.iter().any(|t| t == "text x|y|z <3"));
    }

    #[test]
    fn cdata() {
        let mut p = Parser::new("<a><![CDATA[<x>&y]]></a>");
        assert!(matches!(p.next_event(), Event::Start { .. }));
        assert_eq!(p.next_event(), Event::Text("<x>&y".into()));
    }
}

/// A tiny in-memory DOM node built on top of the pull parser. Used to load
/// rule files: building a tree for a 9 MB document takes ~150 ms but makes the
/// rule conversion code vastly simpler than hand-rolled state machines.
#[derive(Debug, Clone)]
pub struct Node {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<DomItem>,
}

#[derive(Debug, Clone)]
pub enum DomItem {
    Node(Node),
    Text(String),
}

impl Node {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    pub fn children_named<'a, 'b>(&'a self, name: &'b str) -> impl Iterator<Item = &'a Node> + use<'a, 'b> {
        let name = name.to_string();
        self.children.iter().filter_map(move |c| match c {
            DomItem::Node(n) if n.name == name => Some(n),
            _ => None,
        })
    }

    /// All descendant elements with `name` (depth-first order).
    pub fn descendants_named<'a, 'b>(&'a self, name: &'b str) -> impl Iterator<Item = &'a Node> + use<'a, 'b> {
        let name = name.to_string();
        fn rec<'a>(n: &'a Node, name: &str, out: &mut Vec<&'a Node>) {
            for c in &n.children {
                if let DomItem::Node(x) = c {
                    if x.name == name {
                        out.push(x);
                    }
                    rec(x, name, out);
                }
            }
        }
        let mut out = Vec::new();
        rec(self, &name, &mut out);
        out.into_iter()
    }

    pub fn first_child(&self, name: &str) -> Option<&Node> {
        self.children_named(name).next()
    }

    /// Concatenated direct text content (child *elements* are skipped; this is
    /// what `<token>` needs, since nested `<exception>` text must not become
    /// part of the token's surface form).
    pub fn text(&self) -> String {
        let mut s = String::new();
        for c in &self.children {
            if let DomItem::Text(t) = c {
                s.push_str(t);
            }
        }
        s
    }
}

/// Parse a whole XML document into a root node.
pub fn parse_dom(src: &str) -> Node {
    let mut p = Parser::new(src);
    let mut stack: Vec<Node> = vec![Node {
        name: "#document".into(),
        attrs: Vec::new(),
        children: Vec::new(),
    }];
    loop {
        match p.next_event() {
            Event::Start { name, attrs, self_closing } => {
                let node = Node {
                    name,
                    attrs,
                    children: Vec::new(),
                };
                if self_closing {
                    stack.last_mut().unwrap().children.push(DomItem::Node(node));
                } else {
                    stack.push(node);
                }
            }
            Event::End { .. } => {
                if stack.len() > 1 {
                    let n = stack.pop().unwrap();
                    stack.last_mut().unwrap().children.push(DomItem::Node(n));
                }
            }
            Event::Text(t) => {
                stack.last_mut().unwrap().children.push(DomItem::Text(t));
            }
            Event::Eof => break,
        }
    }
    stack.into_iter().next().unwrap()
}
