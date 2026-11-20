//! Memory taken by the rule files when loaded sequentially.
use std::sync::Arc;

use grammaria::engine::rule::RuleFilter;
use grammaria::engine::unifier::UnifierConfiguration;
use grammaria::engine::xmlrules::{load_disambiguation, load_grammar, GrammarRule};

fn rss() -> usize {
    std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)?.parse::<usize>().ok())).unwrap_or(0) / 1024
}

fn main() {
    let dir = "assets/fr/lt";
    let none = |_: &str| -> Option<Arc<dyn RuleFilter>> { None };
    let mut unifier = UnifierConfiguration::default();
    println!("start: {} MiB", rss());
    let mut all = Vec::new();
    for (file, style) in [("grammar.xml", false), ("style.xml", true)] {
        let xml = std::fs::read_to_string(format!("{dir}/{file}")).unwrap();
        let rules = load_grammar(&xml, file, style, &none, &mut unifier);
        drop(xml);
        let mut tokens = 0usize;
        let mut exceptions = 0usize;
        let mut hints = 0usize;
        let mut hint_bytes = 0usize;
        let mut anti = 0usize;
        for r in &rules {
            if let GrammarRule::Pattern(p) = r {
                anti += p.anti_patterns.len();
                let mut count = |pattern: &grammaria::engine::performer::TokenPattern| {
                    tokens += pattern.tokens.len();
                    for t in &pattern.tokens {
                        exceptions += t.current_and_next_exceptions.len() + t.previous_exceptions.len();
                    }
                    for h in &pattern.token_hints {
                        hints += h.lower_case_values.len();
                        hint_bytes += h.lower_case_values.iter().map(|v| v.len() + 24).sum::<usize>();
                    }
                };
                count(&p.rule.pattern);
                for a in &p.anti_patterns {
                    count(&a.rule.pattern);
                }
            }
        }
        println!("{file}: {} rules, {anti} antipatterns, {tokens} tokens, {exceptions} exceptions, {hints} hint values ({} MiB) -> {} MiB", rules.len(), hint_bytes >> 20, rss());
        all.push(rules);
    }
    let mut dis = UnifierConfiguration::default();
    let d = load_disambiguation(&std::fs::read_to_string(format!("{dir}/disambiguation.xml")).unwrap(), &none, &mut dis);
    println!("disambiguation: {} rules -> {} MiB", d.len(), rss());
    println!("size_of PatternToken = {}, TokenRule = {}", std::mem::size_of::<grammaria::engine::pattern::PatternToken>(), std::mem::size_of::<grammaria::engine::rule::TokenRule>());
}
