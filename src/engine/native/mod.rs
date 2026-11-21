//! LanguageTool's French rules that are written in Java rather than in the
//! XML rule files, in the order `French.getRelevantRules` creates them
//! (`org.languagetool.language.French`, LanguageTool, LGPL-2.1-or-later).
//!
//! Each submodule ports one Java class and names it. Positions are byte
//! offsets where Java counts UTF-16 units; wherever Java does arithmetic on
//! positions or lengths the port steps over whole characters instead.
//!
//! `AbstractCompoundRule` and `AbstractSimpleReplaceRule` report each match
//! under an id of its own (`FR_COMPOUNDS_ARC_EN_CIEL`), through a
//! `SpecificIdRule`. A match here refers to its rule by index, so these rules
//! come with one [`SpecificIdRule`] per id they can report, placed right
//! after them in the rule list.

use std::io;
use std::path::Path;

use super::checker::NativeRule;
use super::rule::{RuleInfo, RuleMatch};
use super::token::Sentence;

pub mod antipattern;
pub mod comma_whitespace;
pub mod compound;
pub mod double_punctuation;
pub mod long_paragraph;
pub mod long_sentence;
pub mod messages;
pub mod multiple_whitespace;
pub mod question_whitespace;
pub mod repeated_words;
pub mod sentence_whitespace;
pub mod simple_replace;
pub mod unpaired_brackets;
pub mod uppercase_sentence_start;

/// The Java-coded rules of French, in LanguageTool's order. `speller` takes
/// the place of `MorfologikFrenchSpellerRule` (fourth in the list) when given.
///
/// `assets` is LanguageTool's French resource directory (`compounds.txt`,
/// `replace.txt`, `replace_custom.txt`, `synonyms.txt`).
pub fn french_native_rules(
    assets: &Path,
    speller: Option<Box<dyn NativeRule>>,
) -> io::Result<Vec<Box<dyn NativeRule>>> {
    let read = |name: &str| std::fs::read_to_string(assets.join(name));
    let read_optional = |name: &str| match std::fs::read_to_string(assets.join(name)) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    };
    let invalid = |message: String| io::Error::new(io::ErrorKind::InvalidData, message);

    let mut rules: Vec<Box<dyn NativeRule>> = Vec::new();
    rules.push(Box::new(comma_whitespace::CommaWhitespaceRule::new(false)));
    rules.push(Box::new(double_punctuation::DoublePunctuationRule::new()));
    rules.push(Box::new(
        unpaired_brackets::GenericUnpairedBracketsRule::new(&["[", "(", "{"], &["]", ")", "}"]),
    ));
    // MorfologikFrenchSpellerRule
    rules.extend(speller);
    rules.push(Box::new(
        uppercase_sentence_start::UppercaseSentenceStartRule::new(),
    ));
    rules.push(Box::new(multiple_whitespace::MultipleWhitespaceRule::new()));
    rules.push(Box::new(sentence_whitespace::SentenceWhitespaceRule::new(
        1,
    )));
    rules.push(Box::new(long_sentence::LongSentenceRule::new(40)));
    rules.push(Box::new(long_paragraph::LongParagraphRule::new()));
    let (compound, ids) =
        compound::CompoundRule::french(&read("compounds.txt")?).map_err(invalid)?;
    rules.push(Box::new(compound));
    rules.extend(ids.into_iter().map(|r| Box::new(r) as Box<dyn NativeRule>));
    rules.push(Box::new(question_whitespace::QuestionWhitespaceRule::new(
        true,
    )));
    rules.push(Box::new(question_whitespace::QuestionWhitespaceRule::new(
        false,
    )));
    let (replace, ids) = simple_replace::SimpleReplaceRule::french(&[
        &read("replace.txt")?,
        &read_optional("replace_custom.txt")?,
    ])
    .map_err(invalid)?;
    rules.push(Box::new(replace));
    rules.extend(ids.into_iter().map(|r| Box::new(r) as Box<dyn NativeRule>));
    rules.push(Box::new(
        repeated_words::FrenchRepeatedWordsRule::new(&read("synonyms.txt")?).map_err(invalid)?,
    ));
    Ok(rules)
}

/// A rule category (`Categories`), with its French name.
pub(crate) struct Category {
    pub id: &'static str,
    pub name: &'static str,
}

