//! Inflection: from a lemma and a part-of-speech tag to word forms.
//!
//! Port of LanguageTool's `BaseSynthesizer`, `ManualSynthesizer` and
//! `FrenchSynthesizer`: the forms come from the synthesis dictionary
//! (`french_synth.dict`, keyed by `lemma|tag`), plus those of `added.txt`,
//! minus those of `removed.txt`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use super::pattern::Synthesizer;
use super::soros::Soros;
use super::token::Reading;
use crate::morfologik::{Dictionary, WordData};
use crate::re;

/// `ManualSynthesizer`: `form<TAB>lemma<TAB>tag` lines.
struct Manual {
    forms: HashMap<(String, String), Vec<String>>,
    tags: Vec<String>,
}

impl Manual {
    fn parse(content: &str) -> Manual {
        let mut forms: HashMap<(String, String), Vec<String>> = HashMap::new();
        let mut tags: Vec<String> = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = line.split('#').next().unwrap_or("").trim();
            let mut parts: Vec<&str> = line.split('\t').collect();
            while parts.last().is_some_and(|p| p.is_empty()) {
                parts.pop();
            }
            if parts.len() != 3 {
                continue;
            }
            if !tags.iter().any(|t| t == parts[2]) {
                tags.push(parts[2].to_string());
            }
            forms
                .entry((parts[1].to_string(), parts[2].to_string()))
                .or_default()
                .push(parts[0].to_string());
        }
        Manual { forms, tags }
    }

    fn lookup(&self, lemma: &str, tag: &str) -> Option<&Vec<String>> {
        self.forms.get(&(lemma.to_string(), tag.to_string()))
    }
}

pub struct FrenchSynthesizer {
    dict: Dictionary,
    manual: Option<Manual>,
    removal: Option<Manual>,
    /// `possibleTags`: the tag file, then the tags only `added.txt` knows.
    tags: Vec<String>,
    /// Tags accepted by a tag regular expression.
    by_regex: Mutex<HashMap<String, Arc<Vec<usize>>>>,
    /// Number spellers (`fr.sor`, `Roman.sor`), when the files are there.
    number_speller: Option<Soros>,
    roman_numberer: Option<Soros>,
}

const EXCEPTIONS_E_GRAVE: [&str; 3] = ["burkinabè", "koinè", "épistémè"];

impl FrenchSynthesizer {
    /// Loads `french_synth.dict`, `french_tags.txt`, `added.txt` and
    /// `removed.txt` from LanguageTool's French resource directory.
    pub fn load(dir: &Path) -> std::io::Result<FrenchSynthesizer> {
        let dict = Dictionary::load(dir.join("french_synth.dict"), dir.join("french_synth.info"))?;
        let manual = std::fs::read_to_string(dir.join("added.txt"))
            .ok()
            .map(|c| Manual::parse(&c));
        let removal = std::fs::read_to_string(dir.join("removed.txt"))
            .ok()
            .map(|c| Manual::parse(&c));
        let mut tags: Vec<String> = std::fs::read_to_string(dir.join("french_tags.txt"))?
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_string)
            .collect();
        if let Some(m) = &manual {
            for t in &m.tags {
                if !tags.contains(t) {
                    tags.push(t.clone());
                }
            }
        }
        let number_speller = std::fs::read_to_string(dir.join("fr.sor"))
            .ok()
            .map(|source| Soros::new(&source, "fr"));
        let roman_numberer = std::fs::read_to_string(dir.join("Roman.sor"))
            .ok()
            .map(|source| Soros::new(&source, "Roman"));
        Ok(FrenchSynthesizer {
            dict,
            manual,
            removal,
            tags,
            by_regex: Mutex::new(HashMap::new()),
            number_speller,
            roman_numberer,
        })
    }

    /// `BaseSynthesizer.lookup`.
    fn lookup(&self, lemma: &str, tag: &str, results: &mut Vec<String>) {
        let start = results.len();
        let mut found: Vec<WordData> = Vec::new();
        self.dict.lookup(&format!("{lemma}|{tag}"), &mut found);
        results.extend(found.into_iter().map(|w| w.stem));
        if let Some(forms) = self.manual.as_ref().and_then(|m| m.lookup(lemma, tag)) {
            results.extend(forms.iter().cloned());
        }
        if let Some(removed) = self.removal.as_ref().and_then(|m| m.lookup(lemma, tag)) {
            // `List.removeAll` on the whole result list.
            let _ = start;
            results.retain(|r| !removed.contains(r));
        }
    }

    /// `FrenchSynthesizer.isException`.
    fn is_exception(w: &str) -> bool {
        if w.starts_with("qq") {
            return true;
        }
        w.ends_with('è') && !EXCEPTIONS_E_GRAVE.contains(&w.to_lowercase().as_str())
    }

    /// Forms of `lemma` for every known tag accepted by the regular
    /// expression `tag_regex` (`synthesizeForPosTags`).
    pub fn synthesize_for_tag_regex(&self, lemma: &str, tag_regex: &str) -> Vec<String> {
        let accepted = {
            let mut cache = self.by_regex.lock().unwrap();
            match cache.get(tag_regex) {
                Some(a) => a.clone(),
                None => {
                    let list: Vec<usize> = match re::Regex::new(tag_regex, re::Flags::default()) {
                        Ok(regex) => (0..self.tags.len())
                            .filter(|&i| regex.is_full_match(&self.tags[i]))
                            .collect(),
                        Err(_) => Vec::new(),
                    };
                    let list = Arc::new(list);
                    cache.insert(tag_regex.to_string(), list.clone());
                    list
                }
            }
        };
        let mut results: Vec<String> = Vec::new();
        for &i in accepted.iter() {
            self.lookup(lemma, &self.tags[i], &mut results);
        }
        results.retain(|w| !Self::is_exception(w));
        results
    }

    pub fn possible_tags(&self) -> &[String] {
        &self.tags
    }
}

