//! Trace one rule on one text of a token dump: raw matches, then which
//! antipatterns immunize which tokens.
//!
//!   cargo run --release --example debug_rule -- TOKENS.jsonl LINE RULE_ID[sub]
use std::sync::Arc;

use grammaria::engine::dump::sentences_from_dump;
use grammaria::engine::pattern::Lang;
use grammaria::engine::rule::{RuleContext, RuleFilter};
use grammaria::engine::synth::FrenchSynthesizer;
use grammaria::engine::unifier::UnifierConfiguration;
use grammaria::engine::xmlrules::{load_grammar, GrammarRule};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::Path::new("assets/fr/lt");
    let line_no: usize = args[1].parse().unwrap();
    let wanted = &args[2];
    let input = std::fs::read_to_string(&args[0]).unwrap();
    let line = input.lines().nth(line_no - 1).unwrap();
    let none = |_: &str| -> Option<Arc<dyn RuleFilter>> { None };
    let mut unifier = UnifierConfiguration::default();
    let mut rules = Vec::new();
    for (file, style) in [("grammar.xml", false), ("style.xml", true)] {
        let xml = std::fs::read_to_string(dir.join(file)).unwrap();
        rules.extend(load_grammar(&xml, file, style, &none, &mut unifier));
    }
    let mut lang = Lang::default();
    lang.synthesizer = FrenchSynthesizer::load(dir).ok().map(|s| Arc::new(s) as _);
    let ctx = RuleContext {
        lang,
        unifier,
        disambig_unifier: UnifierConfiguration::default(),
    };
    for s in sentences_from_dump(line) {
        println!("sentence: {:?}", &*s.text);
        for (i, rule) in rules.iter().enumerate() {
            let info = rule.info();
            if info.full_id() != *wanted && info.id != *wanted {
                continue;
            }
            let GrammarRule::Pattern(p) = rule else {
                continue;
            };
            let raw = p.rule.match_sentence(i, &s, &ctx, &ctx.unifier);
            if raw.is_empty() {
                continue;
            }
            println!(
                "{} raw: {:?}",
                info.full_id(),
                raw.iter()
                    .map(|m| (&s.text[m.from_pos..m.to_pos], &m.suggestions))
                    .collect::<Vec<_>>()
            );
            for (k, ap) in p.anti_patterns.iter().enumerate() {
                if let Some(next) = ap.replace(&s, &ctx) {
                    let imm: Vec<&str> = next
                        .tokens
                        .iter()
                        .filter(|t| t.immunized)
                        .map(|t| &*t.token)
                        .collect();
                    if !imm.is_empty() {
                        println!("  antipattern #{k} immunizes {imm:?}");
                    }
                }
            }
            let fin = p.match_sentence(i, &s, &ctx);
            println!(
                "  final: {:?}",
                fin.iter()
                    .map(|m| &s.text[m.from_pos..m.to_pos])
                    .collect::<Vec<_>>()
            );
        }
    }
}
