//! The French disambiguator: multiword chunkers, then the XML rules.
//!
//! Port of `org.languagetool.tagging.disambiguation.fr.FrenchHybridDisambiguator`
//! and `XmlRuleDisambiguator` (LanguageTool, LGPL-2.1-or-later).

use std::path::Path;
use std::sync::Arc;

use super::chunker::{ChunkerSettings, MultiWordChunker, TAG_FOR_NOT_ADDING_TAGS};
use super::disambig::DisambigRule;
use super::fx::FxMap;
use super::rule::RuleContext;
use super::token::Sentence;
use super::unifier::UnifierConfiguration;
use super::xmlrules::{load_disambiguation, FilterFactory};

pub struct Disambiguator {
    chunker_global: MultiWordChunker,
    chunker: MultiWordChunker,
    rules: Vec<Arc<DisambigRule>>,
    /// Rules without a required word (`by_token` holds the others, under
    /// each word of their most selective requirement).
    unhinted: Vec<u32>,
    by_token: FxMap<Box<str>, Vec<u32>>,
}

impl Disambiguator {
    /// Load `multiwords.txt`, `spelling_global.txt`, `disambiguation.xml` and
    /// `disambiguation-global.xml` from a LanguageTool resource directory.
    pub fn load(
        dir: &Path,
        factory: FilterFactory,
        unifier: &mut UnifierConfiguration,
    ) -> std::io::Result<Disambiguator> {
        let read = |name: &str| std::fs::read_to_string(dir.join(name));
        let chunker = MultiWordChunker::new(
            &read("multiwords.txt")?,
            &ChunkerSettings {
                allow_first_capitalized: true,
                allow_all_uppercase: true,
                default_tag: None,
                ignore_spelling: false,
                remove_previous_tags: true,
            },
        );
        let chunker_global = MultiWordChunker::new(
            &read("spelling_global.txt").unwrap_or_default(),
            &ChunkerSettings {
                allow_first_capitalized: false,
                allow_all_uppercase: true,
                default_tag: Some(TAG_FOR_NOT_ADDING_TAGS),
                ignore_spelling: true,
                remove_previous_tags: false,
            },
        );
        let mut rules = load_disambiguation(&read("disambiguation.xml")?, factory, unifier);
        if let Ok(global) = read("disambiguation-global.xml") {
            rules.extend(load_disambiguation(&global, factory, unifier));
        }
        let mut unhinted = Vec::new();
        let mut by_token: FxMap<Box<str>, Vec<u32>> = FxMap::default();
        for (i, rule) in rules.iter().enumerate() {
            // Lemmas change during disambiguation, words do not: only
            // requirements on the word itself are indexed.
            let hint = rule
                .rule
                .pattern
                .token_hints
                .iter()
                .filter(|h| !h.inflected)
                .min_by_key(|h| h.lower_case_values.len());
            match hint {
                Some(h) => {
                    for value in &h.lower_case_values {
                        by_token
                            .entry(Box::from(value.as_str()))
                            .or_default()
                            .push(i as u32);
                    }
                }
                None => unhinted.push(i as u32),
            }
        }
        Ok(Disambiguator {
            chunker_global,
            chunker,
            rules,
            unhinted,
            by_token,
        })
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// The rules that can apply to the sentence, in order: those without a
    /// required word, and those of which the sentence has a required word.
    fn candidates(&self, sentence: &Sentence) -> Vec<u32> {
        let mut wanted = vec![false; self.rules.len()];
        for &r in &self.unhinted {
            wanted[r as usize] = true;
        }
        for key in sentence.token_offsets.keys() {
            if let Some(list) = self.by_token.get(key.as_str()) {
                for &r in list {
                    wanted[r as usize] = true;
                }
            }
        }
        (0..self.rules.len() as u32)
            .filter(|&r| wanted[r as usize])
            .collect()
    }

    /// `FrenchHybridDisambiguator.disambiguate`: from the tagger's sentence to
    /// the disambiguated one. `raw.pre_disambig` is kept as given.
    pub fn disambiguate(&self, raw: Sentence, ctx: &RuleContext) -> Sentence {
        let Sentence {
            text,
            mut tokens,
            pre_disambig,
            ..
        } = raw;
        self.chunker_global.disambiguate(&mut tokens);
        self.chunker.disambiguate(&mut tokens);
        // The tagger's tokens come back as the pre-disambiguation state at
        // the end (`JLanguageTool.getAnalyzedSentence`).
        let mut sentence = Sentence::new(text, tokens, Vec::new());
        let trace = std::env::var_os("GRAMMARIA_TRACE").is_some();
        // Rules that require a word the sentence lacks are skipped without
        // being looked at; the list is rebuilt if a rule changes the words.
        let mut candidates = self.candidates(&sentence);
        let mut next_candidate = 0;
        while next_candidate < candidates.len() {
            let index = candidates[next_candidate] as usize;
            next_candidate += 1;
            let rule = &self.rules[index];
            if let Some(next) = rule.replace_with(&sentence, ctx, false) {
                if trace {
                    eprintln!("disambig {}", rule.id);
                }
                let same_words = Arc::ptr_eq(&next.token_offsets, &sentence.token_offsets);
                sentence = next;
                if !same_words {
                    candidates = self.candidates(&sentence);
                    next_candidate = candidates.partition_point(|&c| (c as usize) <= index);
                }
            }
        }
        let Sentence { text, tokens, .. } = sentence;
        Sentence::new(text, tokens, pre_disambig)
    }
}
