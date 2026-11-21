//! Word tokenizing.
//!
//! Port of `org.languagetool.tokenizers.WordTokenizer` and
//! `org.languagetool.tokenizers.fr.FrenchWordTokenizer` (LanguageTool,
//! LGPL-2.1-or-later).
//!
//! A sentence is cut into words, punctuation marks and whitespace; every
//! character of the sentence ends up in exactly one token. E-mail addresses
//! and URLs are put back together into one token. The French tokenizer keeps
//! elisions (`l'`), clitics (`-t-il`), decimal numbers and digit groups in the
//! shape the French dictionary and rules expect.

use std::sync::OnceLock;

use crate::re::{Flags, Regex};

use super::strtools;

fn regex(src: &str, flags: Flags) -> Regex {
    Regex::new(src, flags).unwrap_or_else(|e| panic!("invalid built-in pattern {src}: {}", e.0))
}

// ---------------------------------------------------------------------------
// WordTokenizer
// ---------------------------------------------------------------------------

const PROTOCOLS: [&str; 19] = [
    "http", "https", "ws", "wss", "ftp", "ftps", "sftp", "file", "mailto", "tel", "sms", "git",
    "ssh", "data", "magnet", "smb", "slack", "spotify", "magnet",
];

/// `WordTokenizer.TOKENIZING_CHARACTERS`.
const TOKENIZING_CHARACTERS: &str = concat!(
    "\u{0020}\u{00A0}\u{115f}\u{1160}\u{1680}",
    "\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}",
    "\u{2008}\u{2009}\u{200A}\u{200B}\u{200c}\u{200d}\u{200e}\u{200f}",
    "\u{2028}\u{2029}\u{202a}\u{202b}\u{202c}\u{202d}\u{202e}\u{202f}",
    "\u{205F}\u{2060}\u{2061}\u{2062}\u{2063}\u{206A}\u{206b}\u{206c}\u{206d}",
    "\u{206E}\u{206F}\u{3000}\u{3164}\u{feff}\u{ffa0}\u{fff9}\u{fffa}\u{fffb}",
    "¦‖∣|,.;()[]{}=*#∗+×·÷<>!?:~/\\\"'«»„”“‘’`´‛′›‹…¿¡‼⁇⁈⁉™®\u{203d}\u{00B6}\u{FFEB}\u{2E2E}",
    "\u{2012}\u{2013}\u{2014}\u{2015}",
    "\u{2500}\u{3161}\u{2713}",
    "\u{25CF}\u{25CB}\u{25C6}\u{27A2}\u{25A0}\u{25A1}\u{2605}\u{274F}\u{2794}\u{21B5}\u{2756}\u{25AA}\u{2751}\u{2022}",
    "\u{2B9A}\u{2265}\u{2192}\u{21FE}\u{21C9}\u{21D2}\u{21E8}\u{21DB}",
    "\u{00b9}\u{00b2}\u{00b3}\u{2070}\u{2071}\u{2074}\u{2075}\u{2076}\u{2077}\u{2078}\u{2079}",
    "\t\n\r\u{000B}",
);

struct Patterns {
    url_chars: Regex,
    domain_chars: Regex,
    no_protocol_url: Regex,
    e_mail: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        url_chars: regex("[a-zA-ZÄÖÜäöü0-9/%$-_.+!*'(),?#~]+", Flags::default()),
        domain_chars: regex("[a-zA-Z0-9][a-zA-Z0-9-]+", Flags::default()),
        no_protocol_url: regex(
            "([a-zA-Z0-9][a-zA-Z0-9-]+\\.)?([a-zA-Z0-9][a-zA-Z0-9-]+)\\.([a-zA-Z0-9][a-zA-Z0-9-]+)/.*",
            Flags::default(),
        ),
        e_mail: regex(
            "(?<!:)@?\\b[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@((\\[[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}\\])|(([a-zA-Z\\-0-9]+\\.)+[a-zA-Z]{2,}))\\b",
            Flags::default(),
        ),
    })
}

/// `WordTokenizer.getProtocols()`.
pub fn protocols() -> &'static [&'static str] {
    &PROTOCOLS
}

/// `WordTokenizer.isUrl`.
pub fn is_url(token: &str) -> bool {
    if token.starts_with("www.") {
        return true;
    }
    for protocol in PROTOCOLS {
        if token
            .strip_prefix(protocol)
            .is_some_and(|rest| rest.starts_with("://"))
        {
            return true;
        }
    }
    patterns().no_protocol_url.is_full_match(token)
}

