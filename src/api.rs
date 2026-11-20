//! Embedding API: load the engine once, check many texts, and serialise the
//! matches in LanguageTool's JSON shape.
//!
//! This is what a host application uses (a server sidecar through
//! `grammaria serve`, or a Rust program linking the crate directly), as
//! opposed to the one-shot CLI in `main.rs`.

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};

use crate::engine::pipeline::{load_french, French};
use crate::engine::token::Sentence;

/// Unit of the `offset`/`length` fields in the JSON output.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OffsetUnit {
    /// UTF-8 bytes, as Rust strings are indexed.
    Bytes,
    /// UTF-16 code units, as LanguageTool (Java) and JavaScript strings count.
    Utf16,
}

/// A problem found in a text.
#[derive(Clone, Debug)]
pub struct Match {
    /// Rule id (`ACCORD_SUJET_VERBE`), its number in the rule group, and
    /// both together (`ACCORD_SUJET_VERBE[3]`).
    pub rule_id: String,
    pub sub_id: Option<String>,
    pub full_rule_id: String,
    /// The rule's name, as its author described it.
    pub description: String,
    pub message: String,
    pub short_message: String,
    /// Proposed corrections, best first.
    pub suggestions: Vec<String>,
    /// Position in the text, in UTF-8 bytes.
    pub offset: usize,
    pub length: usize,
    pub category_id: String,
    pub category_name: String,
    /// `misspelling`, `grammar`, `typographical`, `style`, `uncategorized`...
    pub issue_type: String,
    /// The rule comes from a style rule file.
    pub is_style: bool,
    pub source_file: String,
    pub url: Option<String>,
    /// The sentence the problem is in.
    pub sentence: String,
}

/// A loaded checker: dictionaries, disambiguation and rules.
pub struct Engine {
    french: French,
}

/// The directory of LanguageTool's French resources inside a data directory:
/// `DIR/lt` (with this project's own rules in `DIR/grammaria`), or `DIR`
/// itself when it holds the files directly.
pub fn resource_dir(dir: &Path) -> PathBuf {
    let lt = dir.join("lt");
    if lt.join("grammar.xml").exists() {
        lt
    } else {
        dir.to_path_buf()
    }
}

impl Engine {
    /// Load the engine from a data directory: `lt/` holds LanguageTool's
    /// French resources (`french.dict`, `grammar.xml`, `disambiguation.xml`,
    /// ...) and `grammaria/` this project's own rule files.
    pub fn load(dir: &Path) -> std::io::Result<Engine> {
        Ok(Engine { french: load_french(&resource_dir(dir))? })
    }

    /// The underlying pipeline (analysis, disambiguation, rules).
    pub fn french(&self) -> &French {
        &self.french
    }

    /// The sentences of a text, tokenized, tagged and disambiguated.
    pub fn analyze(&self, text: &str) -> Vec<Sentence> {
        self.french.analyze(text)
    }

    /// Check a document. `style` keeps the matches of the style rule file
    /// (wordiness, repetitions, register); LanguageTool reports them by
    /// default.
    pub fn check(&self, text: &str, style: bool) -> Vec<Match> {
        let (sentences, found) = self.french.check_with_sentences(text);
        // Byte offset at which each sentence starts.
        let mut starts = Vec::with_capacity(sentences.len());
        let mut at = 0;
        for s in &sentences {
            starts.push(at);
            at += s.text.len();
        }
        let mut matches = Vec::with_capacity(found.len());
        for f in found {
            let info = self.french.checker.info(f.rule);
            if info.is_style_file && !style {
                continue;
            }
            let sentence = match starts.binary_search(&f.from_byte) {
                Ok(i) => i,
                Err(i) => i.saturating_sub(1),
            };
            matches.push(Match {
                rule_id: info.id.clone(),
                sub_id: info.sub_id.clone(),
                full_rule_id: info.full_id(),
                description: info.description.clone(),
                message: f.message,
                short_message: f.short_message,
                suggestions: f.suggestions,
                offset: f.from_byte,
                length: f.to_byte.saturating_sub(f.from_byte),
                category_id: info.category_id.clone(),
                category_name: info.category_name.clone(),
                issue_type: info.issue_type.clone(),
                is_style: info.is_style_file,
                source_file: info.source_file.clone(),
                url: info.url.clone(),
                sentence: sentences.get(sentence).map(|s| s.text.trim().to_string()).unwrap_or_default(),
            });
        }
        matches
    }

    /// Check a document and return the `matches` JSON array.
    pub fn check_json(&self, text: &str, style: bool, unit: OffsetUnit) -> String {
        matches_json(text, &self.check(text, style), unit)
    }

