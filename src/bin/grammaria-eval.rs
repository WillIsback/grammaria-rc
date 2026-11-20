//! Batch evaluation driver: checks one text per input line, on all cores, and
//! prints one JSON line per text. This is the engine side of the benchmark
//! (`tools/bench/`); an oracle script prints the same shape for a reference
//! checker so the two can be compared stage by stage.
//!
//!   grammaria-eval [--data DIR] [--style] [--tokens] FILE
//!
//! Input lines escape `\\`, `\n`, `\t` and `\r`. Offsets are code points.
//!
//! Default output, one array of matches per text:
//!   [["RULE_ID", offset, length, ["suggestion", ...], "CATEGORY_ID", "FULL_ID"], ...]
//! With `--tokens`, one array of sentences per text, each an array of tokens:
//!   [["token", offset, ["lemma\tPOSTAG", ...]], ...]

use std::path::PathBuf;

use grammaria::api::{json_escape, Engine};

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

fn main() {
    let mut data = PathBuf::from(std::env::var("GRAMMARIA_DATA").unwrap_or_else(|_| "data".into()));
    let mut style = false;
    let mut tokens = false;
    let mut file: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--data" => data = PathBuf::from(args.next().unwrap_or_default()),
            "--style" => style = true,
            "--tokens" => tokens = true,
            "-h" | "--help" => {
                println!("usage: grammaria-eval [--data DIR] [--style] [--tokens] FILE");
                return;
            }
            other => file = Some(other.to_string()),
        }
    }
    let Some(file) = file else {
        eprintln!("usage: grammaria-eval [--data DIR] [--style] [--tokens] FILE");
        std::process::exit(2);
    };
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
