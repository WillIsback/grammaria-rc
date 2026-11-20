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
         \x20 --watch SECS       re-check the file every SECS seconds, reusing the\n\
         \x20                    cached analysis of unchanged sentences\n\
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
    let mut watch: Option<u64> = None;
    let mut style = false;
    let mut rest_json: Option<String> = None;
    let _ = &mut rest_json;
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
    // The disambiguation XML is shipped verbatim in `assets/fr/` (generated
    // data directories also carry a copy).
    let disambig = ["fr.disambig.xml", "languagetool-disambiguation.xml"]
        .iter()
        .find_map(|n| {
            let p = dir.join(n);
            if p.exists() { Some(p) } else { None }
        })
        .or_else(|| {
            let p = Path::new("assets/fr/languagetool-disambiguation.xml");
            if p.exists() { Some(p.to_path_buf()) } else { None }
        });
    let tg = match grammaria::tagger::Tagger::new(
        &dir.join("fr.lex"),
        &dir.join("fr.words"),
        disambig.as_deref(),
    ) {
        Ok(t) => t,
        Err(e) => {
            let lex = dir.join("fr.lex");
            if !lex.exists() {
                eprintln!(
                    "grammaria: no language data in {} (missing fr.lex).\n\
                     Build it once from the shipped assets:\n\
                     \x20 cargo run --release --bin grammaria-build -- --out {}",
                    dir.display(),
                    dir.display()
                );
            } else {
                eprintln!("grammaria: cannot load data from {}: {e}", dir.display());
            }
            std::process::exit(1);
        }
    };
    let rules = load_rules(&dir);

    // `--watch`: re-check the file on an interval, reusing the cached analysis
    // of sentences that did not change. Only one file is supported.
    if let Some(secs) = watch {
        let path = files.first().cloned().unwrap_or_else(|| "-".to_string());
        let mut checker = grammaria::rules::Checker::new(tg, &rules);
        let mut last: Option<String> = None;
        loop {
            let text = if path == "-" {
                String::new()
            } else {
                std::fs::read_to_string(&path).unwrap_or_default()
            };
            if path == "-" {
                eprintln!("grammaria: --watch needs a file, not stdin");
                break;
            }
            if last.as_deref() != Some(text.as_str()) {
                let matches = checker.check(&text, style);
                if json {
                    print_json(&text, &matches, &path);
                } else {
                    print_text(&text, &matches);
                    eprintln!(
                        "  ({} sentences reused, {} re-checked)",
                        checker.reused, checker.computed
                    );
                }
                last = Some(text);
            }
            std::thread::sleep(std::time::Duration::from_secs(secs.max(1)));
        }
        return;
    }

    let mut exit_code = 0;
    for f in &files {
        let text = if f == "-" {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).ok();
            s
        } else {
            std::fs::read_to_string(f).unwrap_or_default()
        };
        if let Some(rest) = rest_json.as_ref() {
            let _ = rest;
        }
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
    // Fast path: the precompiled binary rule cache.
    if let Some(set) = grammaria::rules::RuleSet::load_cache(dir.join("fr.rules")) {
        return set;
    }
    // Fallback: parse the XML rule files directly.
    let mut set = grammaria::rules::RuleSet::default();
    for (file, asset, is_style) in [
        ("fr.grammar.xml", "assets/fr/languagetool-grammar.xml", false),
        ("fr.style.xml", "assets/fr/languagetool-style.xml", true),
    ] {
        // Prefer the data directory (generated copies), fall back to the
        // shipped assets so a fresh clone works without a data build.
        let xml = std::fs::read_to_string(dir.join(file))
            .or_else(|_| std::fs::read_to_string(asset))
            .ok();
        if let Some(xml) = xml {
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

fn print_json(text: &str, matches: &[grammaria::rules::RuleMatch], file: &str) {
    // Mirrors LanguageTool's `--json` response object so that the output can be
    // consumed by the same clients: `software`, `warnings`, `language` and
    // `matches[]`, where each match carries its `context`, `sentence`, `type`
    // and full `rule` descriptor.
    let mut out = String::from("{\"software\":{\"name\":\"Grammaria\",\"version\":\"");
    out.push_str(env!("CARGO_PKG_VERSION"));
    out.push_str("\",\"apiVersion\":1,\"premium\":false,\"status\":\"\"}");
    out.push_str(",\"warnings\":{\"incompleteResults\":false}");
    out.push_str(",\"language\":{\"name\":\"French\",\"code\":\"fr\"}");
    out.push_str(&format!(",\"file\":\"{}\"", json_escape(file)));
    out.push_str(",\"matches\":[");
    for (i, m) in matches.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        // `context` is a window around the match: LanguageTool pads it with the
        // surrounding sentence; we use the containing sentence, clamped to a
        // reasonable window so long sentences do not dominate the payload.
        let (ctx_start, ctx_end) = context_window(text, m.offset, m.length);
        let ctx_text = text.get(ctx_start..ctx_end).unwrap_or("");
        out.push_str(&format!(
            "{{\"message\":\"{}\",\"shortMessage\":\"{}\",\"replacements\":[{}],\"offset\":{},\"length\":{},\"context\":{{\"text\":\"{}\",\"offset\":{},\"length\":{}}},\"sentence\":\"{}\",\"type\":{{\"typeName\":\"{}\"}},\"rule\":{{\"id\":\"{}\",\"subId\":\"{}\",\"sourceFile\":\"{}\",\"description\":\"{}\",\"issueType\":\"{}\",\"category\":{{\"id\":\"{}\",\"name\":\"{}\"}}}}}}",
            json_escape(&m.message),
            json_escape(&m.short_message),
            m.suggestions
                .iter()
                .map(|s| format!("{{\"value\":\"{}\"}}", json_escape(s)))
                .collect::<Vec<_>>()
                .join(","),
            m.offset,
            m.length,
            json_escape(ctx_text),
            m.offset.saturating_sub(ctx_start),
            m.length,
            json_escape(&m.sentence),
            if m.is_style { "style" } else { "Other" },
            json_escape(&m.rule_id),
            json_escape(&m.full_rule_id),
            if m.is_style { "style.xml" } else { "grammar.xml" },
            json_escape(&m.category_name),
            if m.is_style { "style" } else { "uncategorized" },
            json_escape(&m.category_id),
            json_escape(&m.category_name)
        ));
    }
    // `sentenceRanges` lets clients map offsets back to sentences;
    // `extendedSentenceRanges` mirrors LanguageTool's list of paragraph ranges.
    out.push_str("],\"sentenceRanges\":");
    out.push_str(&json_ranges(text, false));
    out.push_str(",\"extendedSentenceRanges\":");
    out.push_str(&json_ranges(text, true));
    out.push('}');
    println!("{out}");
}

/// `[[from, to], ...]` character ranges of the sentences (or paragraphs) in
/// `text`, as LanguageTool reports them.
fn json_ranges(text: &str, paragraphs: bool) -> String {
    let bytes = text.as_bytes();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    let terminators: &[u8] = if paragraphs { b"\n" } else { b".!?\n" };
    for (i, &c) in bytes.iter().enumerate() {
        if terminators.contains(&c) {
            let mut end = i + 1;
            if paragraphs {
                // Include the newline itself for paragraph ranges.
                while end < bytes.len() && bytes[end] == b'\n' {
                    end += 1;
                }
            }
            // Skip runs that contain no non-whitespace text (e.g. the space
            // between two sentences), so every range names real content.
            if end > start && text[start..end].trim().is_empty() {
                start = end;
                continue;
            }
            if end > start {
                ranges.push((start, end));
            }
            start = end;
        }
    }
    if start < bytes.len() && !text[start..].trim().is_empty() {
        ranges.push((start, bytes.len()));
    }
    format!(
        "[{}]",
        ranges
            .iter()
            .map(|(a, b)| format!("[{a},{b}]"))
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// Context window used in the JSON `context` field: the containing sentence,
/// trimmed when it is longer than `CONTEXT_MAX` characters.
fn context_window(text: &str, offset: usize, _length: usize) -> (usize, usize) {
    const CONTEXT_MAX: usize = 120;
    let bytes = text.as_bytes();
    let offset = offset.min(text.len());
    // Sentence start: after the previous terminal punctuation.
    let mut start = 0;
    for i in (0..offset).rev() {
        if matches!(bytes[i], b'.' | b'!' | b'?' | b'\n') {
            start = i + 1;
            break;
        }
    }
    while start < offset && bytes[start] == b' ' {
        start += 1;
    }
    let mut end = text.len();
    for (i, c) in text.bytes().enumerate().skip(offset) {
        if matches!(c, b'.' | b'!' | b'?' | b'\n') {
            end = i + 1;
            break;
        }
    }
    if end - start > CONTEXT_MAX {
        start = offset.saturating_sub(CONTEXT_MAX / 3);
        end = (start + CONTEXT_MAX).min(end);
        while start < offset && !text.is_char_boundary(start) {
            start += 1;
        }
        while end > offset && !text.is_char_boundary(end) {
            end -= 1;
        }
    }
    (start, end)
}