    /// Serve check requests until `input` reaches end of file.
    ///
    /// Protocol, one request at a time:
    ///
    /// ```text
    /// -> <id> <byte length>\n<that many bytes of UTF-8 text>
    /// <- {"id":<id>,"matches":[...]}\n        (offsets in UTF-16 code units)
    /// <- {"id":<id>,"error":"..."}\n          (text is not valid UTF-8)
    /// ```
    ///
    /// A `{"ready":true}` line is written first, once the data is loaded. A
    /// malformed header ends the loop: the stream can no longer be framed.
    pub fn serve<R: BufRead, W: Write>(&self, mut input: R, mut output: W, style: bool) -> std::io::Result<()> {
        writeln!(output, "{{\"ready\":true}}")?;
        output.flush()?;
        let mut header = String::new();
        loop {
            header.clear();
            if input.read_line(&mut header)? == 0 {
                return Ok(());
            }
            let line = header.trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split(' ');
            let (id, len) = match (
                parts.next().and_then(|s| s.parse::<u64>().ok()),
                parts.next().and_then(|s| s.parse::<usize>().ok()),
            ) {
                (Some(id), Some(len)) => (id, len),
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("malformed request header: {line:?}"),
                    ))
                }
            };
            let mut buf = vec![0u8; len];
            input.read_exact(&mut buf)?;
            match String::from_utf8(buf) {
                Ok(text) => writeln!(
                    output,
                    "{{\"id\":{id},\"matches\":{}}}",
                    self.check_json(&text, style, OffsetUnit::Utf16)
                )?,
                Err(_) => writeln!(output, "{{\"id\":{id},\"error\":\"text is not valid UTF-8\"}}")?,
            }
            output.flush()?;
        }
    }
}

pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Number of UTF-16 code units in `s`.
fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Largest char boundary of `text` that is `<= i`.
fn floor_boundary(text: &str, i: usize) -> usize {
    let mut i = i.min(text.len());
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The `matches` array of LanguageTool's `--json` response: each match carries
/// its `context`, `sentence`, `type` and full `rule` descriptor.
pub fn matches_json(text: &str, matches: &[Match], unit: OffsetUnit) -> String {
    let mut out = String::from("[");
    // Matches come sorted by offset: convert byte offsets to UTF-16 units
    // incrementally instead of rescanning the document for each match.
    let mut seen_bytes = 0usize;
    let mut seen_units = 0usize;
    for (i, m) in matches.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let start = floor_boundary(text, m.offset);
        let end = floor_boundary(text, m.offset + m.length).max(start);
        let (offset, length) = match unit {
            OffsetUnit::Bytes => (m.offset, m.length),
            OffsetUnit::Utf16 => {
                if start < seen_bytes {
                    seen_bytes = 0;
                    seen_units = 0;
                }
                seen_units += utf16_len(&text[seen_bytes..start]);
                seen_bytes = start;
                (seen_units, utf16_len(&text[start..end]))
            }
        };
        // `context` is a window around the match: LanguageTool pads it with the
        // surrounding sentence; we use the containing sentence, clamped to a
        // reasonable window so long sentences do not dominate the payload.
        let (ctx_start, ctx_end) = context_window(text, start);
        let ctx_text = text.get(ctx_start..ctx_end).unwrap_or("");
        let ctx_offset = match unit {
            OffsetUnit::Bytes => start.saturating_sub(ctx_start),
            OffsetUnit::Utf16 => utf16_len(text.get(ctx_start..start).unwrap_or("")),
        };
        let url = match &m.url {
            Some(u) => format!(",\"urls\":[{{\"value\":\"{}\"}}]", json_escape(u)),
            None => String::new(),
        };
        out.push_str(&format!(
            "{{\"message\":\"{}\",\"shortMessage\":\"{}\",\"replacements\":[{}],\"offset\":{},\"length\":{},\"context\":{{\"text\":\"{}\",\"offset\":{},\"length\":{}}},\"sentence\":\"{}\",\"type\":{{\"typeName\":\"{}\"}},\"rule\":{{\"id\":\"{}\",\"subId\":\"{}\",\"sourceFile\":\"{}\",\"description\":\"{}\",\"issueType\":\"{}\"{},\"category\":{{\"id\":\"{}\",\"name\":\"{}\"}}}}}}",
            json_escape(&m.message),
            json_escape(&m.short_message),
            m.suggestions.iter().map(|s| format!("{{\"value\":\"{}\"}}", json_escape(s))).collect::<Vec<_>>().join(","),
            offset,
            length,
            json_escape(ctx_text),
            ctx_offset,
            length,
            json_escape(&m.sentence),
            if m.issue_type == "misspelling" { "UnknownWord" } else { "Other" },
            json_escape(&m.rule_id),
            json_escape(m.sub_id.as_deref().unwrap_or("")),
            json_escape(&m.source_file),
            json_escape(&m.description),
            json_escape(&m.issue_type),
            url,
            json_escape(&m.category_id),
            json_escape(&m.category_name)
        ));
    }
    out.push(']');
    out
}

