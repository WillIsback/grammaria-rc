//! Port of `org.languagetool.rules.AbstractRepeatedWordsRule` (with
//! `SynonymsData`) and of the French
//! `org.languagetool.rules.fr.FrenchRepeatedWordsRule`: a word of
//! `synonyms.txt` used again within 150 words, with its synonyms as
//! suggestions.
//!
//! LanguageTool also gives each match a specific id
//! (`FR_REPEATEDWORDS_FACILE`, `RuleMatch.setSpecificRuleId`); the match
//! keeps the rule's own id here, as `getRule().getId()` does.

use std::collections::HashMap;
use std::sync::Arc;

use super::super::checker::NativeRule;
use super::super::rule::{RuleContext, RuleInfo, RuleMatch};
use super::super::strtools;
use super::super::token::{Reading, Sentence};
use super::{java_split, java_trim, rule_info, rule_match, REPETITIONS_STYLE};
use crate::re::{self, Flags, Regex};

const MESSAGE: &str = "Ce mot apparaît déjà dans l'une des phrases précédant immédiatement celle-ci. Utilisez un synonyme pour apporter plus de variété à votre texte, excepté si la répétition est intentionnelle.";
const SHORT_MESSAGE: &str = "Style : Mot répété";
/// `maxWordsDistance`.
const MAX_WORDS_DISTANCE: usize = 150;

/// `SynonymsData`: the synonyms of a lemma, and what its readings must look
/// like for the rule to apply.
struct SynonymsData {
    synonyms: Vec<String>,
    postag: Option<Arc<Regex>>,
    chunk: Option<Arc<Regex>>,
}

pub struct FrenchRepeatedWordsRule {
    info: RuleInfo,
    words_to_check: HashMap<String, SynonymsData>,
    punctuation: Arc<Regex>,
}

/// `loadWords`: lines `lemma/postag/chunk=synonym;synonym`, or
/// `word;word;word` for words that are synonyms of one another. `#` starts
/// a comment.
fn load_words(text: &str) -> Result<HashMap<String, SynonymsData>, String> {
    let pattern = |src: &str| re::compile(src, Flags::default()).map_err(|e| format!("invalid pattern {src}: {}", e.0));
    let mut map: HashMap<String, SynonymsData> = HashMap::new();
    for raw in text.lines() {
        let line = java_trim(raw.split('#').next().unwrap_or(""));
        if line.is_empty() {
            continue;
        }
        let format_error = || format!("Format error in line: {line}");
        let main_parts = java_split(line, '=');
        match main_parts.len() {
            2 => {
                let parts = java_split(main_parts[1], ';');
                let word_pos_chunk = java_split(main_parts[0], '/');
                let word = word_pos_chunk.first().copied().unwrap_or("");
                if parts.len() < if word.is_empty() { 2 } else { 1 } {
                    return Err(format_error());
                }
                if word.is_empty() {
                    add_mutual_synonyms(&mut map, &parts, line)?;
                    continue;
                }
                if map.contains_key(word) {
                    return Err(format!("Word found in more than one line. \"{word}\" in line: {line}"));
                }
                let data = SynonymsData {
                    synonyms: parts.iter().map(|s| s.to_string()).collect(),
                    postag: word_pos_chunk.get(1).map(|p| pattern(p)).transpose()?,
                    chunk: word_pos_chunk.get(2).map(|p| pattern(p)).transpose()?,
                };
                map.insert(word.to_string(), data);
            }
            1 => {
                let parts = java_split(line, ';');
                if parts.len() < 2 {
                    return Err(format_error());
                }
                add_mutual_synonyms(&mut map, &parts, line)?;
            }
            _ => return Err(format_error()),
        }
    }
    Ok(map)
}

/// Each of `words` gets the others as synonyms.
fn add_mutual_synonyms(map: &mut HashMap<String, SynonymsData>, words: &[&str], line: &str) -> Result<(), String> {
    for key in words {
        if map.contains_key(*key) {
            return Err(format!("Word found in more than one line. \"{key}\" in line: {line}"));
        }
        let synonyms = words.iter().filter(|w| w != &key).map(|w| w.to_string()).collect();
        map.insert(key.to_string(), SynonymsData { synonyms, postag: None, chunk: None });
    }
    Ok(())
}

