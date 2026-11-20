//! Spell checking: LanguageTool's French spelling rule and the rule filters
//! that depend on it.
//!
//! * [`speller_rule`] — `MorfologikFrenchSpellerRule` (`FR_SPELLING_RULE`)
//!   with `MorfologikSpellerRule` and `SpellingCheckRule`;
//! * [`morfologik_speller`] — `MorfologikSpeller`, `MorfologikMultiSpeller`,
//!   `WeightedSuggestion`;
//! * [`fsa_builder`] — the in-memory dictionary made of the plain-text word
//!   lists (Java's `FSABuilder`);
//! * [`wordlist`] — `CachingWordListLoader`, `French.prepareLineForSpeller`;
//! * [`multitoken`] — `MultitokenSpeller`, `FrenchMultitokenSpeller`,
//!   `MultitokenSpellerFilter`;
//! * [`filters`] — `FindSuggestionsFilter`,
//!   `FrenchSuppressMisspelledSuggestionsFilter`;
//! * [`urls`] — `WordTokenizer.isUrl` / `isEMail`;
//! * [`strings`] — the string helpers these classes use;
//! * [`stopgap`] — a provisional tagger and word tokenizer.
//!
//! [`FrenchSpelling`] loads everything once and hands out the pieces the
//! checker needs: the native rule, the language's [`SpellChecker`] and the
//! three rule filters.

pub mod filters;
pub mod fsa_builder;
pub mod morfologik_speller;
pub mod multitoken;
pub mod speller_rule;
pub mod stopgap;
pub mod strings;
pub mod urls;
pub mod wordlist;

use std::path::Path;
use std::sync::Arc;

use super::checker::NativeRule;
use super::pattern::{SpellChecker, WordTagger};
use super::rule::RuleFilter;
use crate::morfologik::Dictionary;

pub use filters::{FindSuggestionsFilter, SuppressMisspelledSuggestionsFilter};
pub use multitoken::{MultitokenEntries, MultitokenSpeller, MultitokenSpellerFilter};
pub use speller_rule::{FrenchSpellerNativeRule, FrenchSpellerRule};

/// The language's word tokenizer (`Language.getWordTokenizer().tokenize`):
/// every character of the text ends up in one token, whitespace included.
pub type Tokenize = Arc<dyn Fn(&str) -> Vec<String> + Send + Sync>;

/// The French spell checker and what depends on it.
pub struct FrenchSpelling {
    /// `French.getDefaultSpellingRule()`.
    pub rule: Arc<FrenchSpellerRule>,
    /// `French.getTagger()`, as given at construction.
    pub tagger: Arc<dyn WordTagger>,
    find_suggestions: Arc<dyn RuleFilter>,
    suppress_misspelled: Arc<dyn RuleFilter>,
    multitoken: Arc<dyn RuleFilter>,
}

impl FrenchSpelling {
    /// Loads the speller from LanguageTool's French resource directory.
    /// `dictionary` is `french.dict` (the dictionary the French speller rule
    /// names, which is also the tagger's), `tagger` the French tagger and
    /// `tokenize` the French word tokenizer.
    pub fn load(assets: &Path, dictionary: Arc<Dictionary>, tagger: Arc<dyn WordTagger>, tokenize: Tokenize) -> std::io::Result<FrenchSpelling> {
        // The multi-word entries are indexed while the rule loads.
        let (rule, entries) = std::thread::scope(|scope| {
            let entries = scope.spawn(|| MultitokenEntries::french(assets));
            let rule = FrenchSpellerRule::load(assets, dictionary, tagger.clone(), tokenize.clone());
            (rule, entries.join().expect("multi-word thread panicked"))
        });
        let rule = Arc::new(rule?);
        let multitoken_speller = Arc::new(MultitokenSpeller::new(rule.clone(), entries));
        Ok(FrenchSpelling {
            find_suggestions: Arc::new(FindSuggestionsFilter::new(rule.clone(), tagger.clone())),
            suppress_misspelled: Arc::new(SuppressMisspelledSuggestionsFilter::new(rule.clone(), tagger.clone(), tokenize)),
            multitoken: Arc::new(MultitokenSpellerFilter::new(multitoken_speller)),
            rule,
            tagger,
        })
    }

