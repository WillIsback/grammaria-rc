//! Memory/size probe used to track the cost-reduction work.
//!
//! Build and run as a normal bin: `cargo run --release --bin memprobe`.
//! Prints the RSS after each loading stage so a change can be attributed.
use std::time::Instant;

fn rss() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .map(|l| l.to_string())
        })
        .and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse::<f64>().ok()))
        .map(|kb| kb / 1024.0)
        .unwrap_or(0.0)
}

const DATA: &str = "data";

fn main() {
    println!("start            {:6.1} MB", rss());

    let t = Instant::now();
    let lex = grammaria::lexicon::Lexicon::load(std::path::Path::new(DATA).join("fr.lex")).unwrap();
    println!("lexicon          {:6.1} MB  ({:?})", rss(), t.elapsed());

    let t = Instant::now();
    let rb = std::fs::read(std::path::Path::new(DATA).join("fr.rules")).unwrap();
    let rs = grammaria::rules::RuleSet::from_cache(&rb).unwrap();
    println!(
        "rules            {:6.1} MB  ({:?})  {} rules",
        rss(),
        t.elapsed(),
        rs.len()
    );

    let t = Instant::now();
    let words = std::fs::read_to_string(std::path::Path::new(DATA).join("fr.words")).unwrap();
    let wl = grammaria::tagger::WordLists::load(&words);
    println!(
        "words            {:6.1} MB  ({:?})  {} multiwords",
        rss(),
        t.elapsed(),
        wl.multiwords.len()
    );

    let t = Instant::now();
    let xml = std::fs::read_to_string(std::path::Path::new(DATA).join("fr.disambig.xml")).unwrap();
    let d = grammaria::tagger::Disambiguation::parse(&xml);
    println!(
        "disambiguation   {:6.1} MB  ({:?})  {} rules",
        rss(),
        t.elapsed(),
        d.rules.len()
    );

    println!("TOTAL            {:6.1} MB", rss());
    drop((lex, rs, wl, d));
}
