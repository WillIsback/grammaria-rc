//! Rule filters at work in LanguageTool's French rules: the rule files, the
//! disambiguator and the filters together, on sentences analysed with the
//! stopgap tokenizer and tagger of `engine::filters::analysis`.
//!
//! Skipped when the LanguageTool resources are not there.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use grammaria::engine::checker::{CheckRule, Checker};
use grammaria::engine::disambiguator::Disambiguator;
use grammaria::engine::filters::{
    french_filter, FrenchFilters, SentenceAnalyzer, SimpleAnalyzer, SimpleDictTagger, Today,
};
use grammaria::engine::pattern::Lang;
use grammaria::engine::rule::RuleContext;
use grammaria::engine::synth::FrenchSynthesizer;
use grammaria::engine::unifier::UnifierConfiguration;
use grammaria::engine::xmlrules::{load_grammar, GrammarRule};

struct Engine {
    checker: Checker,
    analyzer: Arc<SimpleAnalyzer>,
}

fn engine() -> Option<&'static Engine> {
    static ENGINE: OnceLock<Option<Engine>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fr/lt");
            let xml = std::fs::read_to_string(assets.join("grammar.xml")).ok()?;
            let mut unifier = UnifierConfiguration::default();
            let mut disambig_unifier = UnifierConfiguration::default();
            let disambiguator =
                Arc::new(Disambiguator::load(&assets, &french_filter, &mut disambig_unifier).ok()?);
            let rules = load_grammar(&xml, "grammar.xml", false, &french_filter, &mut unifier);
            let analyzer = Arc::new(SimpleAnalyzer::new(disambiguator));
            let filters = FrenchFilters::global();
            filters.set_rules(&rules);
            filters.set_analyzer(analyzer.clone());
            let mut lang = Lang::default();
            lang.synthesizer = Some(Arc::new(FrenchSynthesizer::load(&assets).ok()?));
            lang.tagger = Some(Arc::new(SimpleDictTagger::load(&assets).ok()?));
            let rules: Vec<CheckRule> = rules
                .into_iter()
                .map(|r| match r {
                    GrammarRule::Pattern(p) => CheckRule::Pattern(p),
                    GrammarRule::Regex(r) => CheckRule::Regex(r),
                })
                .collect();
            let checker = Checker::new(
                rules,
                RuleContext {
                    lang,
                    unifier,
                    disambig_unifier,
                },
            );
            Some(Engine { checker, analyzer })
        })
        .as_ref()
}

/// The matches of `rule` in `text`: (matched text, suggestions).
fn matches_of(rule: &str, text: &str) -> Option<Vec<(String, Vec<String>)>> {
    let engine = engine()?;
    let sentence = engine.analyzer.analyze(text, &engine.checker.ctx)?;
    Some(
        engine
            .checker
            .check(&[sentence])
            .into_iter()
            .filter(|m| engine.checker.info(m.rule).id == rule)
            .map(|m| (text[m.from_byte..m.to_byte].to_string(), m.suggestions))
            .collect(),
    )
}

fn expect(rule: &str, text: &str, span: &str, suggestions: &[&str]) {
    let Some(found) = matches_of(rule, text) else {
        return;
    };
    let wanted = vec![(
        span.to_string(),
        suggestions
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
    )];
    assert_eq!(found, wanted, "{rule} in {text:?}");
}

#[test]
fn determiner_and_noun_pairs_pass_the_elision_rules() {
    // "Le homme", "Ce homme", "sa équipe" are weeded out by the elision
    // rules run on each candidate.
    expect(
        "D_N",
        "La homme chante.",
        "La homme",
        &["L'homme", "Les hommes"],
    );
    expect(
        "D_N",
        "Cette homme chante.",
        "Cette homme",
        &["Cet homme", "Ces hommes"],
    );
    expect(
        "D_N",
        "Il joue pour ses équipe de handball.",
        "ses équipe",
        &["son équipe", "ses équipes"],
    );
    expect(
        "D_N",
        "Les voiture roulent.",
        "Les voiture",
        &["La voiture", "Les voitures"],
    );
    expect(
        "D_N",
        "Il parle aux femme.",
        "aux femme",
        &["à la femme", "aux femmes"],
    );
}

#[test]
fn a_correct_pair_is_not_reported() {
    let Some(found) = matches_of("D_N", "Les voitures roulent vite.") else {
        return;
    };
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn postponed_adjectives() {
    expect(
        "AGREEMENT_POSTPONED_ADJ",
        "Il habite une maison très grand.",
        "grand",
        &["grande"],
    );
    let Some(found) = matches_of(
        "AGREEMENT_POSTPONED_ADJ",
        "Il habite une maison très grande.",
    ) else {
        return;
    };
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn week_day_of_a_date() {
    // 28 August 2014 was a Thursday; 28 August 2015, a Friday.
    let filters = FrenchFilters::global();
    filters.set_today(Some(Today {
        year: 2014,
        month: 9,
        day: 1,
    }));
    expect(
        "FR_DATE_WEEKDAY",
        "Il vient le vendredi 28 août 2014.",
        "vendredi 28",
        &["jeudi 28", "vendredi 29"],
    );
    expect(
        "FR_DATE_WEEKDAY",
        "Il vient le Ven 28/08/2014.",
        "Ven 28",
        &["Jeudi 28", "Ven 29"],
    );
    let Some(found) = matches_of("FR_DATE_WEEKDAY", "Il vient le jeudi 28 août 2014.") else {
        return;
    };
    assert!(found.is_empty(), "{found:?}");
    // Read in 2015, the week day is right and the year is the mistake.
    filters.set_today(Some(Today {
        year: 2015,
        month: 9,
        day: 1,
    }));
    expect(
        "FR_DATE_WEEKDAY",
        "Il vient le vendredi 28 août 2014.",
        "2014",
        &["2015"],
    );
    filters.set_today(None);
}
