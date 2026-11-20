//! Tests of the Morfologik reader against LanguageTool's real French
//! dictionaries (`assets/fr/lt/`) and against outputs of the Java
//! implementation (`tests/fixtures/morfologik/`, produced by
//! `tools/morfologik/Oracle.java`).
//!
//! Other oracle files can be checked with the environment variables
//! `MORFOLOGIK_LOOKUP_ORACLE`, `MORFOLOGIK_SPELLER_ORACLE`,
//! `MORFOLOGIK_SPELLER_SHARED_ORACLE` (with `MORFOLOGIK_SPELLER_DICT` and
//! `MORFOLOGIK_SPELLER_DISTANCE` for another dictionary or edit distance) and
//! `MORFOLOGIK_DUMP_ORACLE` (with `MORFOLOGIK_DUMP_DICT`); see
//! `tools/morfologik/README.md`. `cargo test --release --test morfologik --
//! --ignored --nocapture` prints the performance figures.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use grammaria::morfologik::{Automaton, Dictionary, EncoderType, Speller, WordData};

fn asset(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets/fr/lt")
        .join(name)
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/morfologik")
        .join(name)
}

fn french() -> Dictionary {
    Dictionary::load(asset("french.dict"), asset("french.info")).expect("french.dict")
}

fn synth() -> Dictionary {
    Dictionary::load_with_sibling_info(asset("french_synth.dict")).expect("french_synth.dict")
}

fn readings(dict: &Dictionary, word: &str) -> Vec<(String, String)> {
    let mut out: Vec<WordData> = Vec::new();
    dict.lookup(word, &mut out);
    out.into_iter().map(|w| (w.stem, w.tag)).collect()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[test]
fn metadata_of_the_french_dictionaries() {
    let d = french();
    let m = &d.metadata;
    assert_eq!(d.fsa.format_name(), "CFSA2");
    assert_eq!(m.separator, b'_');
    assert_eq!(m.separator_char, '_');
    assert_eq!(m.encoder_type, EncoderType::Suffix);
    assert!(m.frequency_included);
    assert!(m.run_on_words && m.ignore_punctuation && m.ignore_diacritics && m.convert_case);
    assert!(!m.ignore_all_uppercase && !m.ignore_camel_case && !m.ignore_numbers);
    assert!(m.equivalent_chars.is_empty()); // commented out in the file
    assert_eq!(m.replacement_pairs.len(), 48);
    assert_eq!(
        m.replacement_pairs[0],
        ("ent".to_string(), vec!["ement".to_string()])
    );
    let of = |k: &str| {
        m.replacement_pairs
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.join("|"))
    };
    assert_eq!(of("ss").as_deref(), Some("s|c|ç"));
    assert_eq!(of("qu").as_deref(), Some("c|k|qu'"));
    assert_eq!(of("iens").as_deref(), Some("ients"));
    assert_eq!(m.attribute("fsa.dict.speller.runon-words"), Some("true"));

    let s = synth();
    assert_eq!(s.fsa.format_name(), "CFSA2");
    assert!(!s.metadata.frequency_included);
    assert_eq!(s.metadata.encoder_type, EncoderType::Suffix);
}

