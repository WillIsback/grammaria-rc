//! String helpers the spelling classes need: the parts of LanguageTool's
//! `org.languagetool.tools.StringTools` that `engine::strtools` does not have
//! yet (`removeDiacritics`, `makeWrong`, `isAnagram`, `splitCamelCase`,
//! `splitDigitsAtEnd`, `isNotWordString`), a few `java.lang.String` and
//! `org.apache.commons.lang3.StringUtils` behaviours (`split`,
//! `substringBefore`, `hashCode`), and the iteration order of
//! `java.util.HashMap`.
//!
//! Java indexes strings by UTF-16 unit; the helpers here work on Unicode
//! scalar values. The two only differ for characters outside the Basic
//! Multilingual Plane, which the callers never cut in two.

use crate::morfologik::unicode;

/// `StringTools.removeDiacritics`: NFD-normalize, then drop the characters of
/// the "Combining Diacritical Marks" block (U+0300..U+036F).
///
/// The decomposition keeps the base character of each precomposed letter
/// (what remains of it once the marks of that block are dropped), which is
/// exact for the Latin, Greek and Cyrillic letters; Hangul syllables and the
/// few characters whose decomposition holds a mark from another block are
/// left as they are.
pub fn remove_diacritics(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let cp = c as u32;
        if (0x0300..=0x036F).contains(&cp) {
            continue;
        }
        if !(0xC0..=0xFFFF).contains(&cp) || (0xAC00..=0xD7A3).contains(&cp) {
            out.push(c);
            continue;
        }
        match char::from_u32(u32::from(unicode::nfd_first(cp as u16))) {
            Some(base) => out.push(base),
            None => out.push(c),
        }
    }
    out
}

/// `StringTools.makeWrong`: invent a wrong word, to get the speller's
/// suggestions around a correct one.
pub fn make_wrong(s: &str) -> String {
    const SWAPS: [(&str, &str); 16] = [
        ("a", "ä"),
        ("e", "ë"),
        ("i", "ï"),
        ("o", "ö"),
        ("u", "ù"),
        ("á", "ä"),
        ("é", "ë"),
        ("í", "ï"),
        ("ó", "ö"),
        ("ú", "ù"),
        ("à", "ä"),
        ("è", "ë"),
        ("ì", "i"),
        ("ò", "ö"),
        ("ï", "ì"),
        ("ü", "ù"),
    ];
    for (from, to) in SWAPS {
        if s.contains(from) {
            return s.replace(from, to);
        }
    }
    format!("{s}-")
}

/// `StringTools.isAnagram`: same length and same characters.
pub fn is_anagram(a: &str, b: &str) -> bool {
    let mut x: Vec<u16> = a.encode_utf16().collect();
    let mut y: Vec<u16> = b.encode_utf16().collect();
    if x.len() != y.len() {
        return false;
    }
    x.sort_unstable();
    y.sort_unstable();
    x == y
}