/// `WordTokenizer.isEMail`.
pub fn is_email(token: &str) -> bool {
    token.contains('@') && patterns().e_mail.is_full_match(token)
}

/// `WordTokenizer`: the language-independent tokenizer. Every tokenizing
/// character is a token of its own, what lies between two of them is a word.
#[derive(Clone, Copy, Debug, Default)]
pub struct WordTokenizer;

impl WordTokenizer {
    /// `getTokenizingCharacters()`.
    pub fn tokenizing_characters(&self) -> &'static str {
        TOKENIZING_CHARACTERS
    }

    /// `tokenize(text)`.
    pub fn tokenize(&self, text: &str) -> Vec<String> {
        let mut l = Vec::new();
        string_tokenizer(text, TOKENIZING_CHARACTERS, &mut l);
        join_emails_and_urls(l)
    }
}

/// `new StringTokenizer(text, delims, true)`: the runs of non-delimiters and
/// each delimiter, in order.
fn string_tokenizer(text: &str, delims: &str, out: &mut Vec<String>) {
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if delims.contains(c) {
            if start < i {
                out.push(text[start..i].to_string());
            }
            out.push(c.to_string());
            start = i + c.len_utf8();
        }
    }
    if start < text.len() {
        out.push(text[start..].to_string());
    }
}

/// `WordTokenizer.joinEMailsAndUrls`.
pub fn join_emails_and_urls(list: Vec<String>) -> Vec<String> {
    join_urls(join_emails(list))
}

/// `WordTokenizer.joinEMails`: the tokens an e-mail address spans become one.
fn join_emails(list: Vec<String>) -> Vec<String> {
    // The explicit check for "@" is the common way out.
    if !list.iter().any(|t| t.contains('@')) {
        return list;
    }
    let text: Vec<char> = list.iter().flat_map(|t| t.chars()).collect();
    let matches = patterns().e_mail.find_all(&text);
    if matches.is_empty() {
        return list;
    }
    let mut l = Vec::with_capacity(list.len());
    let mut current_position = 0;
    let mut idx = 0;
    for m in &matches {
        let (start, end) = m.spans[0].unwrap_or((0, 0));
        while current_position < end {
            if current_position < start {
                l.push(list[idx].clone());
            } else if current_position == start {
                l.push(text[start..end].iter().collect());
            }
            current_position += list[idx].chars().count();
            idx += 1;
        }
    }
    if current_position < text.len() {
        l.extend(list[idx..].iter().cloned());
    }
    l
}

/// `WordTokenizer.joinUrls` (see RFC 1738).
fn join_urls(l: Vec<String>) -> Vec<String> {
    // A URL starts at "www" or holds a "/" token (see `url_starts_at`);
    // without one the list is unchanged.
    if !l.iter().any(|t| t == "/" || t == "www") {
        return l;
    }
    let mut new_list = Vec::with_capacity(l.len());
    let mut in_url = false;
    let mut url = String::new();
    let mut url_quote: Option<&str> = None;
    for i in 0..l.len() {
        if !in_url && url_starts_at(i, &l) {
            in_url = true;
            if i >= 1 {
                url_quote = Some(&l[i - 1]);
            }
            url.push_str(&l[i]);
        } else if in_url && url_ends_at(i, &l, url_quote) {
            in_url = false;
            url_quote = None;
            new_list.push(std::mem::take(&mut url));
            new_list.push(l[i].clone());
        } else if in_url {
            url.push_str(&l[i]);
        } else {
            new_list.push(l[i].clone());
        }
    }
    if !url.is_empty() {
        new_list.push(url);
    }
    new_list
}

fn url_starts_at(i: usize, l: &[String]) -> bool {
    let token = l[i].as_str();
    if l.len() > i + 3
        && PROTOCOLS.contains(&token)
        && l[i + 1] == ":"
        && l[i + 2] == "/"
        && l[i + 3] == "/"
    {
        return true;
    }
    if l.len() > i + 1 && token == "www" && l[i + 1] == "." {
        // e.g. www.mydomain.org
        return true;
    }
    let p = patterns();
    if l.len() > i + 3
        // e.g. mydomain.org/ (a slash is required, or errors would be read as domains)
        && l[i + 1] == "."
        && l[i + 3] == "/"
        && p.domain_chars.is_full_match(token)
        && p.domain_chars.is_full_match(&l[i + 2])
    {
        return true;
    }
    // e.g. sub.mydomain.org/
    l.len() > i + 5
        && l[i + 1] == "."
        && l[i + 3] == "."
        && l[i + 5] == "/"
        && p.domain_chars.is_full_match(token)
        && p.domain_chars.is_full_match(&l[i + 2])
        && p.domain_chars.is_full_match(&l[i + 4])
}

