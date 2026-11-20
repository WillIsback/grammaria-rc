//! Reading the plain-text word lists of the spell checker.
//!
//! Port of `org.languagetool.rules.spelling.CachingWordListLoader`, of the
//! line reader of `MorfologikMultiSpeller` (`getLines`) and of
//! `French.prepareLineForSpeller` (LanguageTool, LGPL-2.1-or-later).

use std::path::Path;

use super::strings::{java_trim, substring_before};

/// `String.split(regex)` for a single-character class: trailing empty strings
/// are dropped.
fn java_split_any<'a>(s: &'a str, seps: &[char]) -> Vec<&'a str> {
    if s.is_empty() {
        return vec![""];
    }
    let mut parts: Vec<&str> = s.split(|c| seps.contains(&c)).collect();
    while parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    parts
}

/// The text of a resource file, `None` when it does not exist
/// (`ResourceDataBroker.resourceExists`).
pub fn read_resource(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// `CachingWordListLoader.loadWords` on the text of a file: one entry per
/// line, without comments (`#`); a missing file gives no words.
pub fn load_words(content: Option<&str>) -> Vec<String> {
    let Some(content) = content else { return Vec::new() };
    let mut result = Vec::new();
    for line in java_lines(content) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        result.push(java_trim(substring_before(java_trim(line), '#')).to_string());
    }
    result
}

/// `BufferedReader.readLine` over a whole text: lines end with `\n`, `\r` or
/// `\r\n`; a final line without terminator counts, an empty rest does not.
pub fn java_lines(content: &str) -> impl Iterator<Item = &str> {
    let mut rest = content;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end = rest.find(['\n', '\r']).unwrap_or(rest.len());
        let line = &rest[..end];
        let mut next = end;
        if rest[next..].starts_with("\r\n") {
            next += 2;
        } else if next < rest.len() {
            next += 1;
        }
        rest = &rest[next..];
        Some(line)
    })
}

/// `French.prepareLineForSpeller`: of a `form<TAB or ;>tag` line (the format
/// of `multiwords.txt`), keep the form when it is a proper noun, a noun or an
/// adverb; a line without tag is kept whole.
pub fn prepare_line_for_speller(line: &str) -> String {
    const SPELLER_EXCEPTIONS: [&str; 1] = ["Ho Chi Minh"];
    let parts = java_split_any(line, &['#']);
    if parts.is_empty() {
        return line.to_string();
    }
    let form_tag = java_split_any(parts[0], &['\t', ';']);
    let form = java_trim(form_tag.first().copied().unwrap_or(""));
    if SPELLER_EXCEPTIONS.contains(&form) {
        return String::new();
    }
    if form_tag.len() > 1 {
        let tag = java_trim(form_tag[1]);
        return if tag.starts_with('Z') || tag.starts_with('N') || tag == "A" {
            form.to_string()
        } else {
            String::new()
        };
    }
    line.to_string()
}

/// `MorfologikMultiSpeller`'s `getLines`: the entries of the plain-text
/// dictionaries, read as one stream. Java chains the files with a
/// `SequenceInputStream`, so a file that does not end with a line break runs
/// into the first line of the next one; `content` is that concatenation.
pub fn speller_lines(content: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for original in java_lines(content) {
        let line = prepare_line_for_speller(original);
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let line = java_trim(java_split_any(&line, &['#']).first().copied().unwrap_or(""));
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_lists() {
        let words = load_words(Some("# comment\nAlexi\n\nJoe Biden #name\n  eBay  \nlast"));
        assert_eq!(words, ["Alexi", "Joe Biden", "eBay", "last"]);
        assert!(load_words(None).is_empty());
    }

    #[test]
    fn lines_for_the_speller() {
        assert_eq!(prepare_line_for_speller("Intel Core; Z m sp"), "Intel Core");
        assert_eq!(prepare_line_for_speller("home page;N f s"), "home page");
        assert_eq!(prepare_line_for_speller("à peu près\tA"), "à peu près");
        assert_eq!(prepare_line_for_speller("a priori;J e sp"), "");
        assert_eq!(prepare_line_for_speller("Ho Chi Minh"), "");
        assert_eq!(prepare_line_for_speller("Gilberto #name"), "Gilberto #name");
        // Two files chained without a line break in between.
        let lines = speller_lines("# header\nAlexi\nJoe Biden #name\nShabak# Add terms here\n\nb2\n");
        assert_eq!(lines, ["Alexi", "Joe Biden", "Shabak", "b2"]);
    }

    #[test]
    fn line_endings() {
        let l: Vec<&str> = java_lines("a\r\nb\rc\n\nd").collect();
        assert_eq!(l, ["a", "b", "c", "", "d"]);
        assert_eq!(java_lines("a\n").count(), 1);
    }
}
