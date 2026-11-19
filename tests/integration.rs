//! End-to-end tests over the shipped data set.
//!
//! These tests need the generated `data/` directory (`cargo run --release --bin
//! grammaria-build`); they are skipped automatically when it is missing so that
//! `cargo test` still works in a bare checkout.

use std::path::Path;

fn data_dir() -> Option<std::path::PathBuf> {
    let d = std::env::var("GRAMMARIA_DATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("data"));
    if d.join("fr.lex").exists() && d.join("fr.rules").exists() {
        Some(d)
    } else {
        None
    }
}

fn check(text: &str) -> Vec<grammaria::rules::RuleMatch> {
    let dir = data_dir().expect("data directory");
    let tagger = grammaria::tagger::Tagger::new(
        &dir.join("fr.lex"),
        &dir.join("fr.words"),
        Some(&dir.join("fr.disambig.xml")),
    )
    .expect("tagger");
    let rules = grammaria::rules::RuleSet::load_cache(dir.join("fr.rules")).expect("rules");
    grammaria::rules::check_text(tagger, &rules, text, false)
}

#[test]
fn determiner_agreement_is_found_with_suggestions() {
    let m = check("Cette événement est très important.");
    let hit = m
        .iter()
        .find(|x| x.rule_id == "D_N")
        .unwrap_or_else(|| panic!("D_N not reported: {m:?}"));
    assert_eq!(&"Cette événement est très important."[hit.offset..hit.offset + hit.length], "Cette événement");
    assert!(
        hit.suggestions.iter().any(|s| s.eq_ignore_ascii_case("ce événement")),
        "suggestions: {:?}",
        hit.suggestions
    );
}

#[test]
fn gender_mismatch_repairs_the_determiner() {
    let m = check("La toit fuit.");
    let hit = m.iter().find(|x| x.rule_id == "D_N").expect("D_N not reported");
    assert!(
        hit.suggestions.iter().any(|s| s.eq_ignore_ascii_case("le toit")),
        "suggestions: {:?}",
        hit.suggestions
    );
}

#[test]
fn number_mismatch_repairs_the_noun() {
    let m = check("Les voiture roulent vite.");
    let hit = m.iter().find(|x| x.rule_id == "D_N").expect("D_N not reported");
    assert!(
        hit.suggestions.iter().any(|s| s.eq_ignore_ascii_case("les voitures")),
        "suggestions: {:?}",
        hit.suggestions
    );
}

#[test]
fn correct_text_is_clean() {
    // A handful of ordinary, correct sentences must not produce matches
    // (each of these used to be a false positive during development).
    for s in [
        "Le chat mange la souris dans le jardin.",
        "Il a plu toute la nuit.",
        "Les enfants jouent dans la cour.",
        "Tout coule et rien ne demeure.",
        "Elle a pris sa décision sans réfléchir.",
        "Les résultats de l'enquête ont été publiés.",
    ] {
        let m = check(s);
        assert!(m.is_empty(), "{s:?} produced {m:?}");
    }
}

#[test]
fn tagging_matches_language_tool_for_a_simple_sentence() {
    let dir = data_dir().expect("data directory");
    let tagger = grammaria::tagger::Tagger::new(
        &dir.join("fr.lex"),
        &dir.join("fr.words"),
        Some(&dir.join("fr.disambig.xml")),
    )
    .expect("tagger");
    let s = tagger.tag("Le chat mange la souris.", 0);
    let word = |w: &str| {
        s.tokens
            .iter()
            .find(|t| t.text == w)
            .unwrap_or_else(|| panic!("token {w} missing"))
            .tags
            .clone()
    };
    assert_eq!(word("chat"), vec!["N m s"]);
    assert!(word("mange").contains(&"V ind pres 3 s".to_string()));
    assert!(word("Le").contains(&"D m s".to_string()));
}
