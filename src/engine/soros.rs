//! Spelling numbers out (`7` → `sept`): the interpreter of LanguageTool's
//! `Soros` number-to-text programs (`fr.sor`), a port of
//! `org.languagetool.synthesis.Soros` (itself from the Numbertext project).
//!
//! A program is a list of `pattern replacement` lines; the first pattern
//! that matches the whole input rewrites it, and `$(...)` calls in the
//! result are evaluated recursively.

use crate::re::{Flags, Regex};

const M: &str = "\\\";#";
const M2: &str = "$()|[]";
const C: &str = "\u{E000}\u{E001}\u{E002}\u{E003}";
const C2: &str = "\u{E004}\u{E005}\u{E006}\u{E007}\u{E008}\u{E009}";
const PIPE: char = '\u{E003}';

pub struct Soros {
    patterns: Vec<Regex>,
    values: Vec<String>,
    begins: Vec<bool>,
    ends: Vec<bool>,
    func: Regex,
}

/// `translate`: replaces `delim + chars[i]` with `chars2[i]`.
fn translate(s: &str, chars: &str, chars2: &str, delim: &str) -> String {
    let mut s = s.to_string();
    for (a, b) in chars.chars().zip(chars2.chars()) {
        s = s.replace(&format!("{delim}{a}"), &b.to_string());
    }
    s
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern, Flags::default()).expect("valid pattern")
}

fn replace_all(s: &str, pattern: &str, replacement: &str) -> String {
    re(pattern).replace_all(s, replacement)
}

fn replace_first(s: &str, pattern: &str, replacement: &str) -> String {
    re(pattern).replace_first(s, replacement)
}

fn chars(s: &str) -> Vec<char> {
    s.chars().collect()
}

fn text(c: &[char], span: Option<(usize, usize)>) -> String {
    span.map(|(a, b)| c[a..b].iter().collect())
        .unwrap_or_default()
}

impl Soros {
    /// Compiles a program for a language code (`fr`, `fr-CH`, `Roman`...).
    pub fn new(source: &str, lang: &str) -> Soros {
        let c_len = C.chars().count();
        let m2_head: String = M2.chars().take(c_len).collect();
        let func = re(&translate(
            r"(?:\|?(?:\$\()+)?(\|?\$\(([^\(\)]*)\)\|?)(?:\)+\|?)?",
            &m2_head,
            C,
            "\\",
        ));

        let mut source = translate(source, M, C, "\\");
        // Switch off all country-dependent lines, and on the requested ones.
        source = replace_all(
            &source,
            "(^|[\n;])([^\n;#]*#[^\n]*\\[:[^\n:\\]]*:][^\n]*)",
            "$1#$2",
        );
        source = replace_all(
            &source,
            &format!(
                "(^|[\n;])#([^\n;#]*#[^\n]*\\[:{}:][^\n]*)",
                lang.replace('_', "-")
            ),
            "$1$2",
        );
        source = replace_all(&source, "(#[^\n]*)?(\n|$)", ";");
        if !source.contains("__numbertext__") {
            source = format!("__numbertext__;{source}");
        }
        source = source.replace(
            "__numbertext__",
            // default left zero deletion; separator function; no separation
            // if the subcall returns an empty string
            "\"([a-z][-a-z]* )?0+(0|[1-9]\\d*)\" $(\\1\\2);\"\u{E00A}(.*)\u{E00A}(.+)\u{E00A}(.*)\" \\1\\2\\3;\"\u{E00A}.*\u{E00A}\u{E00A}.*\"",
        );

        let line = re("^\\s*(\"[^\"]*\"|[^\\s]*)\\s*(.*[^\\s])?\\s*$");
        let macro_line = re("== *(.*[^ ]?) ==");
        let unquote = |s: &str| replace_first(&replace_first(s, "^\"", ""), "\"$", "");
        let mut soros = Soros {
            patterns: Vec::new(),
            values: Vec::new(),
            begins: Vec::new(),
            ends: Vec::new(),
            func,
        };
        let mut prefix = String::new();
        for part in source.split(';') {
            let mut s = part.to_string();
            if let Some(m) = macro_line.full_match_chars(&chars(&s)) {
                prefix = text(&chars(&s), m.spans[1]);
                continue;
            }
            let mut c = chars(&s);
            let mut sp = line.full_match_chars(&c);
            if !prefix.is_empty() && !s.is_empty() {
                if let Some(m) = &sp {
                    let first = unquote(&text(&c, m.spans[1]));
                    let second = m.spans[2]
                        .map(|_| text(&c, m.spans[2]))
                        .unwrap_or_else(|| "null".to_string());
                    s = format!(
                        "\"{}{}{}{}\" {}",
                        if first.starts_with('^') { "^" } else { "" },
                        prefix,
                        if first.is_empty() { "" } else { " " },
                        replace_first(&first, "^\\^", ""),
                        second
                    );
                    c = chars(&s);
                    sp = line.full_match_chars(&c);
                }
            }
            let Some(m) = sp else { continue };
            if s.is_empty() {
                continue;
            }
            let c_tail: String = C.chars().skip(1).collect();
            let m_tail: String = M.chars().skip(1).collect();
            let mut pattern = translate(&unquote(&text(&c, m.spans[1])), &c_tail, &m_tail, "");
            pattern = pattern.replace('\u{E000}', "\\\\");
            let mut value = if m.spans[2].is_some() {
                unquote(&text(&c, m.spans[2]))
            } else {
                String::new()
            };
            value = translate(&value, M2, C2, "\\");
            // Inner separator calls: [ ... $1 ... ] -> $(sep ... sep $1 sep ... )
            value = replace_all(
                &value,
                "^\\[[$](\\d\\d?|\\([^\\)]+\\))",
                "\\$(\u{E00A}\u{E00A}|\\$$1\u{E00A}",
            );
            value = replace_all(
                &value,
                "\\[([^$\\[\\\\]*)[$](\\d\\d?|\\([^\\)]+\\))",
                "\\$(\u{E00A}$1\u{E00A}\\$$2\u{E00A}",
            );
            value = replace_all(&value, "\u{E00A}\\]$", "|\u{E00A})");
            value = value.replace(']', ")");
            value = replace_all(&value, "(\\$\\d|\\))\\|\\$", "$1||\\$");
            value = translate(&value, C, M, "");
            value = translate(&value, &m2_head, C, "");
            value = translate(&value, C2, M2, "");
            value = replace_all(&value, "[$]", "\\$");
            value = replace_all(&value, "\u{E000}(\\d)", "\u{E000}\u{E001}\\$$1\u{E002}");
            value = replace_all(&value, "\\\\(\\d)", "\\$$1");
            value = value.replace("\\n", "\n");
            let body = replace_first(&replace_first(&pattern, "^\\^", ""), "\\$$", "");
            let Ok(compiled) = Regex::new(&format!("^{body}$"), Flags::default()) else {
                continue;
            };
            soros.patterns.push(compiled);
            soros.begins.push(pattern.starts_with('^'));
            soros.ends.push(pattern.ends_with('$'));
            soros.values.push(value);
        }
        soros
    }