fn url_ends_at(i: usize, l: &[String], url_quote: Option<&str>) -> bool {
    let token = l[i].as_str();
    if strtools::is_whitespace(token) || token == ")" || token == "]" {
        // this is guesswork
        return true;
    }
    let url_chars = &patterns().url_chars;
    if l.len() > i + 1 {
        let next_token = l[i + 1].as_str();
        (strtools::is_whitespace(next_token)
            || matches!(
                next_token,
                "\"" | "»" | "«" | "‘" | "’" | "“" | "”" | "'" | "."
            ))
            && (matches!(token, "." | "," | ";" | ":" | "!" | "?") || Some(token) == url_quote)
            || !url_chars.is_full_match(token)
    } else {
        !url_chars.is_full_match(token) || token == "." || Some(token) == url_quote
    }
}

// ---------------------------------------------------------------------------
// FrenchWordTokenizer
// ---------------------------------------------------------------------------

/// `FrenchWordTokenizer.doNotSplit`.
const DO_NOT_SPLIT: [&str; 22] = [
    "mers-cov",
    "mcgraw-hill",
    "sars-cov-2",
    "sars-cov",
    "ph-metre",
    "ph-metres",
    "anti-ivg",
    "anti-uv",
    "anti-vih",
    "al-qaïda",
    "c'est-à-dire",
    "add-on",
    "add-ons",
    "rendez-vous",
    "garde-à-vous",
    "chez-eux",
    "chez-moi",
    "chez-nous",
    "chez-soi",
    "chez-toi",
    "chez-vous",
    "m'as-tu-vu",
];

/// `FrenchWordTokenizer.patterns`: the splits of elisions and clitics. Each
/// capturing group becomes a token.
fn french_patterns() -> &'static [Regex; 7] {
    static P: OnceLock<[Regex; 7]> = OnceLock::new();
    P.get_or_init(|| {
        let f = Flags::IGNORE_CASE;
        [
            // words not to be split
            regex("^(c['’]te?|m['’]as-tu-vu|c['’]est-à-dire|add-on|add-ons|rendez-vous|garde-à-vous|chez-eux|chez-moi|chez-nous|chez-soi|chez-toi|chez-vous)$", f),
            regex("^([cç]['’]|j['’]|n['’]|m['’]|t['’]|s['’]|l['’]|d['’]|qu['’]|jusqu['’]|lorsqu['’]|puisqu['’]|quoiqu['’])([^\\-]*)(-ce|-elle|-t-elle|-elles|-t-elles|-en|-il|-t-il|-ils|-t-ils|-je|-la|-le|-les|-leur|-lui|-moi|-nous|-on|-t-on|-toi|-tu|-vous|-vs|-y)$", f),
            // Apostrophe at the beginning of a word: two tokens, l' + homme
            regex("^([cç]['’]|j['’]|n['’]|m['’]|t['’]|s['’]|l['’]|d['’]|qu['’]|jusqu['’]|lorsqu['’]|puisqu['’]|quoiqu['’])([^'’\\-].*)$", f),
            regex("^([^\\-\\d]+)(-ce|-t-elle|-t-elles|-elle|-elles|-en|-il|-t-il|-ils|-t-ils|-je|-la|-le|-les|-leur|-lui|-moi|-nous|-on|-t-on|-toi|-tu|-vous|-vs|-y)(-ce|-elle|-t-elle|-elles|-t-elles|-en|-il|-t-il|-ils|-t-ils|-je|-la|-le|-les|-leur|-lui|-moi|-nous|-on|-t-on|-toi|-tu|-vous|-vs|-y)$", f),
            regex("^([^\\-]*)(-t|-m)(['’]en|['’]y)$", f),
            regex("^(.*)(-t-elle|-t-elles|-t-il|-t-ils|-t-on)$", f),
            regex("^(.*)(-ce|-elle|-t-elle|-elles|-t-elles|-en|-il|-t-il|-ils|-t-ils|-je|-la|-le|-les|-leur|-lui|-moi|-nous|-on|-t-on|-toi|-tu|-vous|-vs|-y)$", f),
        ]
    })
}