pub(crate) const CASING: Category = Category {
    id: "CASING",
    name: messages::CATEGORY_CASE,
};
pub(crate) const TYPOS: Category = Category {
    id: "TYPOS",
    name: messages::CATEGORY_TYPO,
};
pub(crate) const PUNCTUATION: Category = Category {
    id: "PUNCTUATION",
    name: messages::CATEGORY_PUNCTUATION,
};
pub(crate) const TYPOGRAPHY: Category = Category {
    id: "TYPOGRAPHY",
    name: messages::CATEGORY_TYPOGRAPHY,
};
pub(crate) const REPETITIONS_STYLE: Category = Category {
    id: "REPETITIONS_STYLE",
    name: messages::CATEOGRY_REPETITIONS_STYLE,
};
pub(crate) const STYLE: Category = Category {
    id: "STYLE",
    name: messages::CATEGORY_STYLE,
};
pub(crate) const MISC: Category = Category {
    id: "MISC",
    name: messages::CATEGORY_MISC,
};

/// The metadata of a Java rule: default on, no sub-id.
pub(crate) fn rule_info(
    id: &str,
    description: &str,
    category: &Category,
    issue_type: &str,
) -> RuleInfo {
    RuleInfo {
        id: id.to_string(),
        description: description.to_string(),
        category_id: category.id.to_string(),
        category_name: category.name.to_string(),
        issue_type: issue_type.to_string(),
        ..RuleInfo::default()
    }
}

/// `new RuleMatch(rule, sentence, fromPos, toPos, message, shortMessage)`
/// followed by `setSuggestedReplacements`. `None` when the span is empty,
/// where Java throws.
pub(crate) fn rule_match(
    rule: usize,
    from: usize,
    to: usize,
    message: &str,
    short_message: &str,
    suggestions: Vec<String>,
) -> Option<RuleMatch> {
    if to <= from {
        return None;
    }
    Some(RuleMatch {
        rule,
        from_pos: from,
        to_pos: to,
        pattern_from: from,
        pattern_to: to,
        message: message.to_string(),
        short_message: short_message.to_string(),
        suggestions,
        original_error_str: String::new(),
    })
}

/// `SpecificIdRule`: the id and description one match of a compound or
/// replacement rule is reported under. It never matches by itself.
pub struct SpecificIdRule {
    info: RuleInfo,
}

impl SpecificIdRule {
    /// A rule that differs from `parent` by its id and description only.
    pub(crate) fn new(id: String, description: String, parent: &RuleInfo) -> SpecificIdRule {
        SpecificIdRule {
            info: RuleInfo {
                id,
                description,
                ..parent.clone()
            },
        }
    }
}

impl NativeRule for SpecificIdRule {
    fn info(&self) -> &RuleInfo {
        &self.info
    }

    fn never_matches(&self) -> bool {
        true
    }
}

/// The ids a rule reports its matches under: one [`SpecificIdRule`] per
/// distinct id, and for each key (a compound, a wrong word) the position of
/// its rule after the parent.
pub(crate) struct SpecificIds {
    by_key: std::collections::HashMap<String, usize>,
}

impl SpecificIds {
    /// `keys` with the id and description of each; keys that share an id
    /// share the rule (and the description of the first).
    pub(crate) fn new(
        keys: impl Iterator<Item = (String, String, String)>,
        parent: &RuleInfo,
    ) -> (SpecificIds, Vec<SpecificIdRule>) {
        let mut by_key = std::collections::HashMap::new();
        let mut by_id: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut rules = Vec::new();
        for (key, id, description) in keys {
            let index = *by_id.entry(id.clone()).or_insert_with(|| {
                rules.push(SpecificIdRule::new(id, description, parent));
                rules.len() - 1
            });
            by_key.insert(key, index);
        }
        (SpecificIds { by_key }, rules)
    }

    /// The rule index to report a match of `key` under, `parent` being the
    /// index of the matching rule itself (used when the key is unknown).
    pub(crate) fn rule_index(&self, parent: usize, key: &str) -> usize {
        match self.by_key.get(key) {
            Some(offset) => parent + 1 + offset,
            None => parent,
        }
    }
}

/// `StringTools.toId` for a language other than German: upper case, spaces
/// to `_`, apostrophes to `_Q_`, then every UTF-16 unit outside `A-Z`,
/// `À-Ö` and `Ø-Þ` to `_`.
pub(crate) fn to_id(input: &str) -> String {
    let upper = input.to_uppercase();
    let mut out = String::with_capacity(upper.len());
    for c in java_trim(&upper).chars() {
        match c {
            '\'' => out.push_str("_Q_"),
            'A'..='Z' | '\u{c0}'..='\u{d6}' | '\u{d8}'..='\u{de}' => out.push(c),
            _ => {
                for _ in 0..c.len_utf16() {
                    out.push('_');
                }
            }
        }
    }
    out
}

