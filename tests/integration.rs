//! End-to-end tests of the public API on the shipped assets.

use std::path::PathBuf;
use std::sync::OnceLock;

use grammaria::api::{Engine, Match, OffsetUnit};

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let dir = std::env::var("GRAMMARIA_DATA").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/fr"));
        Engine::load(&dir).expect("the shipped assets load")
    })
}

fn check(text: &str) -> Vec<Match> {
    engine().check(text, true)
}

fn found<'a>(matches: &'a [Match], rule: &str) -> Option<&'a Match> {
    matches.iter().find(|m| m.rule_id == rule)
}

#[test]
fn subject_verb_agreement_is_found_with_its_correction() {
    let text = "Les enfants mange des pommes.";
    let matches = check(text);
    let m = found(&matches, "ACCORD_SUJET_VERBE").expect("agreement error");
    assert_eq!(&text[m.offset..m.offset + m.length], "mange");
    assert_eq!(m.suggestions, ["mangent"]);
    assert_eq!(m.sentence, text);
}

#[test]
fn infinitive_after_auxiliary_is_found() {
    let matches = check("Elle a manger trois pommes.");
    let m = found(&matches, "R_VAVOIR_VINF").expect("participle expected");
    assert_eq!(m.suggestions, ["mangé"]);
}

#[test]
fn correct_text_is_clean() {
    for text in [
        "Les enfants mangent des pommes.",
        "Elle a mangé trois pommes hier soir.",
        "Nous sommes arrivés à la gare avant le départ du train.",
        "Il faut que tu viennes demain, car la réunion est importante.",
    ] {
        let matches = check(text);
        assert!(matches.is_empty(), "{text}: {:?}", matches.iter().map(|m| &m.full_rule_id).collect::<Vec<_>>());
    }
}

#[test]
fn offsets_are_bytes_and_utf16_units_on_request() {
    let text = "Été comme hiver, les enfants mange des pommes.";
    let matches = check(text);
    let m = found(&matches, "ACCORD_SUJET_VERBE").expect("agreement error");
    assert_eq!(m.offset, text.find("mange").unwrap());
    let json = engine().check_json(text, true, OffsetUnit::Utf16);
    // "Été" is 5 bytes and 3 UTF-16 units.
    assert!(json.contains(&format!("\"offset\":{}", m.offset - 2)), "{json}");
}

#[test]
fn typographic_apostrophes_do_not_shift_positions() {
    let text = "Aujourd’hui, les enfants mange des pommes.";
    let matches = check(text);
    let m = found(&matches, "ACCORD_SUJET_VERBE").expect("agreement error");
    assert_eq!(&text[m.offset..m.offset + m.length], "mange");
}

#[test]
fn analysis_tags_a_simple_sentence() {
    let sentences = engine().analyze("Le chat dort.");
    assert_eq!(sentences.len(), 1);
    let words: Vec<(&str, Vec<&str>)> = sentences[0]
        .tokens
        .iter()
        .filter(|t| !t.is_whitespace)
        .map(|t| (&*t.token, t.readings.iter().filter_map(|r| r.pos()).collect()))
        .collect();
    let tags = |word: &str| &words.iter().find(|w| w.0 == word).expect("token").1;
    assert!(tags("Le").contains(&"D m s"), "{words:?}");
    assert!(tags("chat").contains(&"N m s"), "{words:?}");
    assert!(tags("dort").contains(&"V ind pres 3 s"), "{words:?}");
}

#[test]
fn engine_serves_framed_requests_with_utf16_offsets() {
    let text = "Les enfants mange.";
    let input = format!("7 {}\n{}", text.len(), text);
    let mut out = Vec::new();
    engine().serve(input.as_bytes(), &mut out, true).unwrap();
    let out = String::from_utf8(out).unwrap();
    let mut lines = out.lines();
    assert_eq!(lines.next(), Some("{\"ready\":true}"));
    let answer = lines.next().unwrap();
    assert!(answer.starts_with("{\"id\":7,\"matches\":["), "{answer}");
    assert!(answer.contains("ACCORD_SUJET_VERBE"), "{answer}");
}

#[test]
fn engine_serve_stops_on_a_malformed_header() {
    let mut out = Vec::new();
    assert!(engine().serve("not a header\n".as_bytes(), &mut out, true).is_err());
}

#[test]
fn own_rule_files_are_loaded() {
    let matches = check("Elle semble être totalement ruiné.");
    assert!(matches.iter().any(|m| m.rule_id.starts_with("GRM_")), "{:?}", matches.iter().map(|m| &m.full_rule_id).collect::<Vec<_>>());
}
