//! Load the French rule files and print what was read (a smoke test and a
//! quick way to time the loaders).
use std::sync::Arc;

use grammaria::engine::rule::{FilterRef, RuleFilter};
use grammaria::engine::unifier::UnifierConfiguration;
use grammaria::engine::xmlrules::{load_disambiguation, load_grammar, GrammarRule};

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "assets/fr/lt".into());
    let none = |_: &str| -> Option<Arc<dyn RuleFilter>> { None };
    let mut unifier = UnifierConfiguration::default();
    let mut dis_unifier = UnifierConfiguration::default();
    for (file, style) in [("grammar.xml", false), ("style.xml", true)] {
        let t = std::time::Instant::now();
        let xml = std::fs::read_to_string(format!("{dir}/{file}")).expect("rule file");
        let rules = load_grammar(&xml, file, style, &none, &mut unifier);
        let mut pattern = 0;
        let mut regex = 0;
        let mut off = 0;
        let mut unsupported = std::collections::BTreeMap::<String, usize>::new();
        let mut anti = 0;
        let mut invalid = 0;
        for r in &rules {
            if r.info().default_off || r.info().default_temp_off {
                off += 1;
            }
            match r {
                GrammarRule::Pattern(p) => {
                    pattern += 1;
                    anti += p.anti_patterns.len();
                    if let Some(FilterRef::Unsupported(c)) = &p.rule.filter {
                        *unsupported.entry(c.clone()).or_default() += 1;
                    }
                    if p.rule.pattern.tokens.iter().any(|t| !t.text.is_valid() || t.pos.as_ref().is_some_and(|p| p.pos_pattern.as_ref().is_some_and(|m| !m.is_valid()))) {
                        invalid += 1;
                    }
                }
                GrammarRule::Regex(_) => regex += 1,
            }
        }
        println!("{file}: {} rules ({pattern} pattern, {regex} regexp) in {:?}; off by default: {off}; antipatterns: {anti}; rules with an invalid regex: {invalid}", rules.len(), t.elapsed());
        println!("  filters without native port: {unsupported:?}");
    }
    for file in ["disambiguation.xml", "disambiguation-global.xml"] {
        let t = std::time::Instant::now();
        let xml = std::fs::read_to_string(format!("{dir}/{file}")).expect("rule file");
        let rules = load_disambiguation(&xml, &none, &mut dis_unifier);
        println!("{file}: {} rules in {:?}", rules.len(), t.elapsed());
    }
}
