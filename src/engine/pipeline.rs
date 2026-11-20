//! The whole French checker: analysis, disambiguation and rules, loaded from
//! LanguageTool's resource directory (what `JLanguageTool` does for French).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::analysis::Analyzer;
use super::checker::{CheckRule, Checker, Found, NativeRule};
use super::disambiguator::Disambiguator;
use super::pattern::{Lang, SpellChecker, WordTagger};
use super::rule::{RuleContext, RuleFilter};
use super::synth::FrenchSynthesizer;
use super::tagger::{FrenchTagger, WordDictionary};
use super::token::{Readings, Sentence};
use super::unifier::UnifierConfiguration;
use super::xmlrules::{load_grammar, GrammarRule};
use crate::morfologik::{Dictionary, WordData};

/// LanguageTool's `grammar.xml` and `style.xml`, then this project's own rule
/// files: every `*.xml` of the sibling directory `grammaria`, in name order.
pub fn default_rule_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = vec![dir.join("grammar.xml"), dir.join("style.xml")];
    if let Some(own) = dir.parent().map(|p| p.join("grammaria")) {
        if let Ok(entries) = std::fs::read_dir(&own) {
            let mut extra: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "xml")).collect();
            extra.sort();
            files.extend(extra);
        }
    }
    files
}

/// The tagger dictionary (`french.dict`) read with Morfologik.
pub struct MorfologikDictionary(pub Dictionary);

impl WordDictionary for MorfologikDictionary {
    fn lookup(&self, word: &str, out: &mut Vec<(String, String)>) {
        let mut found: Vec<WordData> = Vec::new();
        self.0.lookup(word, &mut found);
        out.extend(found.into_iter().map(|w| (w.stem, w.tag)));
    }
}

impl WordTagger for FrenchTagger {
    fn tag_word(&self, word: &str) -> Readings {
        self.tag_token(word)
    }
}

/// What is plugged into the rule files: the Java-coded filters, the native
/// rules that run before the rule files, and the spell checker.
#[derive(Default)]
pub struct Extensions {
    pub filters: Vec<Box<dyn Fn(&str) -> Option<Arc<dyn RuleFilter>>>>,
    pub native_rules: Vec<Box<dyn NativeRule>>,
    pub speller: Option<Arc<dyn SpellChecker>>,
}

pub struct French {
    pub analyzer: Analyzer,
    pub disambiguator: Disambiguator,
    pub checker: Checker,
}

impl French {
    /// Loads everything from `dir` (LanguageTool's `resource/fr` and
    /// `rules/fr` files side by side). `extend` receives the tagger and the
    /// synthesizer and returns what is plugged into the rules.
    pub fn load_with(dir: &Path, extend: impl FnOnce(&Path, &Arc<FrenchTagger>, &Arc<FrenchSynthesizer>) -> Extensions) -> io::Result<French> {
        French::load_rule_files(dir, &default_rule_files(dir), extend)
    }

    /// Same, with the given grammar rule files instead of the default ones
    /// (to try a rule file on its own, for instance).
    pub fn load_rule_files(
        dir: &Path,
        rule_files: &[PathBuf],
        extend: impl FnOnce(&Path, &Arc<FrenchTagger>, &Arc<FrenchSynthesizer>) -> Extensions,
    ) -> io::Result<French> {
        let dictionary = Dictionary::load(dir.join("french.dict"), dir.join("french.info"))?;
        let tagger = Arc::new(FrenchTagger::new(Arc::new(MorfologikDictionary(dictionary)), dir)?);
        let srx = std::fs::read_to_string(dir.join("segment.srx"))?;
        let sentence_tokenizer = super::sentences::SrxSentenceTokenizer::new(&srx, "fr").map_err(io::Error::other)?;
        let analyzer = Analyzer::from_parts(tagger.clone(), sentence_tokenizer);
        let synthesizer = Arc::new(FrenchSynthesizer::load(dir)?);
        let extensions = extend(dir, &tagger, &synthesizer);
        let factory = |class: &str| extensions.filters.iter().find_map(|f| f(class));

        let mut disambig_unifier = UnifierConfiguration::default();
        let disambiguator = Disambiguator::load(dir, &factory, &mut disambig_unifier)?;
        let mut unifier = UnifierConfiguration::default();
        let mut xml_rules: Vec<CheckRule> = Vec::new();
        for path in rule_files {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("rules.xml");
            let style = name.contains("style");
            let xml = std::fs::read_to_string(path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
            xml_rules.extend(load_grammar(&xml, name, style, &factory, &mut unifier).into_iter().map(|r| match r {
                GrammarRule::Pattern(p) => CheckRule::Pattern(p),
                GrammarRule::Regex(r) => CheckRule::Regex(r),
            }));
        }
        let Extensions { native_rules, speller, .. } = extensions;
        let mut rules: Vec<CheckRule> = native_rules.into_iter().map(CheckRule::Native).collect();
        rules.extend(xml_rules);
        let lang = Lang { synthesizer: Some(synthesizer), tagger: Some(tagger), speller };
        let checker = Checker::new(rules, RuleContext { lang, unifier, disambig_unifier });
        Ok(French { analyzer, disambiguator, checker })
    }

    /// The rule files alone, without native rules, filters or spelling.
    pub fn load(dir: &Path) -> io::Result<French> {
        French::load_with(dir, |_, _, _| Extensions::default())
    }

    /// `JLanguageTool.analyzeText`: the sentences of `text`, tagged and
    /// disambiguated.
    pub fn analyze(&self, text: &str) -> Vec<Sentence> {
        self.analyzer.analyze_with(text, |raw| {
            let pre = raw.tokens.clone();
            let Sentence { text, tokens, .. } = raw;
            self.disambiguator.disambiguate(Sentence::new(text, tokens, pre), &self.checker.ctx)
        })
    }

    /// `JLanguageTool.check` at the default level.
    pub fn check(&self, text: &str) -> Vec<Found> {
        self.checker.check(&self.analyze(text))
    }
}

/// The French checker with everything this crate implements plugged in.
pub fn load_french(dir: &Path) -> io::Result<French> {
    let mut failure: Option<io::Error> = None;
    let french = French::load_with(dir, |dir, _tagger, _synthesizer| {
        let mut extensions = Extensions::default();
        match super::native::french_native_rules(dir, None) {
            Ok(rules) => extensions.native_rules = rules,
            Err(e) => failure = Some(e),
        }
        extensions
    })?;
    match failure {
        Some(e) => Err(e),
        None => Ok(french),
    }
}

/// The same checker with the given rule files only.
pub fn load_french_with_rules(dir: &Path, rule_files: &[PathBuf]) -> io::Result<French> {
    French::load_rule_files(dir, rule_files, |_, _, _| Extensions::default())
}