/// `adjustPostag` of the French rule: the tag to synthesize a synonym with,
/// as a regular expression that leaves gender and number open where the
/// synonym may not vary.
fn adjust_postag(postag: &str) -> String {
    const REWRITES: [(&str, &str); 9] = [
        ("e sp", ". .*"),
        ("m s", "[me] sp?"),
        ("f s", "[fe] sp?"),
        ("m p", "[me] s?p"),
        ("f p", "[fe] s?p"),
        ("e s", "[me] sp?"),
        ("e p", "[me] s?p"),
        ("m sp", "[me] s?p?"),
        ("f sp", "[fe] s?p?"),
    ];
    for (ending, replacement) in REWRITES {
        if postag.ends_with(ending) {
            // `replaceOnce`: the first occurrence, which need not be the end.
            return postag.replacen(ending, replacement, 1);
        }
    }
    postag.to_string()
}

impl FrenchRepeatedWordsRule {
    /// The rule over the text of `synonyms.txt`.
    pub fn new(synonyms: &str) -> Result<FrenchRepeatedWordsRule, String> {
        Ok(FrenchRepeatedWordsRule {
            info: rule_info("FR_REPEATEDWORDS", "Synonymes de mots répétés.", &REPETITIONS_STYLE, "style"),
            words_to_check: load_words(synonyms)?,
            punctuation: re::compile("\\p{P}", Flags::default()).expect("valid pattern"),
        })
    }
}

impl NativeRule for FrenchRepeatedWordsRule {
    fn info(&self) -> &RuleInfo {
        &self.info
    }

    fn is_text_level(&self) -> bool {
        true
    }