#[test]
fn lookup_returns_lemma_and_tag_in_morfologik_order() {
    let d = french();
    assert_eq!(readings(&d, "maison"), pairs(&[("maison", "N f s")]));
    assert_eq!(
        readings(&d, "suis"),
        pairs(&[
            ("suivre", "V imp pres 2 s"),
            ("suivre", "V ind pres 1 s"),
            ("suivre", "V ind pres 2 s"),
            ("être", "V etre ind pres 1 s"),
        ])
    );
    assert_eq!(
        readings(&d, "Paris"),
        pairs(&[("Paris", "Z e s"), ("Paris", "Z e sp")])
    );
    assert_eq!(readings(&d, "œufs"), pairs(&[("œuf", "N m p")]));
    assert!(readings(&d, "pome").is_empty());
    assert!(readings(&d, "").is_empty());
    assert!(readings(&d, "maison_").is_empty());
    assert!(readings(&d, "mais").len() >= 2);
    assert!(d.contains("maison") && !d.contains("maiso"));

    // The raw lookup keeps the frequency byte, which `frequency` decodes.
    let mut raw = Vec::new();
    d.lookup_raw("maison", &mut raw);
    assert_eq!(raw[0].tag, "N f sO");
    assert_eq!(d.frequency("maison"), 14);
    assert_eq!(d.frequency("suis"), 11);
    assert_eq!(d.frequency("pome"), 0);

    // The output vector is reused.
    let mut out = Vec::new();
    let mut session = d.lookup_session();
    session.lookup("suis", &mut out);
    assert_eq!(out.len(), 4);
    session.lookup("maison", &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].stem, "maison");
    session.lookup("xyzzy", &mut out);
    assert!(out.is_empty());
}

#[test]
fn dictionary_can_be_shared_between_threads() {
    let d = french();
    let words = ["maison", "suis", "Paris", "était", "chevaux"];
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                let mut out = Vec::new();
                for w in words {
                    d.lookup(w, &mut out);
                    assert!(!out.is_empty(), "{w}");
                }
            });
        }
    });
}