    /// [`FrenchSpelling::load`] with the provisional tagger and tokenizer of
    /// [`stopgap`], until the engine's own are available.
    pub fn load_with_stopgaps(assets: &Path) -> std::io::Result<FrenchSpelling> {
        let dictionary = Arc::new(Dictionary::load(assets.join("french.dict"), assets.join("french.info"))?);
        let tagger = Arc::new(stopgap::StopgapTagger::new(dictionary.clone(), assets));
        let tokenizer = stopgap::StopgapTokenizer::new(tagger.clone());
        let tokenize: Tokenize = Arc::new(move |text: &str| tokenizer.tokenize(text));
        FrenchSpelling::load(assets, dictionary, tagger, tokenize)
    }

    /// The spelling rule for the checker's rule list (it comes fourth among
    /// LanguageTool's French rules, before the rule files).
    pub fn native_rule(&self) -> Box<dyn NativeRule> {
        Box::new(FrenchSpellerNativeRule(self.rule.clone()))
    }

    /// The spelling rule as the language's spell checker (`Lang.speller`).
    pub fn spell_checker(&self) -> Arc<dyn SpellChecker> {
        self.rule.clone()
    }

    /// The filter of a rule, by the simple name of its Java class; `None`
    /// for the classes that are not about spelling.
    pub fn filter(&self, class: &str) -> Option<Arc<dyn RuleFilter>> {
        match class {
            "FindSuggestionsFilter" => Some(self.find_suggestions.clone()),
            "FrenchSuppressMisspelledSuggestionsFilter" => Some(self.suppress_misspelled.clone()),
            "MultitokenSpellerFilter" => Some(self.multitoken.clone()),
            _ => None,
        }
    }
}

/// What the tests of this module share: the speller loaded from the
/// repository's resources, and sentences analysed with the provisional
/// tokenizer and tagger. The expected values of those tests were produced by
/// LanguageTool (Java) on the same inputs.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Arc, OnceLock};

    use super::super::pattern::WordTagger;
    use super::super::token::{Reading, Readings, Sentence, SENTENCE_START};
    use super::stopgap::{StopgapTagger, StopgapTokenizer};
    use super::FrenchSpelling;
    use crate::morfologik::Dictionary;

    pub struct Tools {
        pub spelling: FrenchSpelling,
        tagger: Arc<StopgapTagger>,
        tokenizer: StopgapTokenizer,
    }

    /// `None` when the resources are not there (tests are then skipped).
    pub fn tools() -> Option<&'static Tools> {
        static TOOLS: OnceLock<Option<Tools>> = OnceLock::new();
        TOOLS
            .get_or_init(|| {
                let assets = std::path::Path::new("assets/fr/lt");
                let dictionary = Arc::new(Dictionary::load(assets.join("french.dict"), assets.join("french.info")).ok()?);
                let tagger = Arc::new(StopgapTagger::new(dictionary, assets));
                Some(Tools {
                    spelling: FrenchSpelling::load_with_stopgaps(assets).ok()?,
                    tokenizer: StopgapTokenizer::new(tagger.clone()),
                    tagger,
                })
            })
            .as_ref()
    }

    impl Tools {
        /// One sentence: `SENT_START`, then the tokens of `text` with the
        /// readings the tagger gives them.
        pub fn analyse(&self, text: &str) -> Sentence {
            let mut tokens = vec![Readings::new(vec![Reading::from_strs("", Some(SENTENCE_START), None)], 0)];
            let mut pos = 0;
            let mut previous: Arc<str> = Arc::from("");
            for token in self.tokenizer.tokenize(text) {
                let tagged = self.tagger.tag_word(&token);
                let text_of: Arc<str> = Arc::from(token.as_str());
                let readings = tagged
                    .readings
                    .iter()
                    .map(|r| Reading::new(text_of.clone(), r.pos_tag, r.lemma.clone()))
                    .collect();
                let mut readings = Readings::new(readings, pos);
                readings.set_whitespace_before(&previous);
                pos += token.len();
                previous = text_of;
                tokens.push(readings);
            }
            Sentence::new(Arc::from(text), tokens, Vec::new())
        }
    }
}
