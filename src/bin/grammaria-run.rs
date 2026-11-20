//! Run the new engine from plain text, one text per line (`\n`, `\t`, `\\`
//! escaped), and print one JSON line per text in the shape of the reference
//! (`tools/bench/oracle_ltjs.mjs`):
//!
//!   grammaria-run raw     TEXTS.txt   tokens straight out of the tagger
//!   grammaria-run tokens  TEXTS.txt   tokens after disambiguation
//!   grammaria-run matches TEXTS.txt   rule matches
//!
//! `--assets DIR` names the LanguageTool resource directory (default
//! `assets/fr/lt`); `--rules FILE` (repeatable) checks with these rule files
//! only; `--all-rules` also runs the rules that are off by default or
//! reserved to the picky level; `--lt-only` runs LanguageTool's rules alone,
//! without this project's rule files and profile (for comparisons with the
//! reference); `--time` prints load and run times on stderr and
//! `--slow MS` the texts that took at least MS milliseconds.

use std::path::PathBuf;

use grammaria::api::json_escape;
use grammaria::engine::dump::dump_sentences;
use grammaria::engine::pipeline::French;

fn unescape(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

fn main() {
    let mut assets = PathBuf::from("assets/fr/lt");
    let mut time = false;
    let mut slow: Option<u128> = None;
    let mut all_rules = false;
    let mut lt_only = false;
    let mut rule_files: Vec<PathBuf> = Vec::new();
    let mut positional: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--assets" => assets = PathBuf::from(args.next().unwrap_or_default()),
            "--time" => time = true,
            "--slow" => slow = args.next().and_then(|v| v.parse().ok()),
            "--all-rules" => all_rules = true,
            "--lt-only" => lt_only = true,
            "--rules" => rule_files.push(PathBuf::from(args.next().unwrap_or_default())),
            _ => positional.push(a),
        }
    }
    let (Some(mode), Some(file)) = (positional.first(), positional.get(1)) else {
        eprintln!("usage: grammaria-run (raw|tokens|matches) TEXTS.txt [--assets DIR] [--rules FILE]... [--time]");
        std::process::exit(2);
    };
    let started = std::time::Instant::now();
    let loading = if lt_only {
        grammaria::engine::pipeline::load_french_lt(&assets)
    } else if rule_files.is_empty() {
        grammaria::engine::pipeline::load_french(&assets)
    } else {
        grammaria::engine::pipeline::load_french_with_rules(&assets, &rule_files)
    };
    let french = loading.unwrap_or_else(|e| {
        eprintln!("grammaria-run: cannot load {}: {e}", assets.display());
        std::process::exit(1);
    });
    let mut french = french;
    if all_rules {
        french.checker.select(|_, _| true);
    }
    let loaded = started.elapsed();
    let loaded_rss = rss_kb();
    let input = std::fs::read_to_string(file).expect("text file");
    let texts: Vec<String> = input.lines().map(unescape).collect();
    let threads = std::env::var("GRAMMARIA_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));
    let chunk = texts.len().div_ceil(threads).max(1);
    let french: &French = &french;
    let started = std::time::Instant::now();
    let out: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = texts
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .map(|text| {
                            let started = std::time::Instant::now();
                            let out = line(french, mode, text);
                            let ms = started.elapsed().as_millis();
                            if slow.is_some_and(|limit| ms >= limit) {
                                eprintln!("slow: {ms} ms, {} words: {}", text.split_whitespace().count(), text.chars().take(100).collect::<String>().replace('\n', " "));
                            }
                            out
                        })
                        .collect::<Vec<String>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().expect("worker panicked")).collect()
    });
    let ran = started.elapsed();
    let mut buf = String::new();
    for l in &out {
        buf.push_str(l);
        buf.push('\n');
    }
    print!("{buf}");
    if time {
        let words: usize = texts.iter().map(|t| t.split_whitespace().count()).sum();
        eprintln!("memory: {} MiB after load, {} MiB at the end", loaded_rss / 1024, rss_kb() / 1024);
        eprintln!("load {loaded:?}; {} texts, {words} words in {ran:?} on {threads} thread(s): {:.0} words/s", texts.len(), words as f64 / ran.as_secs_f64());
    }
}

/// Resident memory of this process in kB (Linux; 0 elsewhere).
fn rss_kb() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok()))
        .unwrap_or(0)
}

fn line(french: &French, mode: &str, text: &str) -> String {
    match mode {
        "raw" => dump_sentences(&french.analyze(text), true),
        "tokens" => dump_sentences(&french.analyze(text), false),
        _ => {
            let parts: Vec<String> = french
                .check(text)
                .iter()
                .map(|m| {
                    let info = french.checker.info(m.rule);
                    let from = text[..m.from_byte].chars().count();
                    let len = text[m.from_byte..m.to_byte].chars().count();
                    format!(
                        "[\"{}\",{},{},[{}],\"{}\",\"{}\",\"{}\"]",
                        json_escape(&info.id),
                        from,
                        len,
                        m.suggestions.iter().take(5).map(|x| format!("\"{}\"", json_escape(x))).collect::<Vec<_>>().join(","),
                        json_escape(&info.category_id),
                        json_escape(&info.full_id()),
                        json_escape(&info.issue_type),
                    )
                })
                .collect();
            format!("[{}]", parts.join(","))
        }
    }
}
