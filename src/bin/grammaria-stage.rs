//! Run one stage of the new engine on the reference checker's dump of the
//! previous stage, so that each stage can be compared in isolation
//! (`tools/bench/`).
//!
//!   grammaria-stage disambig RAW.jsonl      tagger dump -> disambiguated tokens
//!   grammaria-stage match    TOKENS.jsonl [RAW.jsonl]   disambiguated dump (and the tagger's) -> rule matches
//!
//! `--assets DIR` names the LanguageTool resource directory (default
//! `assets/fr/lt`). `--rules native|xml` restricts `match` to the rules
//! written in code or to those of the rule files (default: all).

use std::path::PathBuf;
use std::sync::Arc;

use grammaria::api::json_escape;
use grammaria::engine::checker::{CheckRule, Checker};
use grammaria::engine::disambiguator::Disambiguator;
use grammaria::engine::dump::{dump_sentences, sentences_from_dump, sentences_from_dumps};
use grammaria::engine::native::french_native_rules;
use grammaria::engine::pattern::Lang;
use grammaria::engine::rule::{RuleContext, RuleFilter};
use grammaria::engine::synth::FrenchSynthesizer;
use grammaria::engine::unifier::UnifierConfiguration;
use grammaria::engine::xmlrules::{load_grammar, GrammarRule};

fn no_filters(_: &str) -> Option<Arc<dyn RuleFilter>> {
    None
}

fn main() {
    let mut assets = PathBuf::from("assets/fr/lt");
    let mut which_rules = String::from("all");
    let mut positional: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--assets" => assets = PathBuf::from(args.next().unwrap_or_default()),
            "--rules" => which_rules = args.next().unwrap_or_default(),
            _ => positional.push(a),
        }
    }
    let (Some(mode), Some(file)) = (positional.first(), positional.get(1)) else {
        eprintln!("usage: grammaria-stage (disambig|match) DUMP.jsonl [--assets DIR] [--rules native|xml]");
        std::process::exit(2);
    };
    let input = std::fs::read_to_string(file).expect("dump file");
    let lines: Vec<(usize, &str)> = input.lines().enumerate().collect();
    // The tagger's dump of the same texts, for `raw_pos` rules.
    let raw_input = positional.get(2).map(|f| std::fs::read_to_string(f).expect("raw dump file")).unwrap_or_default();
    let raw_lines: Vec<&str> = raw_input.lines().collect();
    let raw_lines = &raw_lines;
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let chunk = lines.len().div_ceil(threads).max(1);

    let mut unifier = UnifierConfiguration::default();
    let mut dis_unifier = UnifierConfiguration::default();
    let disambiguator = Disambiguator::load(&assets, &no_filters, &mut dis_unifier).expect("disambiguation files");
    let mut rules: Vec<GrammarRule> = Vec::new();
    if mode == "match" && which_rules != "native" {
        for (name, style) in [("grammar.xml", false), ("style.xml", true)] {
            let xml = std::fs::read_to_string(assets.join(name)).expect("rule file");
            rules.extend(load_grammar(&xml, name, style, &no_filters, &mut unifier));
        }
    }
    let mut lang = Lang::default();
    if let Ok(synth) = FrenchSynthesizer::load(&assets) {
        lang.synthesizer = Some(Arc::new(synth));
    }
    // LanguageTool's order: the rules written in code, then the rule files.
    let mut native: Vec<CheckRule> = Vec::new();
    if mode == "match" && which_rules != "xml" {
        let rules = french_native_rules(&assets, None).expect("native rule data");
        native.extend(rules.into_iter().map(CheckRule::Native));
    }
    let rules: Vec<CheckRule> = native
        .into_iter()
        .chain(rules.into_iter().map(|r| match r {
            GrammarRule::Pattern(p) => CheckRule::Pattern(p),
            GrammarRule::Regex(r) => CheckRule::Regex(r),
        }))
        .collect();
    let checker = Checker::new(rules, RuleContext { lang, unifier, disambig_unifier: dis_unifier });
    let (ctx, disambiguator, checker) = (&checker.ctx, &disambiguator, &checker);

    let out: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = lines
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .map(|(no, line)| match mode.as_str() {
                            "disambig" => {
                                let raw = sentences_from_dump(line);
                                let n = raw.len();
                                let mut done: Vec<_> = raw.into_iter().map(|s| disambiguator.disambiguate(s, ctx)).collect();
                                if n > 0 {
                                    // markAsParagraphEnd
                                    let last = &mut done[n - 1];
                                    if let Some(t) = last.tokens.last_mut() {
                                        t.set_paragraph_end();
                                    }
                                }
                                dump_sentences(&done, false)
                            }
                            _ => {
                                let sentences = sentences_from_dumps(line, raw_lines.get(*no).copied());
                                let mut parts: Vec<String> = Vec::new();
                                for m in checker.check(&sentences) {
                                    let info = checker.info(m.rule);
                                    let text: String = sentences.iter().map(|s| &*s.text).collect();
                                    let from = text[..m.from_byte].chars().count();
                                    let len = text[m.from_byte..m.to_byte].chars().count();
                                    parts.push(format!(
                                        "[\"{}\",{},{},[{}],\"{}\",\"{}\",\"{}\"]",
                                        json_escape(&info.id),
                                        from,
                                        len,
                                        m.suggestions.iter().take(5).map(|x| format!("\"{}\"", json_escape(x))).collect::<Vec<_>>().join(","),
                                        json_escape(&info.category_id),
                                        json_escape(&info.full_id()),
                                        json_escape(&info.issue_type),
                                    ));
                                }
                                format!("[{}]", parts.join(","))
                            }
                        })
                        .collect::<Vec<String>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().expect("worker panicked")).collect()
    });
    let mut buf = String::new();
    for l in &out {
        buf.push_str(l);
        buf.push('\n');
    }
    print!("{buf}");
}