/// The clitics the patterns split off (`-ce|-elle|...|-y`), without their
/// hyphen; `-t-elle|-t-elles|-t-il|-t-ils|-t-on` are the other five.
const CLITICS: [&str; 20] = [
    "ce", "elle", "elles", "en", "il", "ils", "je", "la", "le", "les", "leur", "lui", "moi",
    "nous", "on", "toi", "tu", "vous", "vs", "y",
];
const T_CLITICS: [&str; 5] = ["-t-elle", "-t-elles", "-t-il", "-t-ils", "-t-on"];

/// `CASE_INSENSITIVE | UNICODE_CASE` equality with a lower-case literal.
fn eq_fold(chars: &[char], literal: &str) -> bool {
    let mut it = chars.iter();
    literal.chars().all(|l| {
        it.next()
            .is_some_and(|&c| c == l || crate::re::fold(c) == l)
    }) && it.next().is_none()
}

/// The first group of patterns 1 and 2,
/// `[cç]['’]|j['’]|n['’]|m['’]|t['’]|s['’]|l['’]|d['’]|qu['’]|jusqu['’]|lorsqu['’]|puisqu['’]|quoiqu['’]`
/// at the start of `s`: its length, apostrophe included.
fn elision_len(s: &[char]) -> Option<usize> {
    let k = s.iter().take(7).position(|&c| c == '\'' || c == '’')?;
    let word = &s[..k];
    let known = match k {
        1 => ["c", "ç", "j", "n", "m", "t", "s", "l", "d"]
            .iter()
            .any(|w| eq_fold(word, w)),
        2 => eq_fold(word, "qu"),
        5 => eq_fold(word, "jusqu"),
        6 => eq_fold(word, "lorsqu") || eq_fold(word, "puisqu") || eq_fold(word, "quoiqu"),
        _ => false,
    };
    known.then_some(k + 1)
}

/// The groups of the first of `FrenchWordTokenizer.patterns` that matches
/// `s` (`matcher.find()` on anchored patterns), in order; false when none
/// does.
///
/// Most words meet none of the patterns, or the plain elision (pattern 2) or
/// a final clitic (patterns 5 and 6): those cases are decided here by hand,
/// to the same effect; the other patterns run only on the words that have
/// what they need.
fn split_groups<'a>(s: &'a [char], groups: &mut Vec<&'a [char]>) -> bool {
    let mut apostrophe = false;
    let mut hyphens = 0;
    let mut last_hyphen = 0;
    let mut line_terminator = false;
    for (i, &c) in s.iter().enumerate() {
        match c {
            '\'' | '’' => apostrophe = true,
            '-' => {
                hyphens += 1;
                last_hyphen = i;
            }
            '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}' => line_terminator = true,
            _ => {}
        }
    }
    if !apostrophe && hyphens == 0 {
        // every pattern needs an apostrophe or a hyphen
        return false;
    }
    let patterns = french_patterns();
    let by_regex = |index: usize, groups: &mut Vec<&'a [char]>| -> bool {
        match patterns[index].match_at_chars(s, 0) {
            Some(m) => {
                groups.extend(m.spans.iter().skip(1).flatten().map(|&(a, b)| &s[a..b]));
                true
            }
            None => false,
        }
    };
    if line_terminator {
        // "." does not match these: leave such a word (there should be none) to the patterns
        return (0..patterns.len()).any(|i| by_regex(i, groups));
    }
    // 0: words not to be split; they start with c, m, a, r, g and are short
    if s.len() <= 13
        && matches!(crate::re::fold(s[0]), 'c' | 'm' | 'a' | 'r' | 'g')
        && by_regex(0, groups)
    {
        return true;
    }
    let elision = if apostrophe { elision_len(s) } else { None };
    if let Some(k) = elision {
        // 1: elision, then a word without hyphen, then a clitic
        if hyphens > 0 && by_regex(1, groups) {
            return true;
        }
        // 2: ^(elision)([^'’\-].*)$
        if s.len() > k && !matches!(s[k], '\'' | '’' | '-') {
            groups.push(&s[..k]);
            groups.push(&s[k..]);
            return true;
        }
    }
    if hyphens == 0 {
        return false;
    }
    // 3: two clitics; 4: (-t|-m)(['’]en|['’]y)
    if (hyphens >= 2 && by_regex(3, groups)) || (apostrophe && by_regex(4, groups)) {
        return true;
    }
    // 5: ^(.*)(-t-elle|-t-elles|-t-il|-t-ils|-t-on)$
    for clitic in T_CLITICS {
        let n = clitic.len();
        if s.len() >= n && eq_fold(&s[s.len() - n..], clitic) {
            groups.push(&s[..s.len() - n]);
            groups.push(&s[s.len() - n..]);
            return true;
        }
    }
    // 6: ^(.*)(-ce|-elle|...|-y)$: the greedy ".*" leaves the shortest clitic,
    // which starts at the last hyphen
    if CLITICS
        .iter()
        .any(|clitic| eq_fold(&s[last_hyphen + 1..], clitic))
    {
        groups.push(&s[..last_hyphen]);
        groups.push(&s[last_hyphen..]);
        return true;
    }
    false
}

