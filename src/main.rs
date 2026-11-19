//! Grammaria command line interface.

use std::io::Read;

fn usage() {
    eprintln!(
        "grammaria {}\n\
         A fast French grammar checker.\n\n\
         USAGE:\n  grammaria check [OPTIONS] FILE...\n\n\
         OPTIONS:\n\
         \x20 --json             machine-readable output (LanguageTool-compatible)\n\
         \x20 --tagger           print POS analysis (LanguageTool --verbose format)\n\
         \x20 --data DIR         data directory (default: ./data, $GRAMMARIA_DATA)\n\
         \x20 -                 read from stdin\n",
        env!("CARGO_PKG_VERSION")
    );
}

fn data_dir() -> std::path::PathBuf {
    if let Ok(d) = std::env::var("GRAMMARIA_DATA") {
        return d.into();
    }
    std::path::PathBuf::from("data")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        usage();
        return;
    }
    let mut json = false;
    let mut tagger = false;
    let mut files: Vec<String> = Vec::new();
    let mut data: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--tagger" => tagger = true,
            "--data" => {
                i += 1;
                data = args.get(i).cloned();
            }
            f => files.push(f.to_string()),
        }
        i += 1;
    }
    let dir = data.map(std::path::PathBuf::from).unwrap_or_else(data_dir);
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
    let _ = (json, tagger);
    for f in &files {
        let mut text = String::new();
        if f == "-" {
            std::io::stdin().read_to_string(&mut text).ok();
        } else {
            text = std::fs::read_to_string(f).unwrap_or_default();
        }
        for (a, b) in grammaria::tokenizer::sentence_boundaries(&text) {
            let s = tg.tag(&text[a..b], a);
            println!("{}", tg.verbose(&s));
        }
    }
}