/// Every line `form<TAB>lemma<TAB>tag` of the LanguageTool 6.6 text dump must
/// be returned by the lookup of its form. The `.dict` is a later snapshot
/// (6.9) whose data has changed a little: 142 lines are not found, namely
/// 139 country names retagged from `N . sp` to `Z . s` ("Algérie"), "Cuba"
/// (`Z e sp` → `Z m s`) and two forms that were removed ("bowwindow",
/// "quizz"). These are data differences, not decoding errors: Java's own
/// dump of the `.dict` is identical to ours (`dump_matches_java`).
#[test]
fn text_dump_is_found_by_lookup() {
    let d = french();
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("eval/sources/languagetool-pos-dict.txt");
    let text = std::fs::read_to_string(path).expect("languagetool-pos-dict.txt");
    let mut session = d.lookup_session();
    let mut out = Vec::new();
    let (mut total, mut form_missing, mut reading_missing) = (0usize, 0usize, 0usize);
    let mut examples = Vec::new();
    for line in text.lines() {
        let mut it = line.split('\t');
        let (Some(form), Some(lemma), Some(tag)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        total += 1;
        session.lookup(form, &mut out);
        if out.is_empty() {
            form_missing += 1;
            if examples.len() < 12 {
                examples.push(format!("{line}  => form not in .dict"));
            }
        } else if !out.iter().any(|w| w.stem == lemma && w.tag == tag) {
            reading_missing += 1;
            if examples.len() < 12 {
                examples.push(format!("{line}  => .dict has {out:?}"));
            }
        }
    }
    println!(
        "text dump: {total} lines, {form_missing} forms absent from the .dict, \
         {reading_missing} forms present with other readings"
    );
    for e in &examples {
        println!("  {e}");
    }
    assert_eq!(total, 634_004);
    assert_eq!(form_missing, 2);
    assert_eq!(reading_missing, 140);
}

#[test]
fn enumeration_yields_every_entry() {
    let d = french();
    let (mut n, mut with_tag) = (0usize, 0usize);
    let mut first = None;
    d.for_each_entry(|word, stem, tag| {
        if first.is_none() {
            first = Some((word.to_string(), stem.to_string(), tag.to_string()));
        }
        n += 1;
        if !tag.is_empty() {
            with_tag += 1;
        }
    });
    assert_eq!(n, 666_726);
    assert_eq!(with_tag, n);
    assert_eq!(
        first,
        Some(("!".to_string(), "!".to_string(), "M fin exclA".to_string()))
    );
    // The automaton enumerates the same number of raw sequences.
    let mut it = d.fsa.sequences();
    let mut sequences = 0usize;
    while it.next().is_some() {
        sequences += 1;
    }
    assert_eq!(sequences, n);
    assert_ne!(d.fsa.root_node(), 0);
    assert!(d.fsa.arc_count(d.fsa.root_node()) > 50);
}

/// `MORFOLOGIK_DUMP_ORACLE`: the output of `Oracle dump french.dict`
/// (`word<TAB>stem<TAB>tag` for every entry, in Java's iteration order).
#[test]
fn dump_matches_java() {
    let Ok(path) = std::env::var("MORFOLOGIK_DUMP_ORACLE") else {
        return;
    };
    let dict_path = std::env::var("MORFOLOGIK_DUMP_DICT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| asset("french.dict"));
    let d = Dictionary::load_with_sibling_info(dict_path).unwrap();
    let text = std::fs::read_to_string(path).unwrap();
    let mut expected = text.lines();
    let (mut n, mut bad) = (0usize, 0usize);
    d.for_each_entry(|word, stem, tag| {
        let line = format!("{word}\t{stem}\t{tag}");
        n += 1;
        if expected.next() != Some(line.as_str()) {
            bad += 1;
            if bad < 5 {
                println!("dump differs at entry {n}: {line}");
            }
        }
    });
    println!("dump: {n} entries compared with Java, {bad} differences");
    assert_eq!(expected.next(), None, "Java lists more entries");
    assert_eq!(bad, 0);
}

/// Lines `word<TAB>stem|rawtag<TAB>stem|rawtag...` from Java's
/// `DictionaryLookup.lookup`.
fn check_lookup_oracle(d: &Dictionary, text: &str) -> (usize, usize) {
    let mut out = Vec::new();
    let mut session = d.lookup_session();
    let (mut n, mut bad) = (0usize, 0usize);
    for line in text.lines() {
        let mut it = line.split('\t');
        let word = it.next().unwrap_or("");
        let expected: Vec<&str> = it.collect();
        session.lookup_raw(word, &mut out);
        let got: Vec<String> = out
            .iter()
            .map(|w| format!("{}|{}", w.stem, w.tag))
            .collect();
        n += 1;
        if got != expected {
            bad += 1;
            if bad < 5 {
                println!("lookup differs for {word:?}: {got:?} vs Java {expected:?}");
            }
        }
    }
    (n, bad)
}

#[test]
fn lookup_order_matches_java() {
    let d = french();
    let text = match std::env::var("MORFOLOGIK_LOOKUP_ORACLE") {
        Ok(p) => std::fs::read_to_string(p).unwrap(),
        Err(_) => std::fs::read_to_string(fixture("lookup_oracle.tsv")).unwrap(),
    };
    let (n, bad) = check_lookup_oracle(&d, &text);
    println!("lookup oracle: {n} words, {bad} differences");
    assert!(n >= 2000);
    assert_eq!(bad, 0);
}

#[test]
fn synthesis_dictionary_gives_forms_for_lemma_and_tag() {
    let s = synth();
    let forms = |key: &str| -> Vec<String> {
        let mut out = Vec::new();
        s.lookup(key, &mut out);
        // BaseSynthesizer keeps the "stem" of each reading: the inflected form.
        out.into_iter().map(|w| w.stem).collect()
    };
    assert_eq!(forms("cheval|N m p"), ["chevaux"]);
    assert_eq!(forms("cheval|N m s"), ["cheval"]);
    assert_eq!(forms("être|V etre ind pres 1 s"), ["suis"]);
    assert_eq!(forms("aller|V ind futu 3 p"), ["iront"]);
    assert_eq!(forms("beau|J m s"), ["beau", "bel"]);
    assert_eq!(forms("bigouden|N f p"), ["bigoudènes"]);
    assert!(forms("cheval|N f s").is_empty());
    assert!(forms("cheval").is_empty());

    let text = std::fs::read_to_string(fixture("synth_oracle.tsv")).unwrap();
    let (n, bad) = check_lookup_oracle(&s, &text);
    println!("synthesis oracle: {n} keys, {bad} differences");
    assert!(n >= 500);
    assert_eq!(bad, 0);
}

/// Stems encoded by Java's four `ISequenceEncoder`s must decode to the stem:
/// lines `ENCODER<TAB>word<TAB>stem<TAB>hex of the encoded stem`. LanguageTool
/// ships no dictionary with the INFIX encoder, so this is its only check.
#[test]
fn stem_decoders_match_java_encoders() {
    use grammaria::morfologik::encoders;
    let text = std::fs::read_to_string(fixture("encoders_oracle.tsv")).unwrap();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let (name, word, stem, hex) = (f[0], f[1], f[2], f[3]);
        let encoder = match name {
            "SUFFIX" => EncoderType::Suffix,
            "PREFIX" => EncoderType::Prefix,
            "INFIX" => EncoderType::Infix,
            "NONE" => EncoderType::None,
            other => panic!("unknown encoder {other}"),
        };
        let encoded: Vec<u8> = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        assert!(encoded.len() >= encoders::prefix_bytes(encoder));
        encoders::decode(encoder, word.as_bytes(), &encoded, &mut out);
        assert_eq!(
            String::from_utf8_lossy(&out),
            stem,
            "{name} decoding of {hex} for {word:?}"
        );
        *counts.entry(name).or_default() += 1;
    }
    println!("stem encoders: {counts:?} pairs decoded as in Java");
    assert_eq!(counts.len(), 4);
    assert!(counts.values().all(|&n| n >= 800));
}

// ---------------------------------------------------------------------------
// Speller
// ---------------------------------------------------------------------------

#[test]
fn speller_basics() {
    let d = french();
    let mut sp = Speller::new(&d);
    assert!(!sp.is_misspelled("maison"));
    assert!(!sp.is_misspelled("Maison")); // lowercase form known
    assert!(!sp.is_misspelled("MAISON"));
    assert!(sp.is_misspelled("mAison")); // mixed case is not converted
    assert!(!sp.is_misspelled("PARIS")); // capitalized form known
    assert!(sp.is_misspelled("paris") == !sp.is_in_dictionary("paris"));
    assert!(sp.is_misspelled("pome"));
    assert!(sp.is_misspelled("acceuil"));
    assert!(!sp.is_misspelled("")); // empty
    assert!(!sp.is_misspelled("!") && !sp.is_misspelled("…")); // punctuation ignored
    assert!(sp.is_in_dictionary("maison") && !sp.is_in_dictionary("MAISON"));
    assert_eq!(sp.frequency("maison"), 14);

    sp.reset();
    assert_eq!(sp.find_replacements("acceuil"), ["accueil"]);
    sp.reset();
    let pome = sp.find_replacements("pome");
    assert_eq!(&pome[..4], ["pomme", "paume", "paumé", "pommé"]);
    sp.reset();
    let oeufs = sp.find_replacement_candidates("oeufs");
    assert_eq!(oeufs[0].word, "œufs");
    assert_eq!((oeufs[0].orig_distance, oeufs[0].distance), (0, 14));
    sp.reset();
    assert!(sp.find_replacements("maison").is_empty()); // known word
    assert!(sp
        .find_similar_words("maison")
        .contains(&"raison".to_string()));
    assert_eq!(sp.replace_run_on_words("parmis"), ["par mis", "parmi s"]);
    assert!(sp.replace_run_on_words("maison").is_empty());
    assert_eq!(
        sp.get_all_replacements("garent", 0, 0),
        ["garent", "garement"]
    );
    assert!(sp.converts_case());
}

struct SpellerStats {
    words: usize,
    misspelled_ok: usize,
    in_dict_ok: usize,
    frequency_ok: usize,
    replacements_ok: usize,
    replacement_sets_ok: usize,
    run_on_ok: usize,
    similar_ok: usize,
    similar_checked: usize,
}

/// Lines of `Oracle speller`:
/// `word<TAB>M=0|1<TAB>D=0|1<TAB>F=n<TAB>R=w/dist|...<TAB>O=a b|...<TAB>S=w|...`
fn check_speller_oracle(d: &Dictionary, text: &str, shared: bool) -> SpellerStats {
    let mut st = SpellerStats {
        words: 0,
        misspelled_ok: 0,
        in_dict_ok: 0,
        frequency_ok: 0,
        replacements_ok: 0,
        replacement_sets_ok: 0,
        run_on_ok: 0,
        similar_ok: 0,
        similar_checked: 0,
    };
    let distance = std::env::var("MORFOLOGIK_SPELLER_DISTANCE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let mut sp = Speller::with_edit_distance(d, distance);
    let mut shown = 0;
    for line in text.lines() {
        let mut it = line.split('\t');
        let word = it.next().unwrap_or("");
        let fields: HashMap<&str, &str> = it.filter_map(|f| f.split_once('=')).collect();
        st.words += 1;
        // The Java harness creates a new Speller before each group of calls
        // (as LanguageTool does), unless it runs in "shared" mode.
        if !shared {
            sp.reset();
        }
        let m = if sp.is_misspelled(word) { "1" } else { "0" };
        let dd = if sp.is_in_dictionary(word) { "1" } else { "0" };
        let f = sp.frequency(word).to_string();
        st.misspelled_ok += usize::from(fields.get("M") == Some(&m));
        st.in_dict_ok += usize::from(fields.get("D") == Some(&dd));
        st.frequency_ok += usize::from(fields.get("F") == Some(&f.as_str()));

        if !shared {
            sp.reset();
        }
        let r: Vec<String> = sp
            .find_replacement_candidates(word)
            .iter()
            .map(|c| format!("{}/{}", c.word, c.distance))
            .collect();
        let expected_r = fields.get("R").copied().unwrap_or("");
        let got_r = r.join("|");
        if got_r == expected_r {
            st.replacements_ok += 1;
        } else if shown < 8 {
            shown += 1;
            println!("replacements differ for {word:?}:\n  rust {got_r}\n  java {expected_r}");
        }
        let mut a: Vec<&str> = got_r.split('|').collect();
        let mut b: Vec<&str> = expected_r.split('|').collect();
        a.sort_unstable();
        b.sort_unstable();
        st.replacement_sets_ok += usize::from(a == b);

        let o = sp.replace_run_on_words(word).join("|");
        st.run_on_ok += usize::from(fields.get("O") == Some(&o.as_str()));

        if let Some(expected_s) = fields.get("S") {
            if !shared {
                sp.reset();
            }
            st.similar_checked += 1;
            let s = sp.find_similar_words(word).join("|");
            if s == *expected_s {
                st.similar_ok += 1;
            } else if shown < 8 {
                shown += 1;
                println!("similar words differ for {word:?}:\n  rust {s}\n  java {expected_s}");
            }
        }
    }
    println!(
        "speller oracle ({}): {} words; is_misspelled {}, is_in_dictionary {}, frequency {}, \
         replacements (exact order and weights) {}, replacements (same set) {}, run-on {}, \
         similar words {}/{}",
        if shared {
            "one speller reused"
        } else {
            "fresh speller per word"
        },
        st.words,
        st.misspelled_ok,
        st.in_dict_ok,
        st.frequency_ok,
        st.replacements_ok,
        st.replacement_sets_ok,
        st.run_on_ok,
        st.similar_ok,
        st.similar_checked,
    );
    st
}

/// `french.dict`, or the dictionary named by `MORFOLOGIK_SPELLER_DICT` (to
/// check other dictionaries against oracle files made for them).
fn speller_dictionary() -> Dictionary {
    match std::env::var("MORFOLOGIK_SPELLER_DICT") {
        Ok(p) => Dictionary::load_with_sibling_info(p).unwrap(),
        Err(_) => french(),
    }
}

fn assert_all_agree(st: &SpellerStats) {
    assert_eq!(st.misspelled_ok, st.words, "is_misspelled");
    assert_eq!(st.in_dict_ok, st.words, "is_in_dictionary");
    assert_eq!(st.frequency_ok, st.words, "frequency");
    assert_eq!(st.replacements_ok, st.words, "find_replacement_candidates");
    assert_eq!(st.run_on_ok, st.words, "replace_run_on_words");
    assert_eq!(st.similar_ok, st.similar_checked, "find_similar_words");
}

#[test]
fn speller_matches_java() {
    let d = speller_dictionary();
    let text = match std::env::var("MORFOLOGIK_SPELLER_ORACLE") {
        Ok(p) => std::fs::read_to_string(p).unwrap(),
        Err(_) => std::fs::read_to_string(fixture("speller_oracle.tsv")).unwrap(),
    };
    let st = check_speller_oracle(&d, &text, false);
    assert!(st.words >= 500);
    assert_all_agree(&st);
}

/// The Java speller keeps its distance matrix between calls; so does the
/// port. This checks a sequence of calls on one speller.
#[test]
fn reused_speller_matches_java() {
    let d = speller_dictionary();
    let text = match std::env::var("MORFOLOGIK_SPELLER_SHARED_ORACLE") {
        Ok(p) => std::fs::read_to_string(p).unwrap(),
        Err(_) => std::fs::read_to_string(fixture("speller_oracle.tsv")).unwrap(),
    };
    let st = check_speller_oracle(&d, &text, true);
    assert!(st.words >= 100);
    assert_all_agree(&st);
}

// ---------------------------------------------------------------------------
// Performance (run with --release -- --ignored --nocapture)
// ---------------------------------------------------------------------------

fn rss_kb() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1).map(str::to_string))
        })
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// `MORFOLOGIK_CHARS_ORACLE`: the output of `Oracle chars`, one line per BMP
/// code unit: `hex <isLetter><isLowerCase><isUpperCase><isDigit><isAlphabetic>
/// <toUpperCase hex> <toLowerCase hex>`. Reports how far the `unicode` module
/// is from `java.lang.Character`.
#[test]
fn character_classes_match_java() {
    use grammaria::morfologik::unicode as u;
    let Ok(path) = std::env::var("MORFOLOGIK_CHARS_ORACLE") else {
        return;
    };
    let text = std::fs::read_to_string(path).unwrap();
    let names = [
        "isLetter",
        "isLowerCase",
        "isUpperCase",
        "isDigit",
        "isAlphabetic",
        "toUpperCase",
        "toLowerCase",
    ];
    let mut bad = [0usize; 7];
    let mut examples: Vec<Vec<String>> = vec![Vec::new(); 7];
    for line in text.lines() {
        let mut it = line.split(' ');
        let c = u16::from_str_radix(it.next().unwrap(), 16).unwrap();
        let bits = it.next().unwrap().as_bytes();
        let up = u16::from_str_radix(it.next().unwrap(), 16).unwrap();
        let low = u16::from_str_radix(it.next().unwrap(), 16).unwrap();
        let got = [
            u::is_letter(c) == (bits[0] == b'1'),
            u::is_lower_case(c) == (bits[1] == b'1'),
            u::is_upper_case(c) == (bits[2] == b'1'),
            u::is_digit(c) == (bits[3] == b'1'),
            u::is_alphabetic(c) == (bits[4] == b'1'),
            u::to_upper_case(c) == up,
            u::to_lower_case(c) == low,
        ];
        for (k, ok) in got.iter().enumerate() {
            if !ok {
                bad[k] += 1;
                if examples[k].len() < 400 {
                    examples[k].push(format!("{c:04X}"));
                }
            }
        }
    }
    for k in 0..7 {
        println!(
            "{}: {} differences {}",
            names[k],
            bad[k],
            examples[k].join(" ")
        );
    }
}