/// `FrenchWordTokenizer.wordCharacters`:
/// `§©@€£\$_\p{L}\d\-̀-ͯ¨⁰-₟°%‰‱&�­¬`.
#[inline]
fn is_word_character(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || matches!(c, '@' | '$' | '_' | '-' | '%' | '&');
    }
    matches!(c,
        '§' | '©' | '€' | '£' | '°' | '‰' | '‱' | '\u{a8}' | '\u{ad}' | '\u{ac}' | '\u{fffd}'
        | '\u{300}'..='\u{36f}' | '\u{2070}'..='\u{209f}')
        || strtools::is_letter(c)
}

/// The auxiliary text of `FrenchWordTokenizer.tokenize`.
///
/// LanguageTool protects the characters that must not split a word (the
/// apostrophe of `l'homme`, the hyphen of `peut-être`, the point of `3.14`,
/// the space of `10 000`) by replacing them with marker words such as
/// `xxFR_HYPHENxx`, then tokenizes and puts the characters back. Here the
/// text is left alone and `glue` records which characters are replaced: a
/// glued character behaves as its marker does (a run of letters) in the
/// later replacements and in the tokenizing pattern.
struct Aux {
    chars: Vec<char>,
    glue: Vec<u8>,
}

const GLUED: u8 = 1;
/// `xxFR_SPACE0xx`: a space after four digits, put back before tokenizing.
const SPACE0: u8 = 2;

impl Aux {
    #[inline]
    fn plain(&self, i: usize, c: char) -> bool {
        self.glue[i] == 0 && self.chars[i] == c
    }

    /// `[\p{L}]` (a marker is made of letters).
    #[inline]
    fn letter(&self, i: usize) -> bool {
        self.glue[i] != 0 || strtools::is_letter(self.chars[i])
    }

    /// `[\d]`.
    #[inline]
    fn digit(&self, i: usize) -> bool {
        self.glue[i] == 0 && self.chars[i].is_ascii_digit()
    }

    /// `\b` after a digit: the next character, if any, is not a word
    /// character.
    #[inline]
    fn boundary_after_digit(&self, i: usize) -> bool {
        i >= self.chars.len() || !(self.glue[i] != 0 || strtools::is_regex_word_char(self.chars[i]))
    }

    /// `([\p{L}])APOS([\p{L}1"‘“«])` -> `$1xxFR_APOS_..xx$2`.
    fn apostrophes(&mut self, apostrophe: char) {
        let n = self.chars.len();
        let mut i = 0;
        while i + 2 < n {
            if self.plain(i + 1, apostrophe) && self.letter(i) && {
                let c = self.chars[i + 2];
                self.letter(i + 2) || matches!(c, '1' | '"' | '‘' | '“' | '«')
            } {
                self.glue[i + 1] = GLUED;
                i += 3;
            } else {
                i += 1;
            }
        }
    }

    /// `([\p{L}])-([\p{L}])-([\p{L}])`, then `([\p{L}])-([\p{L}\d])`.
    fn hyphens(&mut self) {
        let n = self.chars.len();
        let mut i = 0;
        while i + 4 < n {
            if self.plain(i + 1, '-')
                && self.plain(i + 3, '-')
                && self.letter(i)
                && self.letter(i + 2)
                && self.letter(i + 4)
            {
                self.glue[i + 1] = GLUED;
                self.glue[i + 3] = GLUED;
                i += 5;
            } else {
                i += 1;
            }
        }
        let mut i = 0;
        while i + 2 < n {
            if self.plain(i + 1, '-') && self.letter(i) && (self.letter(i + 2) || self.digit(i + 2))
            {
                self.glue[i + 1] = GLUED;
                i += 3;
            } else {
                i += 1;
            }
        }
    }

    /// `([\d])\.([\d])` and `([\d]),([\d])`.
    fn decimal_separator(&mut self, sep: char) {
        let n = self.chars.len();
        let mut i = 0;
        while i + 2 < n {
            if self.plain(i + 1, sep) && self.digit(i) && self.digit(i + 2) {
                self.glue[i + 1] = GLUED;
                i += 3;
            } else {
                i += 1;
            }
        }
    }

