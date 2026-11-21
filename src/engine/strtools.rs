//! String helpers with the exact behaviour of LanguageTool's `StringTools`
//! (which rules and filters rely on), plus UTF-16 length helpers: LanguageTool
//! counts positions in Java `char`s.

/// `Character.isLetter` (general category L). The Alphabetic property is
/// wider: it also has the letter-like numbers and symbols (roman numerals,
/// circled letters), which are not letters.
#[inline]
pub fn is_letter(c: char) -> bool {
    crate::re::is_letter(c)
}

/// `Character.isLetter(char)` on one UTF-16 unit: the loops of `StringTools`
/// run over Java `char`s, so a character outside the Basic Multilingual
/// Plane (two surrogates) is never a letter there.
#[inline]
fn is_letter_unit(c: char) -> bool {
    (c as u32) < 0x10000 && crate::re::is_letter(c)
}

/// `Character.isSpaceChar`: the Unicode space, line and paragraph separators.
#[inline]
pub fn java_is_space_char(c: char) -> bool {
    matches!(
        c,
        ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
    )
}

/// What `\b` takes for a word character in a Java pattern (see
/// [`crate::re::is_word_boundary_char`]).
#[inline]
pub fn is_regex_word_char(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    crate::re::is_word_boundary_char(c)
}

/// `StringTools.isWhitespace`: an all-whitespace token (Java `trim()` removes
/// every char `<= ' '`), a single Unicode whitespace char, NBSP, narrow NBSP,
/// zero-width space or BOM. OpenOffice field codes are not whitespace.
pub fn is_whitespace(s: &str) -> bool {
    if s == "\u{2}" || s == "\u{1}" {
        return false;
    }
    if s == "\u{feff}" {
        return true;
    }
    let trimmed = s.trim_matches(|c: char| c <= ' ');
    if trimmed.is_empty() {
        return true;
    }
    let mut it = trimmed.chars();
    let (Some(c), None) = (it.next(), it.next()) else {
        return false;
    };
    if s == "\u{200b}" || s == "\u{a0}" || s == "\u{202f}" {
        return true;
    }
    java_is_whitespace(c)
}

/// `Character.isWhitespace`: Unicode space separators except the non-breaking
/// ones, plus the ASCII control separators.
pub fn java_is_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | '\u{1c}'..='\u{1f}'
    ) || (matches!(c, ' ' | '\u{1680}' | '\u{2000}'..='\u{2006}' | '\u{2008}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{205f}' | '\u{3000}'))
}

/// `StringTools.isAllUppercase`: no lowercase letter (a string without
/// letters counts as all uppercase).
pub fn is_all_uppercase(s: &str) -> bool {
    !s.chars().any(|c| is_letter_unit(c) && c.is_lowercase())
}

/// `StringTools.isNotAllLowercase`.
pub fn is_not_all_lowercase(s: &str) -> bool {
    s.chars().any(|c| is_letter_unit(c) && !c.is_lowercase())
}

/// `StringTools.isCapitalizedWord`: first char uppercase, no other
/// non-lowercase letter.
pub fn is_capitalized_word(s: &str) -> bool {
    let mut it = s.chars();
    match it.next() {
        Some(c) if c.is_uppercase() && (c as u32) < 0x10000 => {
            it.all(|c| !(is_letter_unit(c) && !c.is_lowercase()))
        }
        _ => false,
    }
}

/// `StringTools.isMixedCase`.
pub fn is_mixed_case(s: &str) -> bool {
    !is_all_uppercase(s) && !is_capitalized_word(s) && is_not_all_lowercase(s)
}

pub fn starts_with_uppercase(s: &str) -> bool {
    s.chars().next().is_some_and(char::is_uppercase)
}

pub fn starts_with_lowercase(s: &str) -> bool {
    s.chars().next().is_some_and(char::is_lowercase)
}