impl Synthesizer for FrenchSynthesizer {
    fn synthesize(&self, token: &Reading, pos_tag: &str) -> Vec<String> {
        // `getSpelledNumber` / `getRomanNumber`: without the program, the
        // numeral is given back.
        let spell = |speller: &Option<Soros>, input: &str| match speller {
            Some(s) => s.run(input),
            None => token.token.to_string(),
        };
        match pos_tag {
            "_spell_number_" => return vec![spell(&self.number_speller, &token.token)],
            "_spell_number_:feminine" => {
                return vec![spell(
                    &self.number_speller,
                    &format!("feminine {}", token.token),
                )]
            }
            "_spell_number_:Roman" => return vec![spell(&self.roman_numberer, &token.token)],
            _ => {}
        }
        let Some(lemma) = token.lemma.as_deref() else {
            return Vec::new();
        };
        let mut results: Vec<String> = Vec::new();
        self.lookup(lemma, pos_tag, &mut results);
        results.retain(|w| !Self::is_exception(w));
        results
    }

    fn synthesize_regexp(
        &self,
        token: &Reading,
        pos_tag: &str,
        pos_tag_regexp: bool,
    ) -> Vec<String> {
        if pos_tag_regexp {
            let Some(lemma) = token.lemma.as_deref() else {
                return Vec::new();
            };
            return self.synthesize_for_tag_regex(lemma, pos_tag);
        }
        self.synthesize(token, pos_tag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inflects_a_verb_and_a_noun() {
        let dir = Path::new("assets/fr/lt");
        if !dir.join("french_synth.dict").exists() {
            return;
        }
        let synth = FrenchSynthesizer::load(dir).unwrap();
        let chanter = Reading::from_strs("chante", Some("V ind pres 3 s"), Some("chanter"));
        assert_eq!(synth.synthesize(&chanter, "V ind pres 1 p"), ["chantons"]);
        let cheval = Reading::from_strs("cheval", Some("N m s"), Some("cheval"));
        assert_eq!(
            synth.synthesize_regexp(&cheval, "N m p", false),
            ["chevaux"]
        );
        let seven = Reading::from_strs("7", None, None);
        assert_eq!(synth.synthesize(&seven, "_spell_number_"), ["sept"]);
        let forms = synth.synthesize_regexp(&cheval, "N m .", true);
        assert!(
            forms.contains(&"cheval".to_string()) && forms.contains(&"chevaux".to_string()),
            "{forms:?}"
        );
    }
}
