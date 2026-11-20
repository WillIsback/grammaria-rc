//! `java.lang.Character` and `java.text.Normalizer` look-alikes for the speller.
//!
//! Morfologik's speller works on Java `char`s (UTF-16 code units) and calls
//! `Character.isLetter`, `isLowerCase`, `isUpperCase`, `isDigit`,
//! `toLowerCase`, `toUpperCase` and `Normalizer.normalize(.., NFD)` on them.
//! The functions below reproduce those calls with the standard library's
//! Unicode tables plus small data tables generated from the JDK
//! (`unicode_tables.rs`), so that the port takes the same decisions as the
//! Java code. A surrogate code unit (half of a supplementary character) is
//! never a letter and has no case, as in Java.
//!
//! Checked against JDK 21 over the whole Basic Multilingual Plane (test
//! `character_classes_match_java`): the only differences are about twenty
//! characters added, or whose case mapping changed, after Unicode 15.0 (the
//! version of the JDK's tables; Rust's are more recent).

use super::unicode_tables::{LETTER_NUMBERS, NFD_FIRST, OTHER_ALPHABETIC};

#[inline]
fn to_char(c: u16) -> Option<char> {
    char::from_u32(u32::from(c))
}

/// True if `c` is in one of the sorted inclusive `ranges`.
#[inline]
fn in_ranges(ranges: &[(u16, u16)], c: u16) -> bool {
    ranges
        .binary_search_by(|&(lo, hi)| {
            if c < lo {
                std::cmp::Ordering::Greater
            } else if c > hi {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Combining Latin letters (marks) that gained the `Alphabetic` property
/// after Unicode 15.0: not letters, in any version.
#[inline]
fn is_late_alphabetic_mark(c: u16) -> bool {
    (0x0363..=0x036F).contains(&c) || (0x1DD3..=0x1DE6).contains(&c)
}

/// True for the alphabetic characters that are not letters or letter numbers.
#[inline]
fn is_other_alphabetic(c: u16) -> bool {
    c >= 0x0345 && (in_ranges(&OTHER_ALPHABETIC, c) || is_late_alphabetic_mark(c))
}

/// First code point of each run of ten decimal digits (category `Nd`) in the
/// BMP.
const DIGIT_ZEROS: [u16; 37] = [
    0x0030, 0x0660, 0x06F0, 0x07C0, 0x0966, 0x09E6, 0x0A66, 0x0AE6, 0x0B66, 0x0BE6, 0x0C66, 0x0CE6,
    0x0D66, 0x0DE6, 0x0E50, 0x0ED0, 0x0F20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80,
    0x1A90, 0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0,
    0xFF10,
];

/// `Character.isLetter(char)`: general category `L*`.
///
/// Rust's `char::is_alphabetic` is the wider `Alphabetic` property; letter
/// numbers and the other alphabetic characters (combining vowel signs,
/// circled letters) are removed from it.
#[inline]
pub fn is_letter(c: u16) -> bool {
    if c < 0x80 {
        return (c as u8).is_ascii_alphabetic();
    }
    match to_char(c) {
        Some(ch) => ch.is_alphabetic() && !is_other_alphabetic(c) && !in_ranges(&LETTER_NUMBERS, c),
        None => false,
    }
}

/// The speller's `isAlphabetic`: categories `Lu Ll Lt Lm Lo Nl`.
#[inline]
pub fn is_alphabetic(c: u16) -> bool {
    if c < 0x80 {
        return (c as u8).is_ascii_alphabetic();
    }
    match to_char(c) {
        Some(ch) => ch.is_alphabetic() && !is_other_alphabetic(c),
        None => false,
    }
}

/// `Character.isLowerCase(char)`.
#[inline]
pub fn is_lower_case(c: u16) -> bool {
    if c < 0x80 {
        return (c as u8).is_ascii_lowercase();
    }
    to_char(c).is_some_and(char::is_lowercase)
}

/// `Character.isUpperCase(char)`.
#[inline]
pub fn is_upper_case(c: u16) -> bool {
    if c < 0x80 {
        return (c as u8).is_ascii_uppercase();
    }
    to_char(c).is_some_and(char::is_uppercase)
}

/// `Character.isDigit(char)`: general category `Nd`.
#[inline]
pub fn is_digit(c: u16) -> bool {
    if c < 0x80 {
        return (c as u8).is_ascii_digit();
    }
    DIGIT_ZEROS.iter().any(|&z| c >= z && c < z + 10)
}

/// `Character.toLowerCase(char)`: the simple (one to one) lowercase mapping.
#[inline]
pub fn to_lower_case(c: u16) -> u16 {
    if c < 0x80 {
        return u16::from((c as u8).to_ascii_lowercase());
    }
    let Some(ch) = to_char(c) else { return c };
    // U+0130 has a simple mapping to 'i' but a two-character full mapping.
    if c == 0x0130 {
        return 0x0069;
    }
    single_bmp(ch.to_lowercase(), c)
}

/// `Character.toUpperCase(char)`: the simple (one to one) uppercase mapping.
#[inline]
pub fn to_upper_case(c: u16) -> u16 {
    if c < 0x80 {
        return u16::from((c as u8).to_ascii_uppercase());
    }
    // Greek letters with iota subscript: the simple mapping is the titlecase
    // letter, the full mapping (what Rust gives) is two letters.
    match c {
        0x1F80..=0x1F87 | 0x1F90..=0x1F97 | 0x1FA0..=0x1FA7 => return c + 8,
        0x1FB3 | 0x1FC3 | 0x1FF3 => return c + 9,
        _ => {}
    }
    let Some(ch) = to_char(c) else { return c };
    single_bmp(ch.to_uppercase(), c)
}

/// The mapping if it is a single BMP character, the original otherwise (a
/// multi-character full mapping means there is no simple one).
fn single_bmp(mut mapped: impl Iterator<Item = char>, original: u16) -> u16 {
    match (mapped.next(), mapped.next()) {
        (Some(m), None) if (m as u32) < 0x10000 => m as u16,
        _ => original,
    }
}

/// The first UTF-16 unit of `Normalizer.normalize(String.valueOf(c), NFD)`:
/// the base character of an accented letter, `c` itself if it does not
/// decompose.
#[inline]
pub fn nfd_first(c: u16) -> u16 {
    if c < 0xC0 {
        return c;
    }
    // Hangul syllables decompose arithmetically; the first unit is the
    // leading consonant.
    if (0xAC00..=0xD7A3).contains(&c) {
        return 0x1100 + (c - 0xAC00) / 588;
    }
    match NFD_FIRST.binary_search_by_key(&c, |&(k, _)| k) {
        Ok(i) => NFD_FIRST[i].1,
        Err(_) => c,
    }
}

/// `String.toLowerCase()` in a locale without special casing rules.
pub fn lower_string(s: &str) -> String {
    s.to_lowercase()
}

/// `String.toUpperCase()` in a locale without special casing rules.
pub fn upper_string(s: &str) -> String {
    s.to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decomposition_base() {
        assert_eq!(nfd_first('é' as u16), 'e' as u16);
        assert_eq!(nfd_first('É' as u16), 'E' as u16);
        assert_eq!(nfd_first('ç' as u16), 'c' as u16);
        assert_eq!(nfd_first('ï' as u16), 'i' as u16);
        assert_eq!(nfd_first('œ' as u16), 'œ' as u16);
        assert_eq!(nfd_first('ǘ' as u16), 'u' as u16); // two levels: ü + acute
        assert_eq!(nfd_first('한' as u16), 0x1112);
        assert_eq!(nfd_first('a' as u16), 'a' as u16);
        assert!(NFD_FIRST.windows(2).all(|w| w[0].0 < w[1].0));
        for table in [&OTHER_ALPHABETIC[..], &LETTER_NUMBERS[..]] {
            assert!(table.iter().all(|&(lo, hi)| lo <= hi));
            assert!(table.windows(2).all(|w| w[0].1 < w[1].0));
        }
    }

    #[test]
    fn java_character_classes() {
        assert!(is_letter('é' as u16) && is_lower_case('é' as u16));
        assert!(is_upper_case('É' as u16) && !is_lower_case('É' as u16));
        assert!(!is_letter('1' as u16) && is_digit('1' as u16));
        assert!(is_digit('٣' as u16) && !is_digit('²' as u16));
        // Roman numeral: a letter number, alphabetic for the speller only.
        assert!(!is_letter('Ⅷ' as u16) && is_alphabetic('Ⅷ' as u16));
        // Roman numeral reversed one hundred is a real letter (Lu).
        assert!(is_letter(0x2183) && is_alphabetic(0x2183));
        // Combining marks and circled letters are neither.
        assert!(!is_letter(0x0345) && !is_alphabetic(0x0345));
        assert!(!is_letter(0x0903) && !is_letter('Ⓐ' as u16) && !is_alphabetic('Ⓐ' as u16));
        assert!(is_letter('日' as u16) && is_letter('ж' as u16) && is_letter('ª' as u16));
        assert!(!is_letter(0xD83D) && !is_upper_case(0xD83D));
        assert_eq!(to_upper_case('é' as u16), 'É' as u16);
        assert_eq!(to_upper_case('ß' as u16), 'ß' as u16);
        assert_eq!(to_upper_case(0x1F80), 0x1F88);
        assert_eq!(to_upper_case(0x1FF3), 0x1FFC);
        assert_eq!(to_lower_case('Œ' as u16), 'œ' as u16);
        assert_eq!(to_lower_case(0x0130), 'i' as u16);
        assert_eq!(to_lower_case(0xD83D), 0xD83D);
    }
}