fn upper_char(c: char) -> char {
    let mut u = c.to_uppercase();
    match (u.next(), u.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

fn lower_char(c: char) -> char {
    let mut u = c.to_lowercase();
    match (u.next(), u.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

/// `StringTools.changeFirstCharCase`: change the case of the first letter or
/// digit (leading punctuation is kept), with `Character.toUpperCase` /
/// `toLowerCase` semantics (one char to one char).
fn change_first_char_case(s: &str, upper: bool) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    if chars.len() == 1 {
        return if upper {
            s.to_uppercase()
        } else {
            s.to_lowercase()
        };
    }
    let mut pos = 0;
    let len = chars.len() - 1;
    while !chars[pos].is_alphanumeric() && len > pos {
        pos += 1;
    }
    let mut out = String::with_capacity(s.len());
    out.extend(&chars[..pos]);
    out.push(if upper {
        upper_char(chars[pos])
    } else {
        lower_char(chars[pos])
    });
    out.extend(&chars[pos + 1..]);
    out
}

/// `StringTools.uppercaseFirstChar`.
pub fn uppercase_first_char(s: &str) -> String {
    change_first_char_case(s, true)
}

/// `StringTools.lowercaseFirstChar`.
pub fn lowercase_first_char(s: &str) -> String {
    change_first_char_case(s, false)
}

/// `StringTools.lowercaseFirstCharIfCapitalized`.
pub fn lowercase_first_char_if_capitalized(s: &str) -> String {
    if is_capitalized_word(s) {
        change_first_char_case(s, false)
    } else {
        s.to_string()
    }
}

/// `Character.toTitleCase(char)`: the one-char upper case, except for the
/// digraph letters, which have a title case of their own, and the Georgian
/// letters, which are their own title case. Java maps `char`s, so a
/// supplementary character is left alone.
fn title_char(c: char) -> char {
    match c {
        'Ǆ' | 'ǅ' | 'ǆ' => 'ǅ',
        'Ǉ' | 'ǈ' | 'ǉ' => 'ǈ',
        'Ǌ' | 'ǋ' | 'ǌ' => 'ǋ',
        'Ǳ' | 'ǲ' | 'ǳ' => 'ǲ',
        '\u{10d0}'..='\u{10fa}' | '\u{10fd}'..='\u{10ff}' => c,
        c if c as u32 >= 0x10000 => c,
        c => upper_char(c),
    }
}

/// `Character.toLowerCase(char)`.
fn java_lower_char(c: char) -> char {
    match c {
        'İ' => 'i',
        c if c as u32 >= 0x10000 => c,
        c => lower_char(c),
    }
}

/// `StringTools.convertToTitleCaseIteratingChars`: title-case the first char
/// and every char that follows a space or a hyphen, lowercase the others.
pub fn convert_to_title_case_iterating_chars(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut convert_next = true;
    for c in s.chars() {
        if java_is_space_char(c) || c == '-' {
            convert_next = true;
            out.push(c);
        } else if convert_next {
            out.push(title_char(c));
            convert_next = false;
        } else {
            out.push(java_lower_char(c));
        }
    }
    out
}

/// `StringTools.preserveCase`: give `input` the capitalisation of `model`.
pub fn preserve_case(input: &str, model: &str) -> String {
    if model.is_empty() {
        return input.to_string();
    }
    if is_capitalized_word(model) {
        return uppercase_first_char(&input.to_lowercase());
    }
    if is_all_uppercase(model) {
        return input.to_uppercase();
    }
    input.to_string()
}

/// `StringTools.isEmoji`: a word with a character outside the Basic
/// Multilingual Plane (two Java `char`s) that is not made only of letters,
/// digits, punctuation and spaces (`^[\p{L}\d\p{P}\p{Zs}]+$`).
pub fn is_emoji(s: &str) -> bool {
    if !s.chars().any(|c| c as u32 >= 0x10000) {
        return false;
    }
    static WORD_FOR_SPELLER: std::sync::OnceLock<crate::re::Regex> = std::sync::OnceLock::new();
    let re = WORD_FOR_SPELLER.get_or_init(|| {
        crate::re::Regex::new("^[\\p{L}\\d\\p{P}\\p{Zs}]+$", crate::re::Flags::default())
            .expect("valid pattern")
    });
    !re.is_full_match(s)
}

/// Length in UTF-16 code units (`String.length()` in Java).
#[inline]
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Java `String.equalsIgnoreCase`: char-wise, comparing upper cases then
/// lower cases.
pub fn equals_ignore_case(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let mut x = a.chars();
    let mut y = b.chars();
    loop {
        match (x.next(), y.next()) {
            (None, None) => return true,
            (Some(p), Some(q)) => {
                if p != q && crate::re::fold(p) != crate::re::fold(q) {
                    return false;
                }
            }
            _ => return false,
        }
    }
}

/// `String.toLowerCase()` restricted to what Java does for French text. Rust's
/// `to_lowercase` agrees except for the final sigma, which French never uses.
#[inline]
pub fn to_lower(s: &str) -> String {
    s.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace() {
        assert!(is_whitespace(" "));
        assert!(is_whitespace("\u{a0}"));
        assert!(is_whitespace("\u{202f}"));
        assert!(is_whitespace("\n"));
        assert!(is_whitespace("  \t"));
        assert!(!is_whitespace("a"));
        assert!(!is_whitespace("\u{1}"));
        assert!(
            !is_whitespace("\u{2009}\u{2009}"),
            "two thin spaces are not trimmed by Java"
        );
        assert!(is_whitespace("\u{2009}"));
    }

    #[test]
    fn case_predicates() {
        assert!(is_capitalized_word("Paris"));
        assert!(!is_capitalized_word("PARIS"));
        assert!(!is_capitalized_word("paris"));
        assert!(is_capitalized_word("Jean-pierre"));
        assert!(is_all_uppercase("PARIS"));
        assert!(is_all_uppercase("12"));
        assert!(is_mixed_case("iPhone"));
        assert!(!is_mixed_case("Paris"));
        assert_eq!(uppercase_first_char("école"), "École");
        assert_eq!(uppercase_first_char("«école"), "«École");
        assert_eq!(lowercase_first_char("École"), "école");
        assert_eq!(
            convert_to_title_case_iterating_chars("jean-pierre"),
            "Jean-Pierre"
        );
        assert_eq!(
            convert_to_title_case_iterating_chars("l'homme DE fer"),
            "L'homme De Fer"
        );
        assert_eq!(
            convert_to_title_case_iterating_chars("ÉTATS-UNIS"),
            "États-Unis"
        );
        assert_eq!(preserve_case("chat", "Chien"), "Chat");
        assert_eq!(preserve_case("chat", "CHIEN"), "CHAT");
        assert!(is_emoji("😂"));
        assert!(is_emoji("a😂"));
        assert!(!is_emoji("❤\u{fe0f}"), "no char outside the BMP");
        assert!(!is_emoji("𝒶"), "a letter outside the BMP is not an emoji");
        assert!(
            is_letter('é')
                && is_letter('ª')
                && !is_letter('Ⅷ')
                && !is_letter('ⓐ')
                && !is_letter('1')
        );
        assert!(equals_ignore_case("École", "éCOLE"));
        assert!(!equals_ignore_case("a", "à"));
    }
}
