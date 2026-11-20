//! The French disambiguator: multiword chunkers, then the XML rules.
//!
//! Port of `org.languagetool.tagging.disambiguation.fr.FrenchHybridDisambiguator`
//! and `XmlRuleDisambiguator` (LanguageTool, LGPL-2.1-or-later).

use std::path::Path;
use std::sync::Arc;

use super::chunker::{ChunkerSettings, MultiWordChunker, TAG_FOR_NOT_ADDING_TAGS};
use super::disambig::DisambigRule;
use super::rule::RuleContext;
use super::token::Sentence;
use super::unifier::UnifierConfiguration;
use super::xmlrules::{load_disambiguation, FilterFactory};

pub struct Disambiguator {
    chunker_global: MultiWordChunker,
    chunker: MultiWordChunker,
    rules: Vec<Arc<DisambigRule>>,
}

impl Disambiguator {
    /// Load `multiwords.txt`, `spelling_global.txt`, `disambiguation.xml` and
    /// `disambiguation-global.xml` from a LanguageTool resource directory.
    pub fn load(dir: &Path, factory: FilterFactory, unifier: &mut UnifierConfiguration) -> std::io::Result<Disambiguator> {
        let read = |name: &str| std::fs::read_to_string(dir.join(name));
        let chunker = MultiWordChunker::new(
            &read("multiwords.txt")?,
            &ChunkerSettings { allow_first_capitalized: true, allow_all_uppercase: true, default_tag: None, ignore_spelling: false, remove_previous_tags: true },
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
        Ok(Disambiguator { chunker_global, chunker, rules })
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// `FrenchHybridDisambiguator.disambiguate`: from the tagger's sentence to
    /// the disambiguated one. `raw.pre_disambig` is kept as given.
    pub fn disambiguate(&self, raw: Sentence, ctx: &RuleContext) -> Sentence {
        let Sentence { text, mut tokens, pre_disambig, .. } = raw;
        self.chunker_global.disambiguate(&mut tokens);
        self.chunker.disambiguate(&mut tokens);
        // The tagger's tokens come back as the pre-disambiguation state at
        // the end (`JLanguageTool.getAnalyzedSentence`).
        let mut sentence = Sentence::new(text, tokens, Vec::new());
        for rule in &self.rules {
            if let Some(next) = rule.replace_with(&sentence, ctx, false) {
                if std::env::var_os("GRAMMARIA_TRACE").is_some() {
                    eprintln!("disambig {}", rule.id);
                }
                sentence = next;
            }
        }
        let Sentence { text, tokens, .. } = sentence;
        Sentence::new(text, tokens, pre_disambig)
    }
}