    /// The three "space between digits" replacements, in LanguageTool's
    /// order: `d ddd ddd\b`, then four digits and a space (which protects
    /// years from the last one), then `d ddd\b`.
    fn digit_groups(&mut self) {
        let n = self.chars.len();
        let mut i = 0;
        while i + 8 < n {
            if self.plain(i + 1, ' ')
                && self.plain(i + 5, ' ')
                && self.digit(i)
                && (i + 2..i + 5).all(|k| self.digit(k))
                && (i + 6..i + 9).all(|k| self.digit(k))
                && self.boundary_after_digit(i + 9)
            {
                self.glue[i + 1] = GLUED;
                self.glue[i + 5] = GLUED;
                i += 9;
            } else {
                i += 1;
            }
        }
        let mut i = 0;
        while i + 4 < n {
            if self.plain(i + 4, ' ') && (i..i + 4).all(|k| self.digit(k)) {
                self.glue[i + 4] = SPACE0;
                i += 5;
            } else {
                i += 1;
            }
        }
        let mut i = 0;
        while i + 4 < n {
            if self.plain(i + 1, ' ')
                && self.digit(i)
                && (i + 2..i + 5).all(|k| self.digit(k))
                && self.boundary_after_digit(i + 5)
            {
                self.glue[i + 1] = GLUED;
                i += 5;
            } else {
                i += 1;
            }
        }
        for g in &mut self.glue {
            if *g == SPACE0 {
                *g = 0;
            }
        }
    }
}

/// `FrenchWordTokenizer`.
///
/// A word with a hyphen is kept whole when the tagger knows it (`peut-être`)
/// and split otherwise (`dit-on`): `is_tagged` is that dictionary question
/// (LanguageTool asks `FrenchTagger.INSTANCE`).
pub struct FrenchWordTokenizer {
    is_tagged: Box<dyn Fn(&str) -> bool + Send + Sync>,
}

impl FrenchWordTokenizer {
    pub fn new(is_tagged: impl Fn(&str) -> bool + Send + Sync + 'static) -> FrenchWordTokenizer {
        FrenchWordTokenizer {
            is_tagged: Box::new(is_tagged),
        }
    }

    /// `tokenize(text)`.
    pub fn tokenize(&self, text: &str) -> Vec<String> {
        let mut has_typewriter = false;
        let mut has_typographic = false;
        let mut has_hyphen = false;
        let mut has_digit = false;
        // replace hyphen, non-break hyphen -> hyphen-minus
        let chars: Vec<char> = text
            .chars()
            .map(|c| {
                let c = if c == '\u{2010}' || c == '\u{2011}' {
                    '-'
                } else {
                    c
                };
                match c {
                    '\'' => has_typewriter = true,
                    '’' => has_typographic = true,
                    '-' => has_hyphen = true,
                    '0'..='9' => has_digit = true,
                    _ => {}
                }
                c
            })
            .collect();
        let mut aux = Aux {
            glue: vec![0; chars.len()],
            chars,
        };
        if has_typewriter {
            aux.apostrophes('\'');
        }
        if has_typographic {
            aux.apostrophes('’');
        }
        if has_hyphen {
            aux.hyphens();
        }
        if has_digit {
            aux.decimal_separator('.');
            aux.decimal_separator(',');
            aux.digit_groups();
        }

        let Aux { chars, glue } = aux;
        let n = chars.len();
        let mut l: Vec<String> = Vec::with_capacity(n / 3 + 2);
        let mut groups: Vec<&[char]> = Vec::new();
        let mut i = 0;
        // tokenizerPattern: [wordCharacters]+|[^wordCharacters]
        while i < n {
            let word = |k: usize| glue[k] != 0 || is_word_character(chars[k]);
            let start = i;
            i += 1;
            if word(start) {
                while i < n && word(i) {
                    i += 1;
                }
            } else {
                let c = chars[start];
                if ('\u{fe00}'..='\u{fe0f}').contains(&c) {
                    // a variation selector belongs to the token before it
                    if let Some(last) = l.last_mut() {
                        last.push(c);
                        continue;
                    }
                }
                l.push(c.to_string());
                continue;
            }
            let mut s = &chars[start..i];
            while s.len() > 1 && s[0] == '-' {
                l.push("-".to_string());
                s = &s[1..];
            }
            let mut hyphens_at_end = 0;
            while s.len() > 1 && s[s.len() - 1] == '-' {
                s = &s[..s.len() - 1];
                hyphens_at_end += 1;
            }
            groups.clear();
            if split_groups(s, &mut groups) {
                for group in &groups {
                    self.words_to_add(group, &mut l);
                }
            } else {
                self.words_to_add(s, &mut l);
            }
            for _ in 0..hyphens_at_end {
                l.push("-".to_string());
            }
        }
        join_emails_and_urls(l)
    }