/// Java's `String.trim()`: strips the chars up to U+0020 at both ends.
pub(crate) fn java_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c <= ' ')
}

/// Java's `String.split(sep)` for a one-char separator: trailing empty
/// strings are dropped, and a string without the separator is returned whole
/// (even when empty).
pub(crate) fn java_split(s: &str, sep: char) -> Vec<&str> {
    if !s.contains(sep) {
        return vec![s];
    }
    let mut parts: Vec<&str> = s.split(sep).collect();
    while parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    parts
}

/// `Character.isDigit` on a Java `char`: a decimal digit of the Basic
/// Multilingual Plane.
pub(crate) fn is_java_digit(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_digit();
    }
    const ZEROS: [u32; 36] = [
        0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66, 0xde6,
        0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90,
        0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0,
        0xff10,
    ];
    let c = c as u32;
    ZEROS.iter().any(|&zero| (zero..zero + 10).contains(&c))
}

/// A line terminator, which `.` does not match in a Java regular expression.
pub(crate) fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// The start of the character that ends at byte `pos` of `text` (`pos - 1`
/// in Java, where the rules step back over one-unit characters).
pub(crate) fn prev_char_start(text: &str, pos: usize) -> usize {
    text[..pos].char_indices().next_back().map_or(0, |(i, _)| i)
}

/// The text a text-level rule works on: the sentences' texts joined.
pub(crate) fn full_text(sentences: &[Sentence]) -> String {
    let mut text = String::with_capacity(sentences.iter().map(|s| s.text.len()).sum());
    for s in sentences {
        text.push_str(&s.text);
    }
    text
}

#[cfg(test)]
pub(crate) mod testing {
    //! Sentences for the rule tests.

    use std::sync::Arc;

    use super::super::pattern::Lang;
    use super::super::rule::RuleContext;
    use super::super::token::{Reading, Readings, Sentence, SENTENCE_START};
    use super::super::unifier::UnifierConfiguration;

    pub fn context() -> RuleContext {
        RuleContext {
            lang: Lang::default(),
            unifier: UnifierConfiguration::default(),
            disambig_unifier: UnifierConfiguration::default(),
        }
    }

    /// A sentence from its tokens (whitespace tokens included), each
    /// `"token"` or `"token/lemma/TAG"`; a `*` in front immunizes the token.
    pub fn sentence(tokens: &[&str]) -> Sentence {
        let mut out = vec![Readings::new(
            vec![Reading::from_strs("", Some(SENTENCE_START), None)],
            0,
        )];
        let mut text = String::new();
        let mut prev: Arc<str> = Arc::from("");
        for spec in tokens {
            let (immunized, spec) = match spec.strip_prefix('*') {
                Some(rest) if !rest.is_empty() => (true, rest),
                _ => (false, *spec),
            };
            let mut parts = spec.splitn(3, '/');
            let token = parts.next().unwrap_or("");
            let (token, lemma, tag) = match (parts.next(), parts.next()) {
                (Some(lemma), Some(tag)) if !token.is_empty() => (token, Some(lemma), Some(tag)),
                _ => (spec, None, None),
            };
            let mut readings =
                Readings::new(vec![Reading::from_strs(token, tag, lemma)], text.len());
            readings.set_whitespace_before(&prev);
            readings.immunized = immunized;
            prev = readings.token.clone();
            text.push_str(token);
            out.push(readings);
        }
        if let Some(last) = out.iter_mut().skip(1).rev().find(|t| !t.is_whitespace) {
            last.set_sent_end();
        }
        Sentence::new(Arc::from(text.as_str()), out.clone(), out)
    }

    /// A sentence cut into tokens at each whitespace character and around
    /// each punctuation mark.
    pub fn plain(text: &str) -> Sentence {
        let mut tokens: Vec<String> = Vec::new();
        let mut word = String::new();
        for c in text.chars() {
            if c.is_alphanumeric() || c == '-' || c == '\'' {
                word.push(c);
            } else {
                if !word.is_empty() {
                    tokens.push(std::mem::take(&mut word));
                }
                tokens.push(c.to_string());
            }
        }
        if !word.is_empty() {
            tokens.push(word);
        }
        let refs: Vec<&str> = tokens.iter().map(String::as_str).collect();
        sentence(&refs)
    }
}

#[cfg(test)]
mod tests {
    use super::super::checker::{CheckRule, Checker};
    use super::*;

