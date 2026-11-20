//! Dictionary metadata: the `.info` file that accompanies a `.dict` automaton.
//!
//! Port of `morfologik.stemming.DictionaryMetadata`, `DictionaryAttribute`
//! and `EncoderType` (Morfologik 2.1.9, BSD-3-Clause), with a reader for the
//! `java.util.Properties` text format the `.info` files are written in.
//!
//! Differences with Java:
//! * only the UTF-8 encoding is supported (`fsa.dict.encoding`), which is what
//!   LanguageTool's dictionaries use; another encoding is an error;
//! * a property that Morfologik does not know is kept in
//!   [`DictionaryMetadata::attributes`] instead of being an error.

use std::collections::BTreeMap;
use std::io;

/// `EncoderType`: how the stem of an entry is encoded relative to its word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncoderType {
    /// `TrimSuffixEncoder`: one byte, the number of bytes to cut at the end.
    Suffix,
    /// `TrimPrefixAndSuffixEncoder`: two bytes (prefix cut, suffix cut).
    Prefix,
    /// `TrimInfixAndSuffixEncoder`: three bytes (infix start, infix length,
    /// suffix cut).
    Infix,
    /// `NoEncoder`: the stem is stored verbatim.
    None,
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// Property names (`DictionaryAttribute.propertyName`).
pub mod attr {
    pub const SEPARATOR: &str = "fsa.dict.separator";
    pub const ENCODING: &str = "fsa.dict.encoding";
    pub const FREQUENCY_INCLUDED: &str = "fsa.dict.frequency-included";
    pub const IGNORE_NUMBERS: &str = "fsa.dict.speller.ignore-numbers";
    pub const IGNORE_PUNCTUATION: &str = "fsa.dict.speller.ignore-punctuation";
    pub const IGNORE_CAMEL_CASE: &str = "fsa.dict.speller.ignore-camel-case";
    pub const IGNORE_ALL_UPPERCASE: &str = "fsa.dict.speller.ignore-all-uppercase";
    pub const IGNORE_DIACRITICS: &str = "fsa.dict.speller.ignore-diacritics";
    pub const CONVERT_CASE: &str = "fsa.dict.speller.convert-case";
    pub const RUN_ON_WORDS: &str = "fsa.dict.speller.runon-words";
    pub const LOCALE: &str = "fsa.dict.speller.locale";
    pub const ENCODER: &str = "fsa.dict.encoder";
    pub const INPUT_CONVERSION: &str = "fsa.dict.input-conversion";
    pub const OUTPUT_CONVERSION: &str = "fsa.dict.output-conversion";
    pub const REPLACEMENT_PAIRS: &str = "fsa.dict.speller.replacement-pairs";
    pub const EQUIVALENT_CHARS: &str = "fsa.dict.speller.equivalent-chars";
    pub const LICENSE: &str = "fsa.dict.license";
    pub const AUTHOR: &str = "fsa.dict.author";
    pub const CREATION_DATE: &str = "fsa.dict.created";
}