#[test]
#[ignore = "performance figures; run in release mode"]
fn bench_load_and_lookup() {
    let rss0 = rss_kb();
    let t = Instant::now();
    let d = french();
    let load = t.elapsed();
    let rss1 = rss_kb();
    let t = Instant::now();
    let s = synth();
    let load_synth = t.elapsed();
    let rss2 = rss_kb();
    println!(
        "load french.dict: {:.2} ms, +{} kB RSS (automaton {} bytes)",
        load.as_secs_f64() * 1e3,
        rss1.saturating_sub(rss0),
        d.fsa.size_in_bytes()
    );
    println!(
        "load french_synth.dict: {:.2} ms, +{} kB RSS (automaton {} bytes)",
        load_synth.as_secs_f64() * 1e3,
        rss2.saturating_sub(rss1),
        s.fsa.size_in_bytes()
    );

    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("eval/sources/languagetool-pos-dict.txt");
    let text = std::fs::read_to_string(path).unwrap();
    let mut forms: Vec<&str> = text.lines().filter_map(|l| l.split('\t').next()).collect();
    forms.dedup();
    // Shuffle deterministically so that the automaton is not walked in order.
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for i in (1..forms.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        forms.swap(i, (x % (i as u64 + 1)) as usize);
    }

    let mut session = d.lookup_session();
    let mut out = Vec::new();
    let mut found = 0usize;
    let t = Instant::now();
    for _ in 0..3 {
        for f in &forms {
            session.lookup(f, &mut out);
            found += out.len();
        }
    }
    let el = t.elapsed().as_secs_f64();
    let n = forms.len() * 3;
    println!(
        "lookup (session, strings reused): {n} lookups in {el:.3} s = {:.2} M lookups/s, {:.0} ns each ({found} readings)",
        n as f64 / el / 1e6,
        el / n as f64 * 1e9
    );

    // Running text: the words of the benchmark corpus, in reading order
    // (frequent words dominate, as in real use).
    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("eval/bench/corpus.txt");
    if let Ok(text) = std::fs::read_to_string(corpus) {
        let tokens: Vec<&str> = text
            .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '\''))
            .filter(|t| !t.is_empty())
            .collect();
        let rounds = (2_000_000 / tokens.len().max(1)).max(1);
        let mut known = 0usize;
        let t = Instant::now();
        for _ in 0..rounds {
            for tok in &tokens {
                session.lookup(tok, &mut out);
                known += usize::from(!out.is_empty());
            }
        }
        let el = t.elapsed().as_secs_f64();
        let n = tokens.len() * rounds;
        println!(
            "lookup (running text, {} tokens x {rounds}): {:.2} M lookups/s, {:.0} ns each ({:.1} % known)",
            tokens.len(),
            n as f64 / el / 1e6,
            el / n as f64 * 1e9,
            known as f64 * 100.0 / n as f64
        );
    }

    let t = Instant::now();
    let mut readings = 0usize;
    for f in &forms {
        session.for_each(f, |_, _| readings += 1);
    }
    let el = t.elapsed().as_secs_f64();
    println!(
        "lookup (for_each, no strings): {:.2} M lookups/s ({readings} readings)",
        forms.len() as f64 / el / 1e6
    );

    let t = Instant::now();
    for f in &forms {
        d.lookup(f, &mut out);
    }
    let el = t.elapsed().as_secs_f64();
    println!(
        "lookup (Dictionary::lookup, new session per call): {:.2} M lookups/s",
        forms.len() as f64 / el / 1e6
    );

    let mut sp = Speller::new(&d);
    let t = Instant::now();
    let mut miss = 0usize;
    for f in &forms {
        miss += usize::from(sp.is_misspelled(f));
    }
    let el = t.elapsed().as_secs_f64();
    println!(
        "is_misspelled: {:.2} M words/s ({miss} misspelled)",
        forms.len() as f64 / el / 1e6
    );

    let typos = [
        "pome",
        "acceuil",
        "parmis",
        "oeufs",
        "mmaison",
        "apartement",
        "biensur",
        "language",
    ];
    let t = Instant::now();
    let mut total = 0usize;
    let rounds = 20;
    for _ in 0..rounds {
        for w in typos {
            sp.reset();
            total += sp.find_replacements(w).len();
        }
    }
    let el = t.elapsed().as_secs_f64();
    println!(
        "find_replacements: {:.2} ms per word on average ({} suggestions per round)",
        el / (rounds * typos.len()) as f64 * 1e3,
        total / rounds
    );
}