    fn french_rules() -> Vec<Box<dyn NativeRule>> {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fr/lt");
        french_native_rules(&assets, None).expect("LanguageTool's French data")
    }

    #[test]
    fn rules_come_in_languagetool_order() {
        let rules = french_rules();
        let ids: Vec<&str> = rules
            .iter()
            .filter(|r| !r.never_matches())
            .map(|r| r.info().id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "COMMA_PARENTHESIS_WHITESPACE",
                "DOUBLE_PUNCTUATION",
                "UNPAIRED_BRACKETS",
                "UPPERCASE_SENTENCE_START",
                "WHITESPACE_RULE",
                "SENTENCE_WHITESPACE",
                "TOO_LONG_SENTENCE",
                "TOO_LONG_PARAGRAPH",
                "FR_COMPOUNDS",
                "FRENCH_WHITESPACE_STRICT",
                "FRENCH_WHITESPACE",
                "FR_SIMPLE_REPLACE_SIMPLE",
                "FR_REPEATEDWORDS",
            ]
        );
        // The ids of a rule's matches follow it.
        let compounds = rules
            .iter()
            .position(|r| r.info().id == "FR_COMPOUNDS")
            .unwrap();
        let next = rules
            .iter()
            .position(|r| r.info().id == "FRENCH_WHITESPACE_STRICT")
            .unwrap();
        assert!(next - compounds > 1000);
        assert!(rules[compounds + 1..next]
            .iter()
            .all(|r| r.never_matches() && r.info().id.starts_with("FR_COMPOUNDS_")));
        assert!(rules[compounds + 1..next]
            .iter()
            .any(|r| r.info().id == "FR_COMPOUNDS_ARC_EN_CIEL"));
    }

    #[test]
    fn matches_through_the_checker() {
        let rules: Vec<CheckRule> = french_rules().into_iter().map(CheckRule::Native).collect();
        let checker = Checker::new(rules, testing::context());
        let sentences = [
            testing::plain("bonjour ,un arc en ciel."),
            testing::plain("Jai faim (oui?"),
        ];
        let found: Vec<(&str, usize, usize, Vec<String>)> = checker
            .check(&sentences)
            .into_iter()
            .map(|m| {
                (
                    checker.info(m.rule).id.as_str(),
                    m.from,
                    m.to,
                    m.suggestions,
                )
            })
            .collect();
        let expected: Vec<(&str, usize, usize, Vec<String>)> = vec![
            (
                "UPPERCASE_SENTENCE_START",
                0,
                7,
                vec!["Bonjour".to_string()],
            ),
            ("COMMA_PARENTHESIS_WHITESPACE", 7, 9, vec![", ".to_string()]),
            (
                "FR_COMPOUNDS_ARC_EN_CIEL",
                12,
                23,
                vec!["arc-en-ciel".to_string()],
            ),
            (
                "FR_SIMPLE_REPLACE_SIMPLE_JAI",
                24,
                27,
                vec!["J'ai".to_string()],
            ),
            ("UNPAIRED_BRACKETS", 33, 34, vec![]),
            (
                "FRENCH_WHITESPACE",
                34,
                38,
                vec!["oui\u{202f}?".to_string()],
            ),
        ];
        assert_eq!(found, expected);
    }

    #[test]
    fn ids() {
        assert_eq!(
            to_id("FR_COMPOUNDS_arc en ciel"),
            "FR_COMPOUNDS_ARC_EN_CIEL"
        );
        assert_eq!(
            to_id("FR_SIMPLE_REPLACE_SIMPLE_y'en"),
            "FR_SIMPLE_REPLACE_SIMPLE_Y_Q_EN"
        );
        assert_eq!(
            to_id("FR_COMPOUNDS_île de france"),
            "FR_COMPOUNDS_ÎLE_DE_FRANCE"
        );
        assert_eq!(to_id(" a1œ "), "A__");
    }

    #[test]
    fn java_string_helpers() {
        assert_eq!(java_split("a b", ' '), vec!["a", "b"]);
        assert_eq!(java_split("a  b ", ' '), vec!["a", "", "b"]);
        assert_eq!(java_split("", ' '), vec![""]);
        assert!(java_split(" ", ' ').is_empty());
        assert_eq!(java_trim("\t a \n"), "a");
        assert_eq!(java_trim("\u{a0}a"), "\u{a0}a");
        assert!(
            is_java_digit('7') && is_java_digit('٣') && !is_java_digit('a') && !is_java_digit('²')
        );
        assert_eq!(prev_char_start("aé.", 3), 1);
    }
}
