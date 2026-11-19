//! Grammaria command line interface.

use std::io::Read;
use std::path::{Path, PathBuf};

fn usage() {
    eprintln!(
        "grammaria {}\n\
         A fast French grammar and spelling checker (LanguageTool-compatible).\n\n\
         USAGE:\n  grammaria check [OPTIONS] FILE...\n\n\
         OPTIONS:\n\
         \x20 --json             machine-readable output (LanguageTool --json format)\n\
         \x20 --tagger           print POS analysis instead of checking\n\
         \x20 --style            also run style rules\n\
         \x20 --data DIR         data directory (default: ./data or $GRAMMARIA_DATA)\n\
         \x20 -                   read from stdin\n",
        env!("CARGO_PKG_VERSION")
    );
}

fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("GRAMMARIA_DATA") {
        return PathBuf::from(d);
    }
    PathBuf::from("data")
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        usage();
        return;
    }
    let mut json = false;
    let mut tagger_only = false;
    let mut style = false;
    let mut files: Vec<String> = Vec::new();
    let mut data: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--tagger" => tagger_only = true,
            "--style" => style = true,
            "--data" => {
                i += 1;
                data = args.get(i).cloned();
            }
            f => files.push(f.to_string()),
        }
        i += 1;
    }
    let dir = data.map(PathBuf::from).unwrap_or_else(data_dir);
    let tg = match grammaria::tagger::Tagger::new(
        &dir.join("fr.lex"),
        &dir.join("fr.words"),
        Some(&dir.join("fr.disambig.xml")),
    ) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("grammaria: cannot load data from {}: {e}", dir.display());
            std::process::exit(1);
        }
    };
    let rules = load_rules(&dir);

    let mut exit_code = 0;
    for f in &files {
        let text = if f == "-" {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).ok();
            s
        } else {
            std::fs::read_to_string(f).unwrap_or_default()
        };
        if tagger_only {
            for (a, b) in grammaria::tokenizer::sentence_boundaries(&text) {
                let s = tg.tag(&text[a..b], a);
                println!("{}", tg.verbose(&s));
            }
            continue;
        }
        let matches = grammaria::rules::check_text(tg, &rules, &text, style);
        if !matches.is_empty() {
            exit_code = 1;
        }
        if json {
            print_json(&text, &matches, &f);
        } else {
            print_text(&text, &matches);
        }
    }
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

fn load_rules(dir: &Path) -> grammaria::rules::RuleSet {
    let mut set = grammaria::rules::RuleSet::default();
    for (file, is_style) in [("fr.grammar.xml", false), ("fr.style.xml", true)] {
        let p = dir.join(file);
        if let Ok(xml) = std::fs::read_to_string(&p) {
            let s = grammaria::rules::RuleSet::parse(&xml, is_style);
            set.rules.extend(s.rules);
            set.unification.extend(s.unification);
            set.phrases.extend(s.phrases);
        }
    }
    set
}

fn print_text(text: &str, matches: &[grammaria::rules::RuleMatch]) {
    for m in matches {
        let ctx_start = text[..m.offset.min(text.len())].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let ctx_end = text[m.offset.min(text.len())..]
            .find('\n')
            .map(|i| i + m.offset)
            .unwrap_or(text.len());
        let context = &text[ctx_start..ctx_end];
        let rel = m.offset.saturating_sub(ctx_start);
        println!(
            "{}:{}: {} [{}]{}",
            m.offset,
            m.length,
            context.trim(),
            m.rule_id,
            if m.suggestions.is_empty() {
                String::new()
            } else {
                format!("  suggestions: {}", m.suggestions.join(", "))
            }
        );
        println!("  {}", m.message);
        let _ = rel;
    }
}

fn print_json(_text: &str, matches: &[grammaria::rules::RuleMatch], file: &str) {
    let mut out = String::from("{\"software\":{\"name\":\"Grammaria\",\"version\":\"");
    out.push_str(env!("CARGO_PKG_VERSION"));
    out.push_str("\"},\"language\":{\"name\":\"French\",\"code\":\"fr\"},\"matches\":[");
    for (i, m) in matches.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"message\":\"{}\",\"shortMessage\":\"{}\",\"replacements\":[{}],\"offset\":{},\"length\":{},\"rule\":{{\"id\":\"{}\",\"subId\":\"{}\",\"description\":\"{}\",\"category\":{{\"id\":\"{}\",\"name\":\"{}\"}}}}}}",
            json_escape(&m.message),
            json_escape(&m.short_message),
            m.suggestions
                .iter()
                .map(|s| format!("{{\"value\":\"{}\"}}", json_escape(s)))
                .collect::<Vec<_>>()
                .join(","),
            m.offset,
            m.length,
            json_escape(&m.rule_id),
            json_escape(&m.full_rule_id),
            json_escape(&m.category_name),
            json_escape(&m.category_id),
            json_escape(&m.category_name)
        ));
    }
    out.push_str("]}");
    println!("{out}");
    let _ = file;
}
