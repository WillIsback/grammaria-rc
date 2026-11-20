//! Batch evaluation driver: checks one text per input line, on all cores, and
//! prints one JSON line per text. This is the engine side of the benchmark
//! (`tools/bench/`); an oracle script prints the same shape for a reference
//! checker so the two can be compared stage by stage.
//!
//!   grammaria-eval [--data DIR] [--style] [--tokens] FILE
//!   grammaria-eval --raw [--assets DIR] [--dict FILE] [--stats] FILE
//!
//! Input lines escape `\\`, `\n`, `\t` and `\r`. Offsets are code points.
//!
//! Default output, one array of matches per text:
//!   [["RULE_ID", offset, length, ["suggestion", ...], "CATEGORY_ID", "FULL_ID"], ...]
//! With `--tokens`, one array of sentences per text, each an array of tokens:
//!   [["token", offset, ["lemma\tPOSTAG", ...]], ...]
//! With `--raw`, the new analysis (`grammaria::engine::analysis`) straight out
//! of the tagger, before disambiguation, in the shape of the oracle's `raw`
//! mode: one array of sentences per text, each an array of tokens (the
//! `SENT_START` token and the whitespace tokens included):
//!   ["token", [["lemma", "POSTAG"], ...], flags, ["chunk tag", ...]]
//! flags: 1 immunized, 2 ignored by the speller, 4 whitespace before. This
//! mode only needs LanguageTool's resources (`--assets`, default `assets/fr`:
//! `lt/segment.srx`, `lt/added*.txt`, `lt/removed*.txt` and the dictionary
//! dump `languagetool-pos-dict.txt`, or the one `--dict` names), not the
//! compiled data directory.
//! `--stats` prints timings on stderr.

use std::path::PathBuf;

use grammaria::api::{json_escape, Engine};
use grammaria::engine::analysis::Analyzer;
use grammaria::engine::tagger::TextDictionary;

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

/// Code-point index of byte offset `b` in `text`.
fn cp(text: &str, b: usize) -> usize {
    let mut b = b.min(text.len());
    while !text.is_char_boundary(b) {
        b -= 1;
    }
    text[..b].chars().count()
}

fn matches_line(engine: &Engine, text: &str, style: bool) -> String {
    let mut out = String::from("[");
    for (i, m) in engine.check(text, style).iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let start = cp(text, m.offset);
        let end = cp(text, m.offset + m.length);
        out.push_str(&format!(
            "[\"{}\",{},{},[{}],\"{}\",\"{}\"]",
            json_escape(&m.rule_id),
            start,
            end.saturating_sub(start),
            m.suggestions
                .iter()
                .take(5)
                .map(|s| format!("\"{}\"", json_escape(s)))
                .collect::<Vec<_>>()
                .join(","),
            json_escape(&m.category_id),
            json_escape(&m.full_rule_id),
        ));
    }
    out.push(']');
    out
}

fn tokens_line(engine: &Engine, text: &str) -> String {
    let mut out = String::from("[");
    let mut first_sentence = true;
    for (a, b) in grammaria::tokenizer::sentence_boundaries(text) {
        let s = engine.tagger().tag(&text[a..b], a);
        if !first_sentence {
            out.push(',');
        }
        first_sentence = false;
        out.push('[');
        let mut first = true;
        for t in &s.tokens {
            if !t.kind.is_matchable() {
                continue;
            }
            if !first {
                out.push(',');
            }
            first = false;
            let mut readings: Vec<String> = t
                .tags
                .iter()
                .enumerate()
                .map(|(i, tag)| format!("{}\t{}", t.lemmas.get(i).map(String::as_str).unwrap_or(""), tag))
                .collect();
            readings.sort();
            readings.dedup();
            out.push_str(&format!(
                "[\"{}\",{},[{}]]",
                json_escape(&t.text),
                cp(text, a + t.start),
                readings
                    .iter()
                    .map(|r| format!("\"{}\"", json_escape(r)))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        out.push(']');
    }
    out.push(']');
    out
}

fn json_string(out: &mut String, s: Option<&str>) {
    match s {
        None => out.push_str("null"),
        Some(s) => {
            out.push('"');
            out.push_str(&json_escape(s));
            out.push('"');
        }
    }
}

/// One text in the oracle's `raw` shape. Returns the number of tokens.
fn raw_line(analyzer: &Analyzer, text: &str, out: &mut String) -> usize {
    let mut count = 0;
    out.push('[');
    for (i, sentence) in analyzer.analyze_raw(text).iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('[');
        for (j, t) in sentence.tokens.iter().enumerate() {
            count += 1;
            if j > 0 {
                out.push(',');
            }
            out.push('[');
            // The oracle prints LanguageTool's pre-disambiguation copy of the
            // tokens, whose text is that of the first reading (for a word
            // with soft hyphens: the word without them).
            json_string(out, Some(&t.readings[0].token));
            out.push_str(",[");
            for (k, r) in t.readings.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                out.push('[');
                json_string(out, r.lemma.as_deref());
                out.push(',');
                json_string(out, r.pos());
                out.push(']');
            }
            let flags = (t.immunized as u8) | (t.ignored_by_speller as u8) << 1 | (t.whitespace_before as u8) << 2;
            out.push_str(&format!("],{flags},["));
            for (k, c) in t.chunk_tags.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                json_string(out, Some(c));
            }
            out.push_str("]]");
        }
        out.push(']');
    }
    out.push(']');
    count
}

