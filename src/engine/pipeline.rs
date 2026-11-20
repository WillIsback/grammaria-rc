//! The whole French checker: analysis, disambiguation and rules, loaded from
//! LanguageTool's resource directory (what `JLanguageTool` does for French).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::analysis::Analyzer;
use super::checker::{CheckRule, Checker, Found, NativeRule};
use super::disambiguator::Disambiguator;
use super::filters::{FrenchFilters, SentenceAnalyzer};
use super::pattern::{Lang, SpellChecker, WordTagger};
use super::rule::{RuleContext, RuleFilter};
use super::spelling::{FrenchSpelling, Tokenize};
use super::synth::FrenchSynthesizer;
use super::tagger::{FrenchTagger, WordDictionary};
use super::token::{Readings, Sentence};
use super::unifier::UnifierConfiguration;
use super::fastload::load_grammar_parallel;
use super::xmlrules::GrammarRule;
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
pub struct MorfologikDictionary(pub Arc<Dictionary>);

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

/// What the extensions are built from.
pub struct Loaded<'a> {
    pub dir: &'a Path,
    pub dictionary: &'a Arc<Dictionary>,
    pub tagger: &'a Arc<FrenchTagger>,
    pub synthesizer: &'a Arc<FrenchSynthesizer>,
    pub analyzer: &'a Arc<Analyzer>,
}

/// What is plugged into the rule files: the Java-coded filters, the native
/// rules that run before the rule files, and the spell checker.
#[derive(Default)]
pub struct Extensions {
    pub filters: Vec<Box<dyn Fn(&str) -> Option<Arc<dyn RuleFilter>> + Send + Sync>>,
    pub native_rules: Vec<Box<dyn NativeRule>>,
    pub speller: Option<Arc<dyn SpellChecker>>,
    /// Called once the rule files are read, for the filters that run rules
    /// on a corrected text.
    pub rules_loaded: Option<RulesLoaded>,
}

pub type RulesLoaded = Box<dyn FnOnce(&[GrammarRule], &Arc<Analyzer>, &Arc<Disambiguator>)>;

pub struct French {
    pub analyzer: Arc<Analyzer>,
    pub disambiguator: Arc<Disambiguator>,
    pub checker: Checker,
}

/// `JLanguageTool.analyzeText(text).get(0)` for the filters.
struct FirstSentence {
    analyzer: Arc<Analyzer>,
    disambiguator: Arc<Disambiguator>,
}

impl SentenceAnalyzer for FirstSentence {
    fn analyze(&self, text: &str, ctx: &RuleContext) -> Option<Sentence> {
        analyze(&self.analyzer, &self.disambiguator, text, ctx).into_iter().next()
    }
}

/// `JLanguageTool.analyzeText`.
fn analyze(analyzer: &Analyzer, disambiguator: &Disambiguator, text: &str, ctx: &RuleContext) -> Vec<Sentence> {
    analyzer.analyze_with(text, |raw| {
        let pre = raw.tokens.clone();
        let Sentence { text, tokens, .. } = raw;
        disambiguator.disambiguate(Sentence::new(text, tokens, pre), ctx)
    })
}

impl French {
    /// Loads everything from `dir` (LanguageTool's `resource/fr` and
    /// `rules/fr` files side by side). `extend` receives the tagger and the
    /// synthesizer and returns what is plugged into the rules.
    pub fn load_with(dir: &Path, extend: impl FnOnce(&Loaded) -> Extensions) -> io::Result<French> {
        French::load_rule_files(dir, &default_rule_files(dir), extend)
    }