/// Description of a dictionary: port of `DictionaryMetadata`.
///
/// Maps that are `LinkedHashMap`s in Java are vectors of pairs here, in the
/// same (insertion) order, which the speller relies on.
#[derive(Clone, Debug)]
pub struct DictionaryMetadata {
    /// Every property of the `.info` file, by name (unknown ones included).
    pub attributes: BTreeMap<String, String>,
    /// `fsa.dict.separator` as a byte of the automaton.
    pub separator: u8,
    /// `fsa.dict.separator` as a character.
    pub separator_char: char,
    /// `fsa.dict.encoding`, as written in the file.
    pub encoding: String,
    /// `fsa.dict.encoder`.
    pub encoder_type: EncoderType,
    /// `fsa.dict.speller.locale`, if given (Java falls back to the JVM's
    /// default locale; this port lowercases without locale tailoring).
    pub locale: Option<String>,
    /// `fsa.dict.speller.replacement-pairs`: for each source string, in file
    /// order, its replacements.
    pub replacement_pairs: Vec<(String, Vec<String>)>,
    /// `fsa.dict.input-conversion`: applied to a word before it is looked up.
    pub input_conversion_pairs: Vec<(String, String)>,
    /// `fsa.dict.output-conversion`: applied to what the dictionary returns.
    pub output_conversion_pairs: Vec<(String, String)>,
    /// `fsa.dict.speller.equivalent-chars`: characters the speller treats as
    /// equal (cost 0) to the key character.
    pub equivalent_chars: Vec<(char, Vec<char>)>,
    /// `fsa.dict.frequency-included`: the last byte of every tag is a
    /// frequency class `'A'..='Z'`. Default: false.
    pub frequency_included: bool,
    /// `fsa.dict.speller.ignore-punctuation`. Default: true.
    pub ignore_punctuation: bool,
    /// `fsa.dict.speller.ignore-numbers`. Default: true.
    pub ignore_numbers: bool,
    /// `fsa.dict.speller.ignore-camel-case`. Default: true.
    pub ignore_camel_case: bool,
    /// `fsa.dict.speller.ignore-all-uppercase`. Default: true.
    pub ignore_all_uppercase: bool,
    /// `fsa.dict.speller.ignore-diacritics`. Default: true.
    pub ignore_diacritics: bool,
    /// `fsa.dict.speller.convert-case`. Default: true.
    pub convert_case: bool,
    /// `fsa.dict.speller.runon-words`. Default: true.
    pub run_on_words: bool,
}

impl DictionaryMetadata {
    /// `DictionaryMetadata.read`: parses the text of a `.info` file.
    pub fn parse(info: &str) -> io::Result<DictionaryMetadata> {
        let mut attributes = BTreeMap::new();
        for (k, v) in parse_properties(info) {
            attributes.insert(k, v);
        }
        Self::from_attributes(attributes)
    }

