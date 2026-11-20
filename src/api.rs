//! Embedding API: load the engine once, check many texts, and serialise the
//! matches in LanguageTool's JSON shape.
//!
//! This is what a host application uses (a server sidecar through
//! `grammaria serve`, or a Rust program linking the crate directly), as
//! opposed to the one-shot CLI in `main.rs`.

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};

use crate::rules::{RuleMatch, RuleSet};
use crate::tagger::Tagger;

/// Unit of the `offset`/`length` fields in the JSON output.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OffsetUnit {
    /// UTF-8 bytes, as the rule engine reports them.
    Bytes,
    /// UTF-16 code units, as LanguageTool (Java) and JavaScript strings count.
    Utf16,
}

/// A loaded checker: tagger (lexicon, word lists, disambiguation) and rules.
pub struct Engine {
    tagger: &'static Tagger,
    rules: RuleSet,
}

/// File names accepted for the disambiguation rules inside a data directory.
const DISAMBIG_NAMES: [&str; 2] = ["fr.disambig.xml", "languagetool-disambiguation.xml"];

impl Engine {
    /// Load the engine from a data directory holding `fr.lex`, `fr.words`,
    /// `fr.rules` and the disambiguation XML (`fr.disambig.xml`).
    ///
    /// The lexicon is process-global: a second `load` reuses the first one.
    pub fn load(dir: &Path) -> std::io::Result<Engine> {
        let disambig: Option<PathBuf> = DISAMBIG_NAMES
            .iter()
            .map(|n| dir.join(n))
            .find(|p| p.exists());
        let tagger = Tagger::new(&dir.join("fr.lex"), &dir.join("fr.words"), disambig.as_deref())?;
        let rules = RuleSet::load_cache(dir.join("fr.rules")).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("missing or invalid rule cache: {}", dir.join("fr.rules").display()),
            )
        })?;
        Ok(Engine { tagger, rules })
    }

    /// Build an engine from already loaded parts (the CLI's XML fallback).
    pub fn from_parts(tagger: &'static Tagger, rules: RuleSet) -> Engine {
        Engine { tagger, rules }
    }

    pub fn tagger(&self) -> &'static Tagger {
        self.tagger
    }

    /// Check a document. Offsets of the returned matches are UTF-8 bytes.
    pub fn check(&self, text: &str, style: bool) -> Vec<RuleMatch> {
        crate::rules::check_text(self.tagger, &self.rules, text, style)
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
pub fn matches_json(text: &str, matches: &[RuleMatch], unit: OffsetUnit) -> String {
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
        out.push_str(&format!(
            "{{\"message\":\"{}\",\"shortMessage\":\"{}\",\"replacements\":[{}],\"offset\":{},\"length\":{},\"context\":{{\"text\":\"{}\",\"offset\":{},\"length\":{}}},\"sentence\":\"{}\",\"type\":{{\"typeName\":\"{}\"}},\"rule\":{{\"id\":\"{}\",\"subId\":\"{}\",\"sourceFile\":\"{}\",\"description\":\"{}\",\"issueType\":\"{}\",\"category\":{{\"id\":\"{}\",\"name\":\"{}\"}}}}}}",
            json_escape(&m.message),
            json_escape(&m.short_message),
            m.suggestions
                .iter()
                .map(|s| format!("{{\"value\":\"{}\"}}", json_escape(s)))
                .collect::<Vec<_>>()
                .join(","),
            offset,
            length,
            json_escape(ctx_text),
            ctx_offset,
            length,
            json_escape(&m.sentence),
            if m.is_style { "style" } else { "Other" },
            json_escape(&m.rule_id),
            json_escape(&m.full_rule_id),
            if m.is_style { "style.xml" } else { "grammar.xml" },
            json_escape(&m.category_name),
            if m.is_style { "style" } else { "uncategorized" },
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

    fn m(offset: usize, length: usize) -> RuleMatch {
        RuleMatch {
            rule_id: "R".into(),
            full_rule_id: "R[1]".into(),
            message: "msg \"x\"".into(),
            short_message: String::new(),
            suggestions: vec!["a".into()],
            offset,
            length,
            category_id: "CAT".into(),
            category_name: "Cat".into(),
            priority: crate::rules::Priority::Medium,
            is_style: false,
            urls: Vec::new(),
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
