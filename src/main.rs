use std::path::PathBuf;

use grammaria::api::{json_escape, matches_json, read_all, Engine, Match, OffsetUnit};

fn usage() {
    eprintln!(
        "grammaria {}\n\
         A fast French grammar and spelling checker (LanguageTool-compatible).\n\n\
         USAGE:\n  grammaria check [OPTIONS] FILE...\n\
         \x20 grammaria serve [--no-style] [--data DIR]\n\n\
         `serve` keeps the data loaded and answers framed requests on stdin\n\
         (`<id> <byte length>\\n<text>`) with one JSON line each on stdout;\n\
         offsets are UTF-16 code units.\n\n\
         OPTIONS:\n\
         \x20 --json             machine-readable output (LanguageTool --json format)\n\
         \x20 --tagger           print the analysis (tokens and tags) instead of checking\n\
         \x20 --no-style         leave out the style rules (wordiness, repetitions, register)\n\
         \x20 --data DIR         data directory (default: $GRAMMARIA_DATA, ./data or ./assets/fr)\n\
         \x20 --watch SECS       re-check the file every SECS seconds when it changed\n\
         \x20 -                   read from stdin\n",
        env!("CARGO_PKG_VERSION")
    );
}

/// `$GRAMMARIA_DATA`, else `./data`, else the assets of a source checkout.
fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("GRAMMARIA_DATA") {
        return PathBuf::from(d);
    }
    let data = PathBuf::from("data");
    if data.join("lt").join("grammar.xml").exists() || data.join("grammar.xml").exists() {
        return data;
    }
    PathBuf::from("assets/fr")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        usage();
        return;
    }
    let mut json = false;
    let mut tagger_only = false;
    let mut watch: Option<u64> = None;
    let mut style = true;
    let serve = args[0] == "serve";
    let mut files: Vec<String> = Vec::new();
    let mut data: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--tagger" => tagger_only = true,
            "--watch" => {
                i += 1;
                watch = args.get(i).and_then(|v| v.parse().ok());
            }
            // Style rules are on by default, as in LanguageTool; `--style`
            // is accepted for older command lines.
            "--style" => style = true,
            "--no-style" => style = false,
            "--data" => {
                i += 1;
                data = args.get(i).cloned();
            }
            f => files.push(f.to_string()),
        }
        i += 1;
    }
    let dir = data.map(PathBuf::from).unwrap_or_else(data_dir);
    let engine = match Engine::load(&dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!(
                "grammaria: cannot load the language data from {}: {e}\n\
                 The data directory holds `lt/` (LanguageTool's French resources) and `grammaria/` (this\n\
                 project's rules); `grammaria-build --out DIR` makes one from the shipped assets.",
                dir.display()
            );
            std::process::exit(1);
        }
    };
    if serve {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        if let Err(e) = engine.serve(stdin.lock(), stdout.lock(), style) {
            eprintln!("grammaria serve: {e}");
            std::process::exit(1);
        }
        return;
    }

    if let Some(secs) = watch {
        let path = files.first().cloned().unwrap_or_else(|| "-".to_string());
        if path == "-" {
            eprintln!("grammaria: --watch needs a file, not stdin");
            std::process::exit(2);
        }
        let mut last: Option<String> = None;
        loop {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            if last.as_deref() != Some(text.as_str()) {
                let matches = engine.check(&text, style);
                if json {
                    print_json(&engine, &text, &matches, &path);
                } else {
                    print_text(&text, &matches);
                }
                last = Some(text);
            }
            std::thread::sleep(std::time::Duration::from_secs(secs.max(1)));
        }
    }

    let mut exit_code = 0;
    for f in &files {
        let text = if f == "-" {
            read_all(std::io::stdin())
        } else {
            std::fs::read_to_string(f).unwrap_or_default()
        };
        if tagger_only {
            print_analysis(&engine, &text);
            continue;
        }
        let matches = engine.check(&text, style);
        if !matches.is_empty() {
            exit_code = 1;
        }
        if json {
            print_json(&engine, &text, &matches, f);
        } else {
            print_text(&text, &matches);
        }
    }
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

/// One token per line: `token<TAB>lemma/TAG lemma/TAG ...`, a blank line
/// between sentences.
fn print_analysis(engine: &Engine, text: &str) {
    for sentence in engine.analyze(text) {
        for token in &sentence.tokens {
            if token.is_whitespace && !token.is_sent_start {
                continue;
            }
            let readings: Vec<String> = token
                .readings
                .iter()
                .map(|r| {
                    format!(
                        "{}/{}",
                        r.lemma.as_deref().unwrap_or("-"),
                        r.pos().unwrap_or("-")
                    )
                })
                .collect();
            println!("{}\t{}", token.token, readings.join(" "));
        }
        println!();
    }
}

fn print_text(text: &str, matches: &[Match]) {
    for m in matches {
        let offset = m.offset.min(text.len());
        let ctx_start = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let ctx_end = text[offset..]
            .find('\n')
            .map(|i| i + offset)
            .unwrap_or(text.len());
        let context = &text[ctx_start..ctx_end];
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
        println!("  {}", grammaria::api::plain_message(&m.message));
    }
}

fn print_json(engine: &Engine, text: &str, matches: &[Match], file: &str) {
    // LanguageTool's `--json` envelope: `software`, `language` and
    // `matches[]`, where each match carries its `context`, `sentence`, `type`
    // and full `rule` descriptor.
    let mut out = String::from("{\"software\":{\"name\":\"Grammaria\",\"version\":\"");
    out.push_str(env!("CARGO_PKG_VERSION"));
    out.push_str("\",\"apiVersion\":1,\"premium\":false,\"status\":\"\"}");
    out.push_str(",\"warnings\":{\"incompleteResults\":false}");
    out.push_str(",\"language\":{\"name\":\"French\",\"code\":\"fr\"}");
    out.push_str(&format!(",\"file\":\"{}\"", json_escape(file)));
    out.push_str(",\"matches\":");
    out.push_str(&matches_json(text, matches, OffsetUnit::Bytes));
    // `sentenceRanges` lets clients map offsets back to sentences.
    let ranges: Vec<String> = engine
        .french()
        .analyzer
        .sentence_ranges(text)
        .iter()
        .map(|(a, b)| format!("[{a},{b}]"))
        .collect();
    out.push_str(&format!(",\"sentenceRanges\":[{}]", ranges.join(",")));
    out.push('}');
    println!("{out}");
}
