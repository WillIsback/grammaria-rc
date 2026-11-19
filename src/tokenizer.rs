//! French tokenizer, modelled on LanguageTool's `FrenchWordTokenizer`.
//!
//! Produces words (including elided forms like `l'` / `j'`, hyphenated forms
//! like `dit-il`, apostrophe compounds like `aujourd’hui`), numbers,
//! punctuation and whitespace, with token types that the rule engine relies on
//! (`WORD`, `WORD_DIGIT`, `WORD_MIXED_CASE`, `NUMBER`, `PUNCT`, ...).
//!
//! Tokenization is a small hand-written state machine; it is allocation-light
//! (one `String` per token) and handles typographic apostrophes/quotes.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenType {
    Word,
    WordDigit,
    WordMixedCase,
    HyphenatedWord,
    Number,
    Punctuation,
    SentenceStart,
    SentenceEnd,
    Whitespace,
    Unknown,
    Url,
    Email,
}

impl TokenType {
    pub fn as_str(self) -> &'static str {
        match self {
            TokenType::Word => "WORD",
            TokenType::WordDigit => "WORD_DIGIT",
            TokenType::WordMixedCase => "WORD_MIXED_CASE",
            TokenType::HyphenatedWord => "HYPHENATED_WORD",
            TokenType::Number => "NUMBER",
            TokenType::Punctuation => "PUNCT",
            TokenType::SentenceStart => "SENT_START",
            TokenType::SentenceEnd => "SENT_END",
            TokenType::Whitespace => "WHITESPACE",
            TokenType::Unknown => "UNKNOWN",
            TokenType::Url => "URL",
            TokenType::Email => "EMAIL",
        }
    }

    /// Tokens that `<token>` elements can match. `SENT_START`, `SENT_END` and
    /// whitespace are structural and handled separately by the engine.
    pub fn is_matchable(self) -> bool {
        !matches!(
            self,
            TokenType::SentenceStart | TokenType::SentenceEnd | TokenType::Whitespace
        )
    }
}

/// A token with its byte range in the sentence.
#[derive(Clone, Debug)]
pub struct Token {
    pub text: String,
    /// Byte offset relative to the sentence start.
    pub start: usize,
    pub end: usize,
    pub kind: TokenType,
    /// Number of whitespace characters directly before the token.
    pub spaces_before: u32,
    pub has_newline_before: bool,
}

impl Token {
    pub fn new(text: &str, start: usize, end: usize, kind: TokenType) -> Token {
        Token {
            text: text.to_string(),
            start,
            end,
            kind,
            spaces_before: 0,
            has_newline_before: false,
        }
    }
}

pub fn is_apostrophe(c: char) -> bool {
    c == '\'' || c == '\u{2019}' || c == '\u{2018}' || c == '\u{02BC}' || c == '\u{00B4}'
}

fn is_hyphen(c: char) -> bool {
    c == '-' || c == '\u{2010}' || c == '\u{2011}' || c == '\u{2013}'
}

fn is_sentence_end(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '\u{2026}' | ';')
}

const KNOWN_ABBREVIATIONS: &[&str] = &[
    "m.", "mm.", "mme", "mmes", "mlle", "mlles", "dr.", "pr.", "st.", "ste.", "cf.", "p.", "pp.",
    "ex.", "env.", "av.", "bd.", "vol.", "chap.", "fig.", "art.", "op.", "etc.", "i.e.", "e.g.",
    "c.-à-d.",
];

