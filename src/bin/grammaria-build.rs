//! Asset conversion: turns the shipped textual resources (LanguageTool French
//! POS dictionary, word lists) into the compact binary form loaded at run
//! time by the checker.
//!
//! Run with `cargo run --release --bin grammaria-build -- ...` or via the
//! `xtask`-style `tools/build_data.sh`. All inputs live in `assets/fr/` and are
//! taken verbatim from LanguageTool (LGPL, see `THIRD_PARTY_LICENSES.md`).

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use grammaria::lexicon::{Lexicon, WordTable};

pub const DEFAULT_ASSETS: &str = "assets/fr";
pub const DEFAULT_OUT: &str = "data";

fn main() -> std::io::Result<()> {
    let mut assets = DEFAULT_ASSETS.to_string();
    let mut out = DEFAULT_OUT.to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--assets" => assets = args.next().unwrap_or(assets),
            "--out" => out = args.next().unwrap_or(out),
            "-h" | "--help" => {
                println!("usage: grammaria-build [--assets DIR] [--out DIR]");
                return Ok(());
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    fs::create_dir_all(&out)?;
    let t0 = std::time::Instant::now();
    build_lexicon(&assets, &out)?;
    build_word_lists(&assets, &out)?;
    eprintln!("built assets in {:?}", t0.elapsed());
    Ok(())
}

/// Convert `languagetool-pos-dict.txt` into `fr.lex`.
///
/// Format: `word<TAB>lemma<TAB>tags`, one sense per line, 634 004 lines.
fn build_lexicon(assets: &str, out: &str) -> std::io::Result<()> {
    let path = Path::new(assets).join("languagetool-pos-dict.txt");
    let text = fs::read_to_string(&path)?;
    let mut table = WordTable::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split('\t');
        let word = match parts.next() {
            Some(w) => w,
            None => continue,
        };
        let lemma = parts.next();
        let tags = parts.next();
        match (lemma, tags) {
            (Some(l), Some(t)) => {
                if !word.is_empty() && !l.is_empty() && !t.is_empty() {
                    table.add(word, l, t);
                }
            }
            // Some entries may only carry a word + lemma without tag.
            (Some(l), None) => {
                table.add(word, l, "UNKNOWN");
            }
            _ => {}
        }
    }
    eprintln!(
        "lexicon: {} forms, {} senses from {}",
        table.len(),
        table.sense_count_hint(),
        path.display()
    );
    let lx = Lexicon::from_table(&table);
    let dest = Path::new(out).join("fr.lex");
    lx.save(&dest)?;
    eprintln!("wrote {} ({} bytes)", dest.display(), fs::metadata(&dest)?.len());
    Ok(())
}

/// Convert the auxiliary word lists (multiwords, hyphenated words, spelling
/// additions, forbidden words) into a single compact file.
///
/// Line-oriented sections separated by a header line starting with `#!`:
///   `#!multiwords`   `form; tags`
///   `#!hyphenated`   one hyphenated word per line
///   `#!spelling`     words accepted by the spell checker
///   `#!forbidden`    words always flagged as misspelled
fn build_word_lists(assets: &str, out: &str) -> std::io::Result<()> {
    let mut buf = String::new();
    let mut push_section = |buf: &mut String, name: &str| {
        buf.push_str("#!");
        buf.push_str(name);
        buf.push('\n');
    };

    // multiwords
    let multi = Path::new(assets).join("languagetool-multiwords.txt");
    push_section(&mut buf, "multiwords");
    if let Ok(text) = fs::read_to_string(&multi) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            buf.push_str(line);
            buf.push('\n');
        }
    }

    // hyphenated words
    push_section(&mut buf, "hyphenated");
    let hy = Path::new(assets).join("languagetool-hyphenated_words.txt");
    if let Ok(text) = fs::read_to_string(&hy) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            buf.push_str(line);
            buf.push('\n');
        }
    }

    // extra spelling words (merged: spelling.txt, ignore.txt, common_words.txt,
    // compounds.txt)
    push_section(&mut buf, "spelling");
    let mut seen: HashSet<String> = HashSet::new();
    for f in ["spelling.txt", "ignore.txt", "common_words.txt", "compounds.txt"] {
        let p = Path::new(assets).join(f);
        if let Ok(text) = fs::read_to_string(&p) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let w = line.split('\t').next().unwrap_or(line).trim();
                if !w.is_empty() && seen.insert(w.to_string()) {
                    buf.push_str(w);
                    buf.push('\n');
                }
            }
        }
    }

    push_section(&mut buf, "forbidden");
    let p = Path::new(assets).join("prohibit.txt");
    if let Ok(text) = fs::read_to_string(&p) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            buf.push_str(line);
            buf.push('\n');
        }
    }

    let dest = Path::new(out).join("fr.words");
    fs::write(&dest, buf)?;
    eprintln!("wrote {}", dest.display());
    Ok(())
}