/// `--raw`: tag every text with the new analysis, on all cores.
fn run_raw(assets: &std::path::Path, dict: Option<PathBuf>, file: &str, stats: bool) {
    let started = std::time::Instant::now();
    let dict = dict.unwrap_or_else(|| assets.join("languagetool-pos-dict.txt"));
    let analyzer = TextDictionary::load(&dict)
        .and_then(|dict| Analyzer::new(std::sync::Arc::new(dict), &assets.join("lt")))
        .unwrap_or_else(|e| {
            eprintln!("grammaria-eval: cannot load the resources from {}: {e}", assets.display());
            std::process::exit(1);
        });
    let loaded = started.elapsed();
    let input = std::fs::read_to_string(file).unwrap_or_else(|e| {
        eprintln!("grammaria-eval: cannot read {file}: {e}");
        std::process::exit(1);
    });
    let texts: Vec<String> = input.lines().map(unescape).collect();
    let threads = std::env::var("GRAMMARIA_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));
    let chunk = texts.len().div_ceil(threads).max(1);
    let analyzer = &analyzer;
    let started = std::time::Instant::now();
    let parts: Vec<(String, usize)> = std::thread::scope(|scope| {
        let handles: Vec<_> = texts
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    let mut out = String::new();
                    let mut tokens = 0;
                    for text in part {
                        tokens += raw_line(analyzer, text, &mut out);
                        out.push('\n');
                    }
                    (out, tokens)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("worker panicked")).collect()
    });
    let elapsed = started.elapsed();
    let mut tokens = 0;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    for (out, n) in &parts {
        tokens += n;
        let _ = std::io::Write::write_all(&mut lock, out.as_bytes());
    }
    if stats {
        let bytes: usize = texts.iter().map(String::len).sum();
        eprintln!(
            "load {:.0} ms; {} texts, {} bytes, {} tokens in {:.1} ms on {} threads: {:.0} tokens/s, {:.2} MB/s",
            loaded.as_secs_f64() * 1e3,
            texts.len(),
            bytes,
            tokens,
            elapsed.as_secs_f64() * 1e3,
            parts.len(),
            tokens as f64 / elapsed.as_secs_f64(),
            bytes as f64 / 1e6 / elapsed.as_secs_f64(),
        );
    }
}

fn main() {
    let mut data = PathBuf::from(std::env::var("GRAMMARIA_DATA").unwrap_or_else(|_| "data".into()));
    let mut style = false;
    let mut tokens = false;
    let mut raw = false;
    let mut stats = false;
    let mut assets = PathBuf::from("assets/fr");
    let mut dict: Option<PathBuf> = None;
    let mut file: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--data" => data = PathBuf::from(args.next().unwrap_or_default()),
            "--style" => style = true,
            "--tokens" => tokens = true,
            "--raw" => raw = true,
            "--stats" => stats = true,
            "--assets" => assets = PathBuf::from(args.next().unwrap_or_default()),
            "--dict" => dict = args.next().map(PathBuf::from),
            "-h" | "--help" => {
                println!("usage: grammaria-eval [--data DIR] [--style] [--tokens] FILE");
                println!("       grammaria-eval --raw [--assets DIR] [--dict FILE] [--stats] FILE");
                return;
            }
            other => file = Some(other.to_string()),
        }
    }
    let Some(file) = file else {
        eprintln!("usage: grammaria-eval [--data DIR] [--style] [--tokens] FILE");
        std::process::exit(2);
    };
    if raw {
        run_raw(&assets, dict, &file, stats);
        return;
    }
    let engine = match Engine::load(&data) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("grammaria-eval: cannot load data from {}: {e}", data.display());
            std::process::exit(1);
        }
    };
    let input = std::fs::read_to_string(&file).unwrap_or_else(|e| {
        eprintln!("grammaria-eval: cannot read {file}: {e}");
        std::process::exit(1);
    });
    let texts: Vec<String> = input.lines().map(unescape).collect();
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let chunk = texts.len().div_ceil(threads).max(1);
    let engine = &engine;
    let lines: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = texts
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .map(|t| if tokens { tokens_line(engine, t) } else { matches_line(engine, t, style) })
                        .collect::<Vec<String>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().expect("worker panicked")).collect()
    });
    let mut out = String::with_capacity(lines.iter().map(|l| l.len() + 1).sum());
    for l in &lines {
        out.push_str(l);
        out.push('\n');
    }
    print!("{out}");
}