    /// The text of a number (or of whatever the program rewrites).
    pub fn run(&self, input: &str) -> String {
        self.run_with(input, true, true, 0)
    }

    fn run_with(&self, input: &str, begin: bool, end: bool, depth: usize) -> String {
        if depth > 64 {
            return String::new();
        }
        let input_chars = chars(input);
        for i in 0..self.patterns.len() {
            if (!begin && self.begins[i]) || (!end && self.ends[i]) {
                continue;
            }
            if self.patterns[i].full_match_chars(&input_chars).is_none() {
                continue;
            }
            let mut s = self.patterns[i].replace_all(input, &self.values[i]);
            loop {
                let c = chars(&s);
                let Some(n) = self.func.find_chars(&c, 0) else {
                    break;
                };
                let (start, stop) = n.spans[0].unwrap();
                let (g1_start, g1_stop) = n.spans[1].unwrap();
                let whole = &c[start..stop];
                let group1 = &c[g1_start..g1_stop];
                let b = if group1.first() == Some(&PIPE) || whole.first() == Some(&PIPE) {
                    true
                } else if start == 0 {
                    begin
                } else {
                    false
                };
                let e = if group1.last() == Some(&PIPE) || whole.last() == Some(&PIPE) {
                    true
                } else if stop == c.len() {
                    end
                } else {
                    false
                };
                let inner = self.run_with(&text(&c, n.spans[2]), b, e, depth + 1);
                s = format!(
                    "{}{}{}",
                    c[..g1_start].iter().collect::<String>(),
                    inner,
                    c[g1_stop..].iter().collect::<String>()
                );
            }
            return s;
        }
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn french_numbers() {
        let Ok(source) = std::fs::read_to_string("assets/fr/lt/fr.sor") else {
            return;
        };
        let soros = Soros::new(&source, "fr");
        for (number, words) in [
            ("0", "zéro"),
            ("7", "sept"),
            ("21", "vingt et un"),
            ("71", "soixante et onze"),
            ("80", "quatre-vingts"),
            ("81", "quatre-vingt-un"),
            ("91", "quatre-vingt-onze"),
            ("100", "cent"),
            ("200", "deux cents"),
            ("201", "deux cent un"),
            ("1234", "mille deux cent trente-quatre"),
            ("2000000", "deux millions"),
            ("007", "sept"),
        ] {
            assert_eq!(soros.run(number), words, "{number}");
        }
        assert_eq!(Soros::new(&source, "fr-CH").run("71"), "septante et un");
    }
}