/// Context window used in the JSON `context` field: the containing sentence,
/// trimmed when it is longer than `CONTEXT_MAX` bytes.
fn context_window(text: &str, offset: usize) -> (usize, usize) {
    const CONTEXT_MAX: usize = 120;
    let bytes = text.as_bytes();
    let offset = offset.min(text.len());
    // Sentence start: after the previous terminal punctuation.
    let mut start = 0;
    for i in (0..offset).rev() {
        if matches!(bytes[i], b'.' | b'!' | b'?' | b'\n') {
            start = i + 1;
            break;
        }
    }
    while start < offset && bytes[start] == b' ' {
        start += 1;
    }
    let mut end = text.len();
    for (i, c) in text.bytes().enumerate().skip(offset) {
        if matches!(c, b'.' | b'!' | b'?' | b'\n') {
            end = i + 1;
            break;
        }
    }
    if end - start > CONTEXT_MAX {
        start = offset.saturating_sub(CONTEXT_MAX / 3);
        end = (start + CONTEXT_MAX).min(end);
        while start < offset && !text.is_char_boundary(start) {
            start += 1;
        }
        while end > offset && !text.is_char_boundary(end) {
            end -= 1;
        }
    }
    (start, end)
}

/// Read a whole stream as text (stdin for `grammaria check -`).
pub fn read_all<R: Read>(mut input: R) -> String {
    let mut s = String::new();
    input.read_to_string(&mut s).ok();
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(offset: usize, length: usize) -> Match {
        Match {
            rule_id: "R".into(),
            sub_id: Some("1".into()),
            full_rule_id: "R[1]".into(),
            description: "A rule".into(),
            message: "msg \"x\"".into(),
            short_message: String::new(),
            suggestions: vec!["a".into()],
            offset,
            length,
            category_id: "CAT".into(),
            category_name: "Cat".into(),
            issue_type: "grammar".into(),
            is_style: false,
            source_file: "grammar.xml".into(),
            url: None,
            sentence: String::new(),
        }
    }

    fn field(json: &str, name: &str, nth: usize) -> usize {
        let key = format!("\"{name}\":");
        let at = json.match_indices(&key).nth(nth).expect("field").0 + key.len();
        json[at..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap()
    }

    #[test]
    fn utf16_offsets_count_accents_as_one_and_emojis_as_two() {
        // "é" is 2 bytes / 1 unit, "😀" is 4 bytes / 2 units.
        let text = "é 😀 chère amie";
        let chere = text.find("chère").unwrap();
        let amie = text.find("amie").unwrap();
        let ms = [m(chere, "chère".len()), m(amie, 4)];

        let bytes = matches_json(text, &ms, OffsetUnit::Bytes);
        assert_eq!(field(&bytes, "offset", 0), 8);
        assert_eq!(field(&bytes, "length", 0), 6);

        let utf16 = matches_json(text, &ms, OffsetUnit::Utf16);
        assert_eq!(field(&utf16, "offset", 0), 5);
        assert_eq!(field(&utf16, "length", 0), 5);
        // Second match: offset, context.offset, then the next match's offset.
        assert_eq!(field(&utf16, "offset", 2), 11);
        assert_eq!(field(&utf16, "length", 2), 4);
    }

    #[test]
    fn unsorted_and_out_of_range_matches_do_not_panic() {
        let text = "été 😀";
        // "été" is 5 bytes; the emoji spans bytes 6..10. The last two matches
        // point past the end and inside the emoji.
        let ms = [m(6, 4), m(0, 3), m(400, 2), m(7, 1)];
        let json = matches_json(text, &ms, OffsetUnit::Utf16);
        assert_eq!(field(&json, "offset", 0), 4);
        assert_eq!(field(&json, "length", 0), 2);
        assert_eq!(field(&json, "offset", 2), 0);
        assert_eq!(field(&json, "length", 2), 2);
    }

    #[test]
    fn json_strings_are_escaped() {
        let json = matches_json("a", &[m(0, 1)], OffsetUnit::Bytes);
        assert!(json.contains("\"message\":\"msg \\\"x\\\"\""));
    }
}