    /// Builds the metadata from a property map (`new DictionaryMetadata(map)`).
    pub fn from_attributes(attributes: BTreeMap<String, String>) -> io::Result<DictionaryMetadata> {
        let get = |name: &str| attributes.get(name).map(String::as_str);

        // Required attributes: separator, encoder, encoding.
        let Some(encoder) = get(attr::ENCODER) else {
            return Err(invalid(format!(
                "Use an explicit {}=... metadata key (SUFFIX, PREFIX, INFIX or NONE).",
                attr::ENCODER
            )));
        };
        let encoder_type = match encoder.trim().to_uppercase().as_str() {
            "SUFFIX" => EncoderType::Suffix,
            "PREFIX" => EncoderType::Prefix,
            "INFIX" => EncoderType::Infix,
            "NONE" => EncoderType::None,
            _ => {
                return Err(invalid(format!(
                    "Invalid encoder name '{}', only these coders are valid: \
                     [SUFFIX, PREFIX, INFIX, NONE]",
                    encoder.trim()
                )))
            }
        };

        let (Some(separator), Some(encoding)) = (get(attr::SEPARATOR), get(attr::ENCODING)) else {
            return Err(invalid(
                "At least one the required attributes was not provided: \
                 fsa.dict.separator, fsa.dict.encoding"
                    .to_string(),
            ));
        };
        let mut sep_chars = separator.chars();
        let separator_char = match (sep_chars.next(), sep_chars.next()) {
            (Some(c), None) if c.len_utf16() == 1 => c,
            _ => {
                return Err(invalid(format!(
                    "Attribute {} must be a single character.",
                    attr::SEPARATOR
                )))
            }
        };

        let normalized: String = encoding
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if normalized != "utf8" {
            return Err(invalid(format!(
                "Encoding not supported by this reader (UTF-8 only): {encoding}"
            )));
        }
        if !separator_char.is_ascii() {
            return Err(invalid(format!(
                "Separator character is not a single byte in encoding {encoding}: {separator_char}"
            )));
        }

        let boolean = |name: &str, default: bool| -> io::Result<bool> {
            match get(name) {
                None => Ok(default),
                Some(v) => boolean_value(v),
            }
        };

        let replacement_pairs = match get(attr::REPLACEMENT_PAIRS) {
            None => Vec::new(),
            Some(v) => {
                let mut pairs: Vec<(String, Vec<String>)> = Vec::new();
                for (from, to) in parse_pairs(attr::REPLACEMENT_PAIRS, v)? {
                    match pairs.iter_mut().find(|(k, _)| *k == from) {
                        Some((_, list)) => list.push(to),
                        None => pairs.push((from, vec![to])),
                    }
                }
                pairs
            }
        };

        let conversion = |name: &str| -> io::Result<Vec<(String, String)>> {
            let Some(v) = get(name) else {
                return Ok(Vec::new());
            };
            let mut pairs: Vec<(String, String)> = Vec::new();
            for (from, to) in parse_pairs(name, v)? {
                if pairs.iter().any(|(k, _)| *k == from) {
                    return Err(invalid(format!(
                        "Input conversion cannot specify different values for the same input string: {from}"
                    )));
                }
                pairs.push((from, to));
            }
            Ok(pairs)
        };

        let equivalent_chars = match get(attr::EQUIVALENT_CHARS) {
            None => Vec::new(),
            Some(v) => {
                let mut map: Vec<(char, Vec<char>)> = Vec::new();
                for (from, to) in parse_pairs(attr::EQUIVALENT_CHARS, v)? {
                    let (Some(f), Some(t)) = (single_utf16_char(&from), single_utf16_char(&to))
                    else {
                        return Err(invalid(format!(
                            "Attribute {} is not in the proper format: {v}",
                            attr::EQUIVALENT_CHARS
                        )));
                    };
                    match map.iter_mut().find(|(k, _)| *k == f) {
                        Some((_, list)) => list.push(t),
                        None => map.push((f, vec![t])),
                    }
                }
                map
            }
        };

        Ok(DictionaryMetadata {
            separator: separator_char as u8,
            separator_char,
            encoding: encoding.to_string(),
            encoder_type,
            locale: get(attr::LOCALE).map(str::to_string),
            replacement_pairs,
            input_conversion_pairs: conversion(attr::INPUT_CONVERSION)?,
            output_conversion_pairs: conversion(attr::OUTPUT_CONVERSION)?,
            equivalent_chars,
            frequency_included: boolean(attr::FREQUENCY_INCLUDED, false)?,
            ignore_punctuation: boolean(attr::IGNORE_PUNCTUATION, true)?,
            ignore_numbers: boolean(attr::IGNORE_NUMBERS, true)?,
            ignore_camel_case: boolean(attr::IGNORE_CAMEL_CASE, true)?,
            ignore_all_uppercase: boolean(attr::IGNORE_ALL_UPPERCASE, true)?,
            ignore_diacritics: boolean(attr::IGNORE_DIACRITICS, true)?,
            convert_case: boolean(attr::CONVERT_CASE, true)?,
            run_on_words: boolean(attr::RUN_ON_WORDS, true)?,
            attributes,
        })
    }

    /// `getExpectedMetadataFileName`: the `.info` file name for a dictionary
    /// file name (the extension is replaced, or `.info` appended).
    pub fn expected_metadata_file_name(dictionary_file: &str) -> String {
        match dictionary_file.rfind('.') {
            Some(dot) => format!("{}.info", &dictionary_file[..dot]),
            None => format!("{dictionary_file}.info"),
        }
    }

    /// The raw value of a property, if the file defines it.
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(String::as_str)
    }
}

/// The string as one UTF-16 code unit (Java's `s.length() == 1`).
fn single_utf16_char(s: &str) -> Option<char> {
    let mut it = s.chars();
    match (it.next(), it.next()) {
        (Some(c), None) if c.len_utf16() == 1 => Some(c),
        _ => None,
    }
}

/// `DictionaryAttribute.booleanValue`.
fn boolean_value(value: &str) -> io::Result<bool> {
    match value.to_lowercase().as_str() {
        "true" | "yes" | "on" => Ok(true),
        "false" | "no" | "off" => Ok(false),
        _ => Err(invalid(format!("Not a boolean value: {value}"))),
    }
}

/// Java's `\s` (without `UNICODE_CHARACTER_CLASS`).
fn is_java_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r')
}