    /// Same, with the given grammar rule files instead of the default ones
    /// (to try a rule file on its own, for instance).
    pub fn load_rule_files(
        dir: &Path,
        rule_files: &[PathBuf],
        extend: impl FnOnce(&Loaded) -> Extensions,
    ) -> io::Result<French> {
        let dictionary = Arc::new(Dictionary::load(dir.join("french.dict"), dir.join("french.info"))?);
        let tagger = Arc::new(FrenchTagger::new(Arc::new(MorfologikDictionary(dictionary.clone())), dir)?);
        let srx = std::fs::read_to_string(dir.join("segment.srx"))?;
        let sentence_tokenizer = super::sentences::SrxSentenceTokenizer::new(&srx, "fr").map_err(io::Error::other)?;
        let analyzer = Arc::new(Analyzer::from_parts(tagger.clone(), sentence_tokenizer));
        let synthesizer = Arc::new(FrenchSynthesizer::load(dir)?);
        let extensions = extend(&Loaded { dir, dictionary: &dictionary, tagger: &tagger, synthesizer: &synthesizer, analyzer: &analyzer });
        let factory = |class: &str| extensions.filters.iter().find_map(|f| f(class));

        // The disambiguation files are read while the grammar files are.
        let mut disambig_unifier = UnifierConfiguration::default();
        let mut unifier = UnifierConfiguration::default();
        let mut grammar_rules: Vec<GrammarRule> = Vec::new();
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(8);
        let factory: &(dyn Fn(&str) -> Option<Arc<dyn RuleFilter>> + Sync) = &factory;
        let (disambiguator, grammar) = std::thread::scope(|scope| {
            let disambiguation = scope.spawn(|| Disambiguator::load(dir, &|class| factory(class), &mut disambig_unifier));
            let grammar = (|| -> io::Result<()> {
                for path in rule_files {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("rules.xml");
                    let style = name.contains("style");
                    let xml = std::fs::read_to_string(path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
                    grammar_rules.extend(load_grammar_parallel(&xml, name, style, factory, &mut unifier, threads));
                }
                Ok(())
            })();
            (disambiguation.join().expect("disambiguation loader panicked"), grammar)
        });
        grammar?;
        let disambiguator = Arc::new(disambiguator?);
        let Extensions { native_rules, speller, rules_loaded, .. } = extensions;
        if let Some(loaded) = rules_loaded {
            loaded(&grammar_rules, &analyzer, &disambiguator);
        }
        let mut rules: Vec<CheckRule> = native_rules.into_iter().map(CheckRule::Native).collect();
        rules.extend(grammar_rules.into_iter().map(|r| match r {
            GrammarRule::Pattern(p) => CheckRule::Pattern(p),
            GrammarRule::Regex(r) => CheckRule::Regex(r),
        }));
        let lang = Lang { synthesizer: Some(synthesizer), tagger: Some(tagger), speller };
        let checker = Checker::new(rules, RuleContext { lang, unifier, disambig_unifier });
        Ok(French { analyzer, disambiguator, checker })
    }

    /// The rule files alone, without native rules, filters or spelling.
    pub fn load(dir: &Path) -> io::Result<French> {
        French::load_with(dir, |_| Extensions::default())
    }

    /// `JLanguageTool.analyzeText`: the sentences of `text`, tagged and
    /// disambiguated.
    pub fn analyze(&self, text: &str) -> Vec<Sentence> {
        analyze(&self.analyzer, &self.disambiguator, text, &self.checker.ctx)
    }

    /// Switch rules on or off, starting from LanguageTool's defaults.
    pub fn apply_profile(&mut self, profile: &Profile) {
        self.checker.select(|info, by_default| {
            let named = |list: &[String]| list.iter().any(|id| *id == info.id || *id == info.full_id());
            if named(&profile.off) {
                false
            } else {
                by_default || named(&profile.on)
            }
        });
    }

    /// `JLanguageTool.check` at the default level.
    pub fn check(&self, text: &str) -> Vec<Found> {
        self.checker.check(&self.analyze(text))
    }

    /// The same, with the analysed sentences the matches were found in.
    pub fn check_with_sentences(&self, text: &str) -> (Vec<Sentence>, Vec<Found>) {
        let sentences = self.analyze(text);
        let found = self.checker.check(&sentences);
        (sentences, found)
    }
}

/// The French checker with everything this crate implements plugged in.
pub fn load_french(dir: &Path) -> io::Result<French> {
    load_full(dir, &default_rule_files(dir), true)
}

/// LanguageTool alone: its rule files, native rules and filters, without
/// this project's rules and rule profile. This is what is compared with the
/// reference implementation.
pub fn load_french_lt(dir: &Path) -> io::Result<French> {
    load_full(dir, &[dir.join("grammar.xml"), dir.join("style.xml")], false)
}

fn load_full(dir: &Path, rule_files: &[PathBuf], with_profile: bool) -> io::Result<French> {
    let mut failure: Option<io::Error> = None;
    let french = French::load_rule_files(dir, rule_files, |loaded| {
        let mut extensions = Extensions::default();
        let filters = FrenchFilters::new();
        let factory = filters.clone();
        extensions.filters.push(Box::new(move |class| factory.filter(class)));
        extensions.rules_loaded = Some(Box::new(move |rules, analyzer, disambiguator| {
            filters.set_rules(rules);
            filters.set_analyzer(Arc::new(FirstSentence { analyzer: analyzer.clone(), disambiguator: disambiguator.clone() }));
        }));
        // Spelling: the rule itself, the language's spell checker and the
        // filters that ask it about their suggestions.
        let analyzer = loaded.analyzer.clone();
        let tokenize: Tokenize = Arc::new(move |text: &str| analyzer.word_tokenizer().tokenize(text));
        let spelling = match FrenchSpelling::load(loaded.dir, loaded.dictionary.clone(), loaded.tagger.clone(), tokenize) {
            Ok(spelling) => Some(spelling),
            Err(e) => {
                failure = Some(e);
                None
            }
        };
        let speller_rule = spelling.as_ref().map(|s| s.native_rule());
        if let Some(spelling) = spelling {
            extensions.speller = Some(spelling.spell_checker());
            extensions.filters.push(Box::new(move |class| spelling.filter(class)));
        }
        match super::native::french_native_rules(loaded.dir, speller_rule) {
            Ok(rules) => extensions.native_rules = rules,
            Err(e) => failure = Some(e),
        }
        extensions
    })?;
    if let Some(e) = failure {
        return Err(e);
    }
    let mut french = french;
    if with_profile {
        if let Some(profile) = dir.parent().map(|p| p.join("grammaria").join("profile.txt")).and_then(|p| std::fs::read_to_string(p).ok()) {
            french.apply_profile(&Profile::parse(&profile));
        }
    }
    Ok(french)
}

/// This project's choice of rules, on top of LanguageTool's defaults: the
/// file `grammaria/profile.txt` holds one `off RULE_ID` or `on RULE_ID` per
/// line (a rule id or a full id such as `OU[40]`; `#` starts a comment).
#[derive(Default, Clone, Debug)]
pub struct Profile {
    pub off: Vec<String>,
    pub on: Vec<String>,
}

impl Profile {
    pub fn parse(text: &str) -> Profile {
        let mut profile = Profile::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let mut words = line.split_whitespace();
            match (words.next(), words.next()) {
                (Some("off"), Some(id)) => profile.off.push(id.to_string()),
                (Some("on"), Some(id)) => profile.on.push(id.to_string()),
                _ => {}
            }
        }
        profile
    }
}

/// The same checker with the given rule files only.
pub fn load_french_with_rules(dir: &Path, rule_files: &[PathBuf]) -> io::Result<French> {
    French::load_rule_files(dir, rule_files, |_| Extensions::default())
}