/// `StringTools.splitCamelCase`: cut before each run of uppercase letters.
pub fn split_camel_case(input: &str) -> Vec<String> {
    if super::super::strtools::is_all_uppercase(input) {
        return vec![input.to_string()];
    }
    let mut word = String::new();
    let mut result = String::new();
    let mut previous_is_uppercase = false;
    for c in input.chars() {
        if c.is_uppercase() {
            if !previous_is_uppercase {
                result.push_str(&word);
                result.push(' ');
                word.clear();
            }
            previous_is_uppercase = true;
        } else {
            previous_is_uppercase = false;
        }
        word.push(c);
    }
    result.push_str(&word);
    java_split(java_trim(&result), ' ')
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// `StringTools.splitDigitsAtEnd`: `["mot", "12"]` for `mot12`.
pub fn split_digits_at_end(input: &str) -> Vec<String> {
    let digits: usize = input
        .chars()
        .rev()
        .take_while(|c| java_is_digit(*c))
        .map(char::len_utf8)
        .sum();
    let (word, number) = input.split_at(input.len() - digits);
    if !word.is_empty() && !number.is_empty() {
        vec![word.to_string(), number.to_string()]
    } else {
        vec![input.to_string()]
    }
}

/// `StringTools.isNotWordString`: one character or more, none of them a
/// letter.
pub fn is_not_word_string(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(java_is_letter)
}

/// `Character.isLetter`.
pub fn java_is_letter(c: char) -> bool {
    let cp = c as u32;
    if cp <= 0xFFFF {
        unicode::is_letter(cp as u16)
    } else {
        c.is_alphabetic()
    }
}

/// `Character.isDigit`.
pub fn java_is_digit(c: char) -> bool {
    let cp = c as u32;
    if cp <= 0xFFFF {
        unicode::is_digit(cp as u16)
    } else {
        c.is_numeric()
    }
}

/// `StringTools.isAllUppercase`, on UTF-16 units as in Java: a character
/// outside the Basic Multilingual Plane is two surrogates, which are not
/// letters (`engine::strtools` looks at whole characters).
pub fn java_is_all_uppercase(s: &str) -> bool {
    !s.encode_utf16()
        .any(|c| unicode::is_letter(c) && unicode::is_lower_case(c))
}

/// `StringTools.isMixedCase`, on UTF-16 units.
pub fn java_is_mixed_case(s: &str) -> bool {
    let units: Vec<u16> = s.encode_utf16().collect();
    let all_uppercase = !units
        .iter()
        .any(|&c| unicode::is_letter(c) && unicode::is_lower_case(c));
    let capitalized = units.first().is_some_and(|&c| unicode::is_upper_case(c))
        && !units[1..]
            .iter()
            .any(|&c| unicode::is_letter(c) && !unicode::is_lower_case(c));
    let not_all_lowercase = units
        .iter()
        .any(|&c| unicode::is_letter(c) && !unicode::is_lower_case(c));
    !all_uppercase && !capitalized && not_all_lowercase
}

/// `StringTools.startsWithUppercase`, on UTF-16 units.
pub fn java_starts_with_uppercase(s: &str) -> bool {
    s.encode_utf16().next().is_some_and(unicode::is_upper_case)
}

/// `String.trim()`: strips the characters up to U+0020 at both ends.
pub fn java_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c <= ' ')
}

/// `String.split(String.valueOf(sep))`: trailing empty strings are dropped
/// (an empty input still gives one empty string).
pub fn java_split(s: &str, sep: char) -> Vec<&str> {
    if s.is_empty() {
        return vec![""];
    }
    let mut parts: Vec<&str> = s.split(sep).collect();
    while parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    parts
}

/// `StringUtils.split(s, sep)`: the non-empty pieces between separators.
pub fn split_non_empty(s: &str, sep: char) -> Vec<&str> {
    s.split(sep).filter(|p| !p.is_empty()).collect()
}

/// `StringUtils.substringBefore(s, "#")`.
pub fn substring_before(s: &str, sep: char) -> &str {
    match s.find(sep) {
        Some(i) => &s[..i],
        None => s,
    }
}

/// `String.hashCode()`.
pub fn java_hash(s: &str) -> i32 {
    let mut h: i32 = 0;
    for u in s.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(i32::from(u));
    }
    h
}

/// The order in which a `java.util.HashMap<String, _>` filled with `keys` (in
/// that order, without removals) iterates over them: by bucket, and by
/// insertion inside a bucket. Returns indices into `keys`.
///
/// Buckets that Java turned into trees (eight or more colliding keys) may
/// come out in a slightly different order; with `String.hashCode` and the
/// table sizes involved that practically never happens.
pub fn java_hash_map_order<S: AsRef<str>>(keys: &[S]) -> Vec<usize> {
    // The table doubles whenever the number of entries exceeds 3/4 of it.
    let mut capacity: usize = 16;
    while keys.len() > capacity / 4 * 3 {
        capacity *= 2;
    }
    let bucket = |k: &str| {
        let h = java_hash(k) as u32;
        ((h ^ (h >> 16)) as usize) & (capacity - 1)
    };
    let mut order: Vec<(usize, usize)> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| (bucket(k.as_ref()), i))
        .collect();
    order.sort();
    order.into_iter().map(|(_, i)| i).collect()
}

/// `\p{script=latin}`, for the characters a spell checker meets.
pub fn is_latin_script(c: char) -> bool {
    matches!(c as u32,
        0x41..=0x5A | 0x61..=0x7A | 0xAA | 0xBA | 0xC0..=0xD6 | 0xD8..=0xF6 | 0xF8..=0x2B8
        | 0x2E0..=0x2E4 | 0x1D00..=0x1D25 | 0x1D2C..=0x1D5C | 0x1D62..=0x1D65 | 0x1D6B..=0x1D77
        | 0x1D79..=0x1DBE | 0x1E00..=0x1EFF | 0x2071 | 0x207F | 0x2090..=0x209C | 0x212A..=0x212B
        | 0x2132 | 0x214E | 0x2160..=0x2188 | 0x2C60..=0x2C7F | 0xA722..=0xA787 | 0xA78B..=0xA7CA
        | 0xA7D0..=0xA7D9 | 0xA7F2..=0xA7FF | 0xAB30..=0xAB5A | 0xAB5C..=0xAB64 | 0xAB66..=0xAB69
        | 0xFB00..=0xFB06 | 0xFF21..=0xFF3A | 0xFF41..=0xFF5A | 0x10780..=0x107BA | 0x1DF00..=0x1DF2A)
}