    /// `wordsToAdd`: a word with a hyphen is split unless the dictionary has
    /// it.
    fn words_to_add(&self, s: &[char], l: &mut Vec<String>) {
        if s.is_empty() {
            return;
        }
        let word: String = s.iter().collect();
        if !s.contains(&'-') {
            l.push(word);
            return;
        }
        // words containing hyphen (-) are looked up in the dictionary
        let normalized: String = s
            .iter()
            .filter(|&&c| c != '\u{ad}')
            .map(|&c| if c == '’' { '\'' } else { c })
            .collect();
        if (self.is_tagged)(&normalized) || DO_NOT_SPLIT.contains(&word.to_lowercase().as_str()) {
            l.push(word);
        } else {
            // if not found, the word is split
            string_tokenizer(&word, "-", l);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fr(text: &str) -> Vec<String> {
        let known = [
            "peut-être",
            "rendez-vous",
            "celui-ci",
            "c'est-à-dire",
            "grand-mère",
            "-il",
            "-t-il",
            "-le",
            "-moi",
            "-t",
        ];
        let t = FrenchWordTokenizer::new(move |w| known.contains(&w.to_lowercase().as_str()));
        let tokens = t.tokenize(text);
        let joined: String = tokens.concat();
        assert_eq!(
            joined.chars().count(),
            text.chars().count(),
            "tokens cover the text"
        );
        tokens
    }

    fn show(text: &str) -> String {
        fr(text).join("|")
    }

    #[test]
    fn words_punctuation_and_whitespace() {
        assert_eq!(show("Il dort, non ?"), "Il| |dort|,| |non| |?");
        assert_eq!(show("Un\tdeux  trois"), "Un|\t|deux| | |trois");
        assert_eq!(show("50 % de 3€ & co"), "50| |%| |de| |3€| |&| |co");
        assert_eq!(show(""), "");
    }

    #[test]
    fn elisions() {
        assert_eq!(show("l'homme qu'il n'a"), "l'|homme| |qu'|il| |n'|a");
        assert_eq!(show("L’école"), "L’|école");
        assert_eq!(show("aujourd'hui"), "aujourd'hui");
        assert_eq!(show("presqu'île"), "presqu'île");
        assert_eq!(show("c'te fille"), "c'te| |fille");
        // one apostrophe pass does not see overlapping matches
        assert_eq!(show("a'b'c"), "a'b|'|c");
        assert_eq!(show("l'«homme»"), "l'|«|homme|»");
        assert_eq!(show("d'1 an"), "d'|1| |an");
    }

    #[test]
    fn clitics_and_hyphens() {
        assert_eq!(show("Dit-il"), "Dit|-il");
        assert_eq!(show("A-t-il"), "A|-t-il");
        assert_eq!(show("donne-le-moi"), "donne|-le|-moi");
        assert_eq!(show("va-t'en"), "va|-t|'en");
        assert_eq!(show("qu'a-t-il"), "qu'|a|-t-il");
        assert_eq!(show("peut-être"), "peut-être");
        assert_eq!(show("Peut-être"), "Peut-être");
        assert_eq!(show("rendez-vous"), "rendez-vous");
        assert_eq!(show("celui-ci"), "celui-ci");
        assert_eq!(show("un chou-xyz"), "un| |chou|-|xyz");
        assert_eq!(show("a-b-c-d-e"), "a|-|b|-|c|-|d|-|e");
        assert_eq!(show("-oui-"), "-|oui|-");
        assert_eq!(show("--"), "-|-");
        assert_eq!(show("mers-cov"), "mers-cov");
        // U+2010 and U+2011 are read as the hyphen-minus
        assert_eq!(show("peut\u{2010}être"), "peut-être");
    }

    /// `split_groups` as LanguageTool writes it: the first pattern that matches.
    fn split_groups_by_patterns(s: &[char]) -> Option<Vec<String>> {
        french_patterns()
            .iter()
            .find_map(|p| p.match_at_chars(s, 0))
            .map(|m| {
                m.spans
                    .iter()
                    .skip(1)
                    .flatten()
                    .map(|&(a, b)| s[a..b].iter().collect())
                    .collect()
            })
    }

    #[test]
    fn hand_written_splits_agree_with_the_patterns() {
        let starts = [
            "", "c'", "C’", "ç'", "j'", "n'", "m'", "t'", "s'", "l'", "L’", "d'", "qu'", "QU'",
            "Qu’", "jusqu'", "lorsqu'", "puisqu'", "quoiqu'", "presqu'", "quelqu'", "x'", "ſ'",
            "jusq'", "'", "’", "qu", "c", "aujourd'", "l''", "l'-",
        ];
        let middles = [
            "",
            "a",
            "est",
            "homme",
            "a-t",
            "est-à",
            "va",
            "donne",
            "peut-être",
            "1",
            "as-tu",
            "É",
            "x'y",
            "-",
            "a-",
            "te",
            "t",
            "ete",
            "en",
            "y",
            "il",
            "on",
            "add",
            "rendez",
            "chez",
            "garde-à",
            "d'un",
            "12",
            "a1",
        ];
        let ends = [
            "",
            "-ce",
            "-CE",
            "-elle",
            "-t-elle",
            "-elles",
            "-t-elles",
            "-en",
            "-il",
            "-t-il",
            "-T-IL",
            "-ils",
            "-t-ils",
            "-je",
            "-la",
            "-le",
            "-les",
            "-leur",
            "-lui",
            "-moi",
            "-nous",
            "-on",
            "-t-on",
            "-toi",
            "-tu",
            "-vous",
            "-vs",
            "-y",
            "-le-moi",
            "-t-il-le",
            "-la-lui",
            "-vous-en",
            "-t'en",
            "-m'y",
            "-t’y",
            "-m'en",
            "-t",
            "-m",
            "-ci",
            "-là",
            "-vu",
            "-dire",
            "-t-",
            "-on-dit",
            "-t-elle-même",
            "'en",
            "-t-ille",
            "-Il",
            "-İl",
            "-ſ",
            "-vſ",
        ];
        let mut checked = 0;
        for a in starts {
            for b in middles {
                for c in ends {
                    let word: Vec<char> = format!("{a}{b}{c}").chars().collect();
                    if word.is_empty() {
                        continue;
                    }
                    let mut groups = Vec::new();
                    let fast = split_groups(&word, &mut groups).then(|| {
                        groups
                            .iter()
                            .map(|g| g.iter().collect::<String>())
                            .collect::<Vec<_>>()
                    });
                    assert_eq!(fast, split_groups_by_patterns(&word), "{a}{b}{c}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 40_000);
    }

    #[test]
    fn numbers() {
        assert_eq!(show("3.14 et 2,5"), "3.14| |et| |2,5");
        assert_eq!(show("1.2.3"), "1.2|.|3");
        assert_eq!(show("10 000 euros"), "10 000| |euros");
        assert_eq!(show("1 234 567 fois"), "1 234 567| |fois");
        assert_eq!(show("en 2019 123 fois"), "en| |2019| |123| |fois");
        assert_eq!(show("1 234é"), "1| |234é");
        assert_eq!(show("1 2345"), "1| |2345");
    }

    #[test]
    fn variation_selectors_join_the_previous_token() {
        assert_eq!(show("\u{2764}\u{fe0f} ok"), "\u{2764}\u{fe0f}| |ok");
        assert_eq!(show("x\u{1f602}y"), "x|\u{1f602}|y");
    }

    #[test]
    fn urls_and_emails() {
        assert_eq!(
            show("Voir http://foo.org/a?b=1. Fin"),
            "Voir| |http://foo.org/a?b=1|.| |Fin"
        );
        assert_eq!(show("sur www.exemple.fr ici"), "sur| |www.exemple.fr| |ici");
        assert_eq!(show("(https://a.b/c)"), "(|https://a.b/c|)");
        assert_eq!(show("exemple.org/page"), "exemple.org/page");
        assert_eq!(show("exemple.org page"), "exemple|.|org| |page");
        assert_eq!(
            show("écrire à jean.dupont@exemple.fr vite"),
            "écrire| |à| |jean.dupont@exemple.fr| |vite"
        );
        assert!(is_url("http://foo.org"));
        assert!(is_url("www.foo.org"));
        assert!(is_url("foo.org/x"));
        assert!(!is_url("foo.org"));
        assert!(is_email("a.b@c.fr"));
        assert!(!is_email("a.b@c"));
    }

    #[test]
    fn base_tokenizer() {
        let t = WordTokenizer;
        assert_eq!(
            t.tokenize("Un test, vraiment.").join("|"),
            "Un| |test|,| |vraiment|."
        );
        assert_eq!(t.tokenize("l'ami").join("|"), "l|'|ami");
    }
}