    fn match_text(&self, rule: usize, sentences: &[Sentence], ctx: &RuleContext) -> Vec<RuleMatch> {
        let mut matches = Vec::new();
        let mut word_number = 0usize;
        // Lemma -> number of the last word it was seen at.
        let mut words_last_seen: HashMap<&str, usize> = HashMap::new();
        let mut pos = 0;
        let mut prev_sentence_length = 0;
        for sentence in sentences {
            pos += prev_sentence_length;
            prev_sentence_length = sentence.text.len();
            let n = sentence.len();
            // Only complete sentences are looked at.
            if n == 0 || !matches!(&*sentence.tok(n - 1).token, "." | "!" | "?") {
                continue;
            }
            let mut sent_start = true;
            let mut lemmas_in_sentence: Vec<&str> = Vec::new();
            // `i` counts the tokens that are not immunized; as in Java it is
            // then used as an index among all the tokens.
            for (i, atrs) in (0..n).map(|k| sentence.tok(k)).filter(|t| !t.immunized).enumerate() {
                let token: &str = &atrs.token;
                if !token.is_empty() {
                    word_number += 1;
                }
                let is_capitalized = strtools::is_capitalized_word(token);
                let is_all_uppercase = strtools::is_all_uppercase(token);
                let is_exception =
                    token.is_empty() || is_all_uppercase || (is_capitalized && !sent_start) || sentence.tok(i).has_pos_tag_starting_with("Z");
                if sent_start && !token.is_empty() && !self.punctuation.is_full_match(token) {
                    sent_start = false;
                }
                if is_exception {
                    continue;
                }
                let mut lemmas: Vec<&str> = Vec::new();
                for atr in &atrs.readings {
                    let Some(lemma) = atr.lemma.as_deref() else { continue };
                    lemmas.push(lemma);
                    let Some(&seen_at) = words_last_seen.get(lemma) else { continue };
                    if lemmas_in_sentence.contains(&lemma) || word_number - seen_at > MAX_WORDS_DISTANCE {
                        continue;
                    }
                    let Some(data) = self.words_to_check.get(lemma) else { continue };
                    let postag = atr.pos();
                    if data.postag.as_ref().is_some_and(|re| !postag.is_some_and(|p| re.is_full_match(p))) {
                        continue;
                    }
                    if data.chunk.as_ref().is_some_and(|re| !atrs.matches_chunk_regex(re)) {
                        continue;
                    }
                    let mut suggestions: Vec<String> = Vec::new();
                    for replacement_lemma in &data.synonyms {
                        let mut replacements: Vec<String> = match (&ctx.lang.synthesizer, postag) {
                            (Some(synthesizer), Some(postag)) => {
                                let reading = Reading::from_strs(token, Some(postag), Some(replacement_lemma));
                                synthesizer.synthesize_regexp(&reading, &adjust_postag(postag), true)
                            }
                            _ => Vec::new(),
                        };
                        if replacements.is_empty() {
                            replacements.push(replacement_lemma.clone());
                        }
                        for r in replacements {
                            suggestions.push(if is_all_uppercase {
                                r.to_uppercase()
                            } else if is_capitalized {
                                strtools::uppercase_first_char(&r)
                            } else {
                                r
                            });
                        }
                    }
                    matches.extend(rule_match(rule, pos + atrs.start_pos, pos + atrs.end_pos(), MESSAGE, SHORT_MESSAGE, suggestions));
                    break;
                }
                for lemma in lemmas {
                    if let Some((key, _)) = self.words_to_check.get_key_value(lemma) {
                        words_last_seen.insert(key, word_number);
                        lemmas_in_sentence.push(key);
                    }
                }
            }
        }
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{context, sentence};
    use super::*;

    const DATA: &str = "#important/J.*=crucial;grave\nfacile/J.*=simple;commode;aisé;\nnotamment/A=surtout;spécialement # note\nvite;rapidement;promptement\n";

    fn check(sentences: &[Sentence]) -> Vec<(usize, usize, Vec<String>)> {
        let rule = FrenchRepeatedWordsRule::new(DATA).unwrap();
        rule.match_text(0, sentences, &context()).into_iter().map(|m| (m.from_pos, m.to_pos, m.suggestions)).collect()
    }

    fn facile() -> [&'static str; 8] {
        ["C'est/ce/R dem m s", " ", "facile/facile/J e s", " ", "à/à/P", " ", "faire/faire/V inf", "."]
    }

    #[test]
    fn synonyms_file() {
        let words = load_words(DATA).unwrap();
        assert_eq!(words.len(), 5);
        assert_eq!(words["facile"].synonyms, vec!["simple", "commode", "aisé"]);
        assert!(words["facile"].postag.as_ref().unwrap().is_full_match("J e s"));
        assert_eq!(words["vite"].synonyms, vec!["rapidement", "promptement"]);
        assert!(words["vite"].postag.is_none());
        assert!(load_words("a=b\na=c\n").is_err());
        assert!(load_words("seul\n").is_err());
    }

    #[test]
    fn repeated_word_in_a_later_sentence() {
        let second = sentence(&["Facile/facile/J e s", " ", "à/à/P", " ", "dire/dire/V inf", "."]);
        assert_eq!(
            check(&[sentence(&facile()), second]),
            vec![(22, 28, vec!["Simple".to_string(), "Commode".to_string(), "Aisé".to_string()])]
        );
        // Twice in one sentence is not reported, nor is another part of speech.
        let twice = sentence(&["facile/facile/J e s", " ", "et", " ", "facile/facile/J e s", "."]);
        assert!(check(&[twice]).is_empty());
        let noun = sentence(&["Le", " ", "facile/facile/N m s", " ", "plaît", "."]);
        assert!(check(&[sentence(&facile()), noun]).is_empty());
    }

    #[test]
    fn exceptions() {
        // An unfinished sentence, a capitalised word inside a sentence.
        let unfinished = sentence(&["Trop", " ", "facile/facile/J e s"]);
        assert!(check(&[sentence(&facile()), unfinished]).is_empty());
        let capitalised = sentence(&["Trop", " ", "Facile/facile/J e s", "."]);
        assert!(check(&[sentence(&facile()), capitalised]).is_empty());
    }

    #[test]
    fn tags_for_synthesis() {
        assert_eq!(adjust_postag("J e sp"), "J . .*");
        assert_eq!(adjust_postag("J m s"), "J [me] sp?");
        assert_eq!(adjust_postag("N f p"), "N [fe] s?p");
        assert_eq!(adjust_postag("J e s"), "J [me] sp?");
        assert_eq!(adjust_postag("A"), "A");
    }
}