/// `\p{C}`: control, format, private-use and unassigned characters (the
/// assigned ranges a text can hold).
pub fn is_other_category(c: char) -> bool {
    c.is_control()
        || matches!(c as u32,
            0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E
            | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB | 0xE000..=0xF8FF
            | 0xE0001 | 0xE0020..=0xE007F | 0xF0000..=0x10FFFF)
}

/// `\p{Zs}`: the space separators.
pub fn is_space_separator(c: char) -> bool {
    matches!(
        c as u32,
        0x20 | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x202F | 0x205F | 0x3000
    )
}

/// `LevenshteinDistance.getDefaultInstance().apply` (Apache Commons Text):
/// insertions, deletions and substitutions, no limit.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<u16> = a.encode_utf16().collect();
    let b: Vec<u16> = b.encode_utf16().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut row: Vec<usize> = (0..=a.len()).collect();
    for (j, cb) in b.iter().enumerate() {
        let mut upper_left = row[0];
        row[0] = j + 1;
        for (i, ca) in a.iter().enumerate() {
            let upper = row[i + 1];
            let cost = usize::from(ca != cb);
            row[i + 1] = (row[i] + 1).min(upper + 1).min(upper_left + cost);
            upper_left = upper;
        }
    }
    row[a.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diacritics() {
        assert_eq!(remove_diacritics("Éléphant à l'œil"), "Elephant a l'œil");
        assert_eq!(remove_diacritics("a\u{300} garçon"), "a garcon");
        assert_eq!(remove_diacritics("ǘ"), "u");
    }

    #[test]
    fn wrong_words() {
        assert_eq!(make_wrong("chat"), "chät");
        assert_eq!(make_wrong("livre"), "livrë");
        assert_eq!(make_wrong("né"), "në");
        assert_eq!(make_wrong("brr"), "brr-");
    }

    #[test]
    fn splitting() {
        assert_eq!(split_camel_case("HelloWorld"), ["Hello", "World"]);
        assert_eq!(split_camel_case("helloWorld"), ["hello", "World"]);
        assert_eq!(split_camel_case("ABC"), ["ABC"]);
        assert_eq!(split_camel_case("iPhone"), ["i", "Phone"]);
        assert_eq!(split_camel_case("maison"), ["maison"]);
        assert_eq!(split_digits_at_end("mot12"), ["mot", "12"]);
        assert_eq!(split_digits_at_end("12"), ["12"]);
        assert_eq!(split_digits_at_end("mot"), ["mot"]);
        assert_eq!(java_split("a b ", ' '), ["a", "b"]);
        assert_eq!(java_split(" a", ' '), ["", "a"]);
        assert_eq!(java_split("", ' '), [""]);
        assert_eq!(split_non_empty(" a  b ", ' '), ["a", "b"]);
        assert_eq!(substring_before("mot #note", '#'), "mot ");
    }

    #[test]
    fn predicates() {
        assert!(is_anagram("chien", "niche"));
        assert!(!is_anagram("chien", "chat"));
        assert!(is_not_word_string("12-3"));
        assert!(!is_not_word_string("a1"));
        assert!(!is_not_word_string(""));
        assert!(
            is_latin_script('é')
                && is_latin_script('œ')
                && !is_latin_script('1')
                && !is_latin_script('я')
        );
        assert!(
            !is_latin_script('\u{300}'),
            "combining marks have the Inherited script"
        );
    }

    #[test]
    fn java_hashing() {
        assert_eq!(java_hash(""), 0);
        assert_eq!(java_hash("a"), 97);
        assert_eq!(java_hash("hello"), 99162322);
        assert_eq!(java_hash("new york"), 1319303759);
        // "Aa" and "BB" collide: insertion order decides inside the bucket.
        assert_eq!(java_hash_map_order(&["BB", "Aa", "b", "a"]), [0, 1, 3, 2]);
    }

    #[test]
    fn edit_distance() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("ab", "ba"), 2);
    }
}