/// Tokenize `text`; byte offsets are relative to `text`.
pub fn tokenize(text: &str) -> Vec<Token> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let n = chars.len();
    let mut out: Vec<Token> = Vec::with_capacity(n / 4 + 8);
    let mut i = 0usize;
    let mut spaces: u32 = 0;
    let mut newline = false;
    let byte_at = |s: usize, e: usize| -> (usize, usize) {
        let bs = if s < n { chars[s].0 } else { text.len() };
        let be = if e < n { chars[e].0 } else { text.len() };
        (bs, be)
    };
    let mut pending: Option<(usize, usize, TokenType)> = None;
    while i < n {
        let c = chars[i].1;
        if c.is_whitespace() {
            spaces += 1;
            if c == '\n' {
                newline = true;
            }
            i += 1;
            continue;
        }
        if let Some((s, e, kind)) = pending.take() {
            let (bs, be) = byte_at(s, e);
            let mut t = Token::new(&text[bs..be], bs, be, kind);
            t.spaces_before = spaces;
            t.has_newline_before = newline;
            out.push(t);
            spaces = 0;
            newline = false;
        }
        // URL fast path
        {
            let rest = &text[chars[i].0..];
            if rest.starts_with("http://") || rest.starts_with("https://") || rest.starts_with("www.") {
                let mut k = i;
                while k < n && !chars[k].1.is_whitespace() {
                    k += 1;
                }
                pending = Some((i, k, TokenType::Url));
                i = k;
                continue;
            }
        }
        if c.is_alphabetic() {
            let start = i;
            let mut k = i;
            let mut hyphen_inside = false;
            let mut apostrophe_inside = false;
            let mut mixed_case = false;
            let mut saw_upper = false;
            let mut saw_lower = false;
            let mut inner_dot = false;
            while k < n {
                let ch = chars[k].1;
                if ch.is_alphabetic() {
                    if ch.is_uppercase() {
                        saw_upper = true;
                    } else if ch.is_lowercase() {
                        saw_lower = true;
                    }
                    if saw_upper && saw_lower {
                        mixed_case = true;
                    }
                    k += 1;
                    continue;
                }
                if is_hyphen(ch) {
                    if k + 1 < n && (chars[k + 1].1.is_alphanumeric() || is_apostrophe(chars[k + 1].1)) {
                        hyphen_inside = true;
                        k += 1;
                        continue;
                    }
                    break;
                }
                if is_apostrophe(ch) && k + 1 < n && chars[k + 1].1.is_alphabetic() {
                    let prefix = &text[chars[start].0..chars[k].0];
                    if prefix.chars().count() <= 2 {
                        // elision: `l'`, `j'`, `qu'`, `n'` keep the apostrophe
                        // at the end of the token
                        k += 1;
                        apostrophe_inside = true;
                        break;
                    }
                    apostrophe_inside = true;
                    k += 1;
                    continue;
                }
                if ch == '.' && k + 1 < n && chars[k + 1].1.is_alphabetic() {
                    let prefix = &text[chars[start].0..chars[k].0];
                    if dotted_ok(prefix) && prefix.chars().count() <= 3 && !prefix.contains('\'') {
                        inner_dot = true;
                        k += 1;
                        continue;
                    }
                }
                break;
            }
            if k == start {
                pending = Some((i, i + 1, TokenType::Punctuation));
                i += 1;
                continue;
            }
            // A capitalised-at-start word with inner punctuation ("Les",
            // "L'Humanité", "Royaume-Uni") is a normal WORD; WORD_MIXED_CASE
            // is reserved for inner powerCase like "iPhone" / "McDonald".
            let first_upper = chars[start].1.is_uppercase();
            let inner_cmpd = apostrophe_inside || inner_dot;
            let kind = if hyphen_inside && mixed_case {
                TokenType::WordMixedCase
            } else if hyphen_inside {
                TokenType::HyphenatedWord
            } else if mixed_case && !first_upper && !inner_cmpd {
                TokenType::WordMixedCase
            } else {
                TokenType::Word
            };
            pending = Some((start, k, kind));
            i = k;
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            let mut k = i;
            while k < n {
                let ch = chars[k].1;
                if ch.is_ascii_digit() || ch.is_alphabetic() {
                    k += 1;
                } else if (ch == ' ' || ch == '\u{00A0}' || ch == '\u{202F}') && k + 1 < n && chars[k + 1].1.is_ascii_digit() {
                    k += 1;
                } else if ch == ',' && k + 1 < n && chars[k + 1].1.is_ascii_digit() {
                    k += 1;
                } else if ch == '.' && k + 1 < n && chars[k + 1].1.is_ascii_digit() {
                    k += 1;
                } else {
                    break;
                }
            }
            let has_letter = chars[start..k].iter().any(|&(_, ch)| ch.is_alphabetic());
            let kind = if has_letter { TokenType::WordDigit } else { TokenType::Number };
            pending = Some((start, k, kind));
            i = k;
            continue;
        }
        // punctuation run
        let start = i;
        let mut k = i;
        while k < n {
            let ch = chars[k].1;
            if ch.is_alphanumeric() || ch.is_whitespace() || is_apostrophe(ch) || is_hyphen(ch) {
                break;
            }
            k += 1;
        }
        if k == start {
            k = start + 1;
        }
        pending = Some((start, k, TokenType::Punctuation));
        i = k;
    }
    if let Some((s, e, kind)) = pending {
        let (bs, be) = byte_at(s, e);
        let mut t = Token::new(&text[bs..be], bs, be, kind);
        t.spaces_before = spaces;
        t.has_newline_before = newline;
        out.push(t);
    }
    out
}

/// A dot may continue a token only when every dot-separated segment so far is
/// alphanumeric (acronym style) -- otherwise a sentence-final dot would be
/// swallowed.
fn dotted_ok(prefix: &str) -> bool {
    prefix.split('.').all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_alphanumeric()))
}