/// `value.split(",\\s*")`, each item then `trim().split(" ")` and required to
/// have exactly two parts: the syntax shared by the pair-list attributes.
fn parse_pairs(name: &str, value: &str) -> io::Result<Vec<(String, String)>> {
    // split(",\\s*"): cut at each comma and swallow the white space after it.
    let mut items: Vec<&str> = Vec::new();
    let mut rest = value;
    while let Some(comma) = rest.find(',') {
        items.push(&rest[..comma]);
        rest = rest[comma + 1..].trim_start_matches(is_java_space);
    }
    items.push(rest);
    // String.split drops trailing empty strings (but keeps a lone "").
    while items.len() > 1 && items.last() == Some(&"") {
        items.pop();
    }

    let mut pairs = Vec::with_capacity(items.len());
    for item in items {
        // String.trim() removes every char <= U+0020 at both ends.
        let item = item.trim_matches(|c: char| c <= ' ');
        let mut parts: Vec<&str> = item.split(' ').collect();
        while parts.len() > 1 && parts.last() == Some(&"") {
            parts.pop();
        }
        if parts.len() != 2 {
            return Err(invalid(format!(
                "Attribute {name} is not in the proper format: {value}"
            )));
        }
        pairs.push((parts[0].to_string(), parts[1].to_string()));
    }
    Ok(pairs)
}

/// Reads the `java.util.Properties` text format (`Properties.load(Reader)`).
///
/// Returns the pairs in file order; a later definition of a key overrides an
/// earlier one when they are collected in a map.
pub fn parse_properties(text: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    let is_ws = |c: char| c == ' ' || c == '\t' || c == '\u{0C}';
    let is_eol = |c: char| c == '\n' || c == '\r';

    while i < n {
        // Skip leading white space and blank lines.
        while i < n && (is_ws(chars[i]) || is_eol(chars[i])) {
            i += 1;
        }
        if i >= n {
            break;
        }
        // Comment line.
        if chars[i] == '#' || chars[i] == '!' {
            while i < n && !is_eol(chars[i]) {
                i += 1;
            }
            continue;
        }
        // Gather the logical line, joining continuation lines.
        let mut line: Vec<char> = Vec::new();
        loop {
            let start = i;
            while i < n && !is_eol(chars[i]) {
                i += 1;
            }
            let natural = &chars[start..i];
            // Consume the line terminator (\r\n counts as one).
            if i < n {
                if chars[i] == '\r' && i + 1 < n && chars[i + 1] == '\n' {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let backslashes = natural.iter().rev().take_while(|&&c| c == '\\').count();
            if backslashes % 2 == 1 {
                // Continuation: drop the backslash, skip the leading white
                // space of the next line.
                line.extend_from_slice(&natural[..natural.len() - 1]);
                while i < n && is_ws(chars[i]) {
                    i += 1;
                }
                if i >= n {
                    break;
                }
            } else {
                line.extend_from_slice(natural);
                break;
            }
        }

        // Split key and value at the first unescaped '=', ':' or white space.
        let len = line.len();
        let mut key_end = 0;
        let mut value_start = len;
        let mut escaped = false;
        let mut has_sep = false;
        while key_end < len {
            let c = line[key_end];
            if (c == '=' || c == ':') && !escaped {
                value_start = key_end + 1;
                has_sep = true;
                break;
            } else if is_ws(c) && !escaped {
                value_start = key_end + 1;
                break;
            }
            escaped = c == '\\' && !escaped;
            key_end += 1;
        }
        // Skip white space around the separator, and the separator itself if
        // the key ended on white space.
        while value_start < len {
            let c = line[value_start];
            if !is_ws(c) {
                if !has_sep && (c == '=' || c == ':') {
                    has_sep = true;
                } else {
                    break;
                }
            }
            value_start += 1;
        }
        out.push((
            unescape_property(&line[..key_end]),
            unescape_property(&line[value_start..]),
        ));
    }
    out
}

/// `Properties.loadConvert`: resolves `\t \n \r \f`, `\uXXXX` and `\x`.
fn unescape_property(chars: &[char]) -> String {
    let mut out = String::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if c != '\\' || i >= chars.len() {
            if c != '\\' {
                out.push(c);
            }
            continue;
        }
        let e = chars[i];
        i += 1;
        match e {
            't' => out.push('\t'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            'f' => out.push('\u{0C}'),
            'u' => {
                let hex: String = chars[i..chars.len().min(i + 4)].iter().collect();
                match u32::from_str_radix(&hex, 16) {
                    Ok(v) if hex.len() == 4 => {
                        i += 4;
                        out.push(char::from_u32(v).unwrap_or('\u{FFFD}'));
                    }
                    // Java throws on a malformed \uxxxx; keep the text.
                    _ => out.push('u'),
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_format() {
        let text = "# comment\n! other comment\n\
                    a=1\n  b : two words  \nc\td\ne\\ f = g\\u00e9\\n\n\
                    long = one,\\\n   two\nempty=\nnovalue\n";
        let p = parse_properties(text);
        let get = |k: &str| p.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("a"), Some("1"));
        assert_eq!(get("b"), Some("two words  "));
        assert_eq!(get("c"), Some("d"));
        assert_eq!(get("e f"), Some("gé\n"));
        assert_eq!(get("long"), Some("one,two"));
        assert_eq!(get("empty"), Some(""));
        assert_eq!(get("novalue"), Some(""));
        assert_eq!(p.len(), 7);
    }

    #[test]
    fn pair_lists() {
        let m = DictionaryMetadata::parse(
            "fsa.dict.separator=+\nfsa.dict.encoding=UTF-8\nfsa.dict.encoder=infix\n\
             fsa.dict.speller.replacement-pairs=a b, a c,d e, qu qu'\n\
             fsa.dict.speller.equivalent-chars=x y, x z\n\
             fsa.dict.input-conversion=ĳ ij\n",
        )
        .unwrap();
        assert_eq!(m.separator, b'+');
        assert_eq!(m.encoder_type, EncoderType::Infix);
        assert_eq!(
            m.replacement_pairs,
            vec![
                ("a".to_string(), vec!["b".to_string(), "c".to_string()]),
                ("d".to_string(), vec!["e".to_string()]),
                ("qu".to_string(), vec!["qu'".to_string()]),
            ]
        );
        assert_eq!(m.equivalent_chars, vec![('x', vec!['y', 'z'])]);
        assert_eq!(
            m.input_conversion_pairs,
            vec![("ĳ".to_string(), "ij".to_string())]
        );
        // Defaults of DictionaryMetadata.DEFAULT_ATTRIBUTES.
        assert!(!m.frequency_included);
        assert!(m.ignore_punctuation && m.ignore_numbers && m.ignore_camel_case);
        assert!(m.ignore_all_uppercase && m.ignore_diacritics && m.convert_case);
        assert!(m.run_on_words);
    }

    #[test]
    fn required_attributes_and_errors() {
        assert!(
            DictionaryMetadata::parse("fsa.dict.separator=+\nfsa.dict.encoding=utf8\n").is_err()
        );
        assert!(
            DictionaryMetadata::parse("fsa.dict.encoder=SUFFIX\nfsa.dict.encoding=utf8\n").is_err()
        );
        assert!(DictionaryMetadata::parse(
            "fsa.dict.separator=+\nfsa.dict.encoder=SUFFIX\nfsa.dict.encoding=iso-8859-2\n"
        )
        .is_err());
        assert!(DictionaryMetadata::parse(
            "fsa.dict.separator=+\nfsa.dict.encoder=SUFFIX\nfsa.dict.encoding=utf8\n\
             fsa.dict.speller.convert-case=maybe\n"
        )
        .is_err());
        assert!(DictionaryMetadata::parse(
            "fsa.dict.separator=+\nfsa.dict.encoder=SUFFIX\nfsa.dict.encoding=utf8\n\
             fsa.dict.speller.replacement-pairs=a b c\n"
        )
        .is_err());
    }

    #[test]
    fn metadata_file_name() {
        assert_eq!(
            DictionaryMetadata::expected_metadata_file_name("french.dict"),
            "french.info"
        );
        assert_eq!(
            DictionaryMetadata::expected_metadata_file_name("french"),
            "french.info"
        );
    }
}