/// Split a text block into sentences (byte ranges). A sentence ends after
/// `.`, `!`, `?`, `…` (and `;` before an uppercase word) followed by
/// whitespace, unless the period closes a known abbreviation, an ordinal or is
/// part of a dotted acronym.
pub fn sentence_boundaries(text: &str) -> Vec<(usize, usize)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let n = chars.len();
    let mut bounds = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let byte_at = |idx: usize| -> usize {
        if idx < n {
            chars[idx].0
        } else {
            text.len()
        }
    };
    while i < n {
        let c = chars[i].1;
        if c == '\n' {
            if byte_at(i) > byte_at(start) {
                bounds.push((byte_at(start), byte_at(i)));
            }
            start = i + 1;
            i += 1;
            continue;
        }
        if !is_sentence_end(c) {
            i += 1;
            continue;
        }
        let mut k = i;
        while k < n {
            let ch = chars[k].1;
            if is_sentence_end(ch)
                || matches!(ch, '"' | '\u{00AB}' | '\u{00BB}' | '\u{201D}' | '\u{201C}' | ')' | ']' | '\u{2019}')
            {
                k += 1;
            } else {
                break;
            }
        }
        let mut j = k;
        while j < n && (chars[j].1 == ' ' || chars[j].1 == '\t' || chars[j].1 == '\u{00A0}') {
            j += 1;
        }
        let at_end = j >= n || chars[j].1 == '\n';
        let next_ok = at_end
            || chars[j].1.is_uppercase()
            || chars[j].1.is_ascii_digit()
            || matches!(chars[j].1, '\u{00AB}' | '"' | '\u{201C}' | '(' | '\u{2014}' | '-' | '*' | '\u{2019}');
        let only_semicolon = (i..k).all(|x| chars[x].1 == ';');
        if only_semicolon && !next_ok {
            i = k;
            continue;
        }
        if c == '.' && !only_semicolon {
            let mut ws = i;
            while ws > start && (chars[ws - 1].1.is_alphanumeric() || chars[ws - 1].1 == '.') {
                ws -= 1;
            }
            let word: String = chars[ws..i].iter().map(|&(_, ch)| ch).collect();
            if !word.is_empty() && word.chars().all(|ch| ch.is_ascii_digit() || ch == '.') {
                i = k;
                continue;
            }
            let lower = word.to_lowercase();
            let with_dot = format!("{}.", lower);
            if KNOWN_ABBREVIATIONS.iter().any(|a| lower == *a || with_dot == *a) {
                i = k;
                continue;
            }
            if word.chars().count() == 1 && word.chars().all(|ch| ch.is_uppercase()) && !at_end {
                i = k;
                continue;
            }
            if word.contains('.') && word.split('.').all(|s| s.chars().count() <= 1) {
                i = k;
                continue;
            }
        }
        if next_ok {
            bounds.push((byte_at(start), byte_at(k)));
            start = k;
            i = k;
            continue;
        }
        i = k;
    }
    if start < n {
        bounds.push((byte_at(start), text.len()));
    }
    bounds
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(s: &str) -> Vec<String> {
        tokenize(s).into_iter().map(|t| t.text).collect()
    }

    #[test]
    fn words() {
        assert_eq!(
            texts("Je m'appelle Jean-Ursule, j'ai 2 chats."),
            vec!["Je", "m'", "appelle", "Jean-Ursule", ",", "j'", "ai", "2", "chats", "."]
        );
        assert_eq!(texts("l'homme"), vec!["l'", "homme"]);
        assert_eq!(texts("aujourd'hui"), vec!["aujourd'hui"]);
        assert_eq!(texts("dit-il"), vec!["dit-il"]);
    }

    #[test]
    fn sentences() {
        let t = "Bonjour. Comment allez-vous ? M. Dupont est là. Il a 2 ans.";
        let parts: Vec<&str> = sentence_boundaries(t).iter().map(|&(a, b)| &t[a..b]).collect();
        assert_eq!(
            parts,
            vec!["Bonjour.", " Comment allez-vous ?", " M. Dupont est là.", " Il a 2 ans."]
        );
    }

    #[test]
    fn types() {
        let t = tokenize("Les 2CV roulent vite, 3 km/h.");
        assert_eq!(t[0].kind, TokenType::Word);
        assert_eq!(t[1].kind, TokenType::WordDigit);
        assert_eq!(t[2].kind, TokenType::Word);
        assert_eq!(t[3].kind, TokenType::Word);
        assert_eq!(t[5].kind, TokenType::Number);
        assert_eq!(tokenize("iPhone")[0].kind, TokenType::WordMixedCase);
    }

    #[test]
    fn urls() {
        let t = tokenize("voir https://example.com/a ici");
        assert_eq!(t[1].kind, TokenType::Url);
    }
}
