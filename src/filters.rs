//! Native implementations of the LanguageTool `RuleFilter` classes that the
//! French rules need for correctness.
//!
//! LanguageTool implements these as Java plugins (e.g.
//! `WordWithDeterminerFilter`). Re-implementing them in Rust is what turns a
//! pattern matcher into a grammar checker: the filter inspects the analysed
//! sentence around a match, decides whether it is a real error and produces
//! corrected suggestions.

use crate::pattern::{Match, PatternElem, Pattern};
use crate::sentence::Sentence;

/// Result of a filter.
///
/// LanguageTool filter classes return a `RuleMatch` when the match is a
/// genuine error and `null` when it must be discarded. We mirror that:
/// `is_error == true` keeps the match and `is_error == false` discards it.
/// Some filters (e.g. `WordWithDeterminerFilter`) also *generate* the
/// suggestions, replacing the static ones from the rule.
pub struct FilterOutcome {
    pub is_error: bool,
    pub suggestions: Vec<String>,
    pub has_suggestions: bool,
}

impl FilterOutcome {
    /// The filter has no opinion: keep the match as found.
    pub fn keep() -> Self {
        FilterOutcome {
            is_error: true,
            suggestions: Vec::new(),
            has_suggestions: false,
        }
    }

    pub fn reject() -> Self {
        FilterOutcome {
            is_error: false,
            suggestions: Vec::new(),
            has_suggestions: false,
        }
    }
}

/// `org.languagetool.rules.fr.WordWithDeterminerFilter`
///
/// Validates a determiner + noun/adjective pair and suggests the correctly
/// agreed forms. Arguments: `wordFrom` and `determinerFrom` (1-based element
/// positions in the pattern).
pub fn word_with_determiner(s: &Sentence, m: &Match, args: &FilterArgs) -> FilterOutcome {
    let pos_word = args.get_int("wordFrom").unwrap_or(2);
    let pos_det = args.get_int("determinerFrom").unwrap_or(1);
    let det_idx = m.per_elem.get(pos_det.saturating_sub(1)).copied().unwrap_or(m.token_start);
    let word_idx = m.per_elem.get(pos_word.saturating_sub(1)).copied().unwrap_or(m.token_end.saturating_sub(1));
    if det_idx >= s.tokens.len() || word_idx >= s.tokens.len() {
        return FilterOutcome::keep();
    }
    let det = &s.tokens[det_idx];
    let word = &s.tokens[word_idx];

    // determiners: (P.)?D .* | J .* | V.* ppa .*
    // The determiner reading must be a determiner/adjective/past-participle
    // tag (LanguageTool's `(P.)?D .*|J .*|V.* ppa .*`).
    let det_tag = det
        .tags
        .iter()
        .find(|t| {
            !t.starts_with("_GN_")
                && (t.starts_with("D ")
                    || t.starts_with("P+D ")
                    || t.starts_with("PD ")
                    || t.starts_with("J ")
                    || (t.starts_with("V") && t.contains("ppa")))
        })
        .cloned();
    let word_tag = word
        .tags
        .iter()
        .find(|t| {
            !t.starts_with("_GN_")
                && (t.starts_with("N")
                    || t.starts_with("Z")
                    || t.starts_with("J")
                    || (t.starts_with("V") && t.contains("ppa")))
        })
        .cloned();
    let (Some(dt), Some(wt)) = (det_tag, word_tag) else {
        return FilterOutcome::keep();
    };
    let _ = &dt;
    // gender/number of the word
    let parts: Vec<&str> = wt.split(' ').collect();
    if parts.len() < 3 {
        return FilterOutcome::keep();
    }
    let w_gender = parts[1];
    let w_number = parts[2].trim_end_matches(')');
    let d_parts: Vec<&str> = dt.split(' ').collect();
    if d_parts.len() < 3 {
        return FilterOutcome::keep();
    }
    let d_gender = d_parts[d_parts.len() - 2];
    let d_number = d_parts[d_parts.len() - 1].trim_end_matches(')');

    // Number agreement: `sp` (singular *or* plural) is compatible with both.
    let number_ok = d_number == w_number || d_number == "sp" || w_number == "sp";
    if std::env::var("GRM_FILTER_DEBUG").is_ok() {
        eprintln!("det='{}' tag='{}' (g={} n={}) | word='{}' tag='{}' (g={} n={}) -> num_ok={}", det.text, dt, d_gender, d_number, word.text, wt, w_gender, w_number, number_ok);
    }
    // Gender-invariant determiners: they only need to agree in number.
    // (`aucun(e)`, `tout(e)`, `ce/cette` DO inflect for gender.)
    let det_lower = crate::strutil::lower(&det.text);
    let gender_invariant = matches!(
        det_lower.as_str(),
        "l'" | "les" | "des" | "ces" | "ses" | "mes" | "tes" | "nos" | "vos" | "leurs" | "quels"
            | "quelles" | "plusieurs" | "certains" | "certaines" | "aux" | "du" | "de la"
            | "quelques" | "divers" | "diverses" | "de" | "d'"
    );
    // Determiners whose surface form encodes gender: disagreement is an error.
    let encodes_gender = matches!(
        det_lower.as_str(),
        "un" | "une" | "le" | "la" | "ce" | "cette" | "mon" | "ma" | "ton" | "ta" | "son" | "sa"
            | "aucun" | "aucune" | "tout" | "toute" | "tous" | "toutes" | "chaque" | "quel" | "quelle"
            | "notre" | "votre" | "leur" | "cet" | "nul" | "nulle" | "tel" | "telle" | "maints"
            | "maintes" | "certain" | "certaine"
    );
    // `tout/toute/tous/toutes` and `chaque` are quantifiers: for these, gender
    // agreement with an epicene or the other-number noun is not checkable here.
    let quantifier = matches!(det_lower.as_str(), "chaque" | "tous" | "toutes" | "notre" | "votre" | "leur");
    let gender_ok = !encodes_gender
        || gender_invariant
        || quantifier
        || d_gender == w_gender
        || d_gender == "e"
        || w_gender == "e"
        || d_gender == "sp";
    if number_ok && gender_ok {
        // The pair agrees: this is not an error, discard the match.
        return FilterOutcome::reject();
    }
    // The pair does not agree: this is a real error. Suggest the correct forms
    // by trying the noun's gender/number with the determiner and vice versa.
    // Suggest the two classic repairs, in order of preference:
    //   1. adapt the noun to the determiner ("Les voiture" -> "Les voitures"),
    //   2. adapt the determiner to the noun ("La toit" -> "Le toit").
    let mut suggestions = Vec::new();
    let original = format!("{} {}", det.text, word.text);
    // 1. noun follows the determiner
    if d_number != "sp" && (d_gender != "e" || !encodes_gender) {
        let number = d_number;
        let gender = if encodes_gender && d_gender != "e" { d_gender } else { w_gender };
        let cand = noun_form(&word.text, gender, number, w_gender);
        if let Some(w2) = cand {
            if let Some(dt2) = determiner_form(&det.text, gender, number, d_gender, d_number) {
                let sug = format!("{} {}", dt2, w2).replace("' ", "'");
                if sug != original && !suggestions.contains(&sug) {
                    suggestions.push(sug);
                }
            } else {
                let sug = format!("{} {}", det.text, w2).replace("' ", "'");
                if sug != original && !suggestions.contains(&sug) {
                    suggestions.push(sug);
                }
            }
        }
    }
    // 2. determiner follows the noun
    let cand = noun_form(&word.text, w_gender, w_number, w_gender);
    if let Some(w2) = cand {
        if let Some(dt2) = determiner_form(&det.text, w_gender, w_number, d_gender, d_number) {
            let sug = format!("{} {}", dt2, w2).replace("' ", "'");
            if sug != original && !suggestions.contains(&sug) {
                suggestions.push(sug);
            }
        }
    }
    // Keep only suggestions whose noun is a real dictionary word, and prefer
    // those; this removes synthetic forms such as "toite" or "bell".
    if let Some(lx) = crate::lexicon::global_lexicon() {
        let valid: Vec<String> = suggestions
            .iter()
            .filter(|sug| {
                sug.split_whitespace()
                    .last()
                    .map(|w| lx.contains(w) || lx.contains(&crate::strutil::lower(w)))
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        if !valid.is_empty() {
            suggestions = valid;
        }
    }
    // Disagreement between determiner and noun. LanguageTool only reports the
    // match when it can synthesise a corrected form; do the same (a match with
    // no suggestion is usually a tagging artefact, e.g. "Tout coule").
    if suggestions.is_empty() {
        return FilterOutcome::reject();
    }
    suggestions.truncate(4);
    FilterOutcome {
        is_error: true,
        has_suggestions: true,
        suggestions,
    }
}

/// Small French inflection helper for nouns/adjectives used by the
/// determiner filter: plural (`-s`, `-x` after `au/eu/ou`) and feminine
/// (`-e`, with the common `-eur/-euse`, `-teur/-trice`, `-er/-ère` patterns).
fn noun_form(word: &str, gender: &str, number: &str, original_gender: &str) -> Option<String> {
    let mut w = word.to_string();
    // feminine form when the requested gender differs from the noun's own
    if gender == "f" && original_gender == "m" {
        w = feminize(&w);
    } else if gender == "m" && original_gender == "f" {
        w = masculinize(&w);
    }
    // plural
    if number == "p" && !w.ends_with('s') && !w.ends_with('x') && !w.ends_with('z') {
        if w.ends_with("au") || w.ends_with("eu") || w.ends_with("ou") {
            w.push('x');
        } else if w.ends_with("al") {
            w.truncate(w.len() - 2);
            w.push_str("aux");
        } else {
            w.push('s');
        }
    }
    if number == "s" {
        if w.ends_with("aux") {
            w.truncate(w.len() - 3);
            w.push_str("al");
        } else if w.ends_with("eaux") {
            w.truncate(w.len() - 4);
            w.push_str("eau");
        } else if (w.ends_with('s') || w.ends_with('x')) && w.chars().count() > 2 {
            w.pop();
        }
    }
    Some(w)
}

/// Feminine of a masculine noun (common regular patterns).
fn feminize(w: &str) -> String {
    let special: &[(&str, &str)] = &[
        ("homme", "femme"),
        ("père", "mère"),
        ("frère", "sœur"),
        ("roi", "reine"),
        ("dieu", "déesse"),
        ("neveu", "nièce"),
        ("oncle", "tante"),
        ("cheval", "jument"),
        ("taureau", "vache"),
        ("coq", "poule"),
        ("fils", "fille"),
        ("garçon", "fille"),
    ];
    for (m, f) in special {
        if w == *m {
            return f.to_string();
        }
    }
    if w.ends_with("eur") {
        format!("{}euse", &w[..w.len() - 3])
    } else if w.ends_with("teur") {
        format!("{}trice", &w[..w.len() - 4])
    } else if w.ends_with("er") {
        format!("{}ère", &w[..w.len() - 2])
    } else if w.ends_with("eau") {
        format!("{}elle", &w[..w.len() - 3])
    } else if w.ends_with("el") || w.ends_with("en") || w.ends_with("on") {
        // bon -> bonne, petit -> petite
        format!("{}{}e", w, w.chars().last().unwrap())
    } else if w.ends_with('e') || w.ends_with('é') {
        w.to_string()
    } else {
        format!("{w}e")
    }
}

/// Masculine of a feminine noun (best effort).
fn masculinize(w: &str) -> String {
    let special: &[(&str, &str)] = &[
        ("femme", "homme"),
        ("mère", "père"),
        ("sœur", "frère"),
        ("reine", "roi"),
        ("nièce", "neveu"),
        ("tante", "oncle"),
        ("jument", "cheval"),
        ("vache", "taureau"),
        ("poule", "coq"),
        ("fille", "fils"),
    ];
    for (f, m) in special {
        if w == *f {
            return m.to_string();
        }
    }
    if w.ends_with("euse") {
        format!("{}eur", &w[..w.len() - 4])
    } else if w.ends_with("trice") {
        format!("{}teur", &w[..w.len() - 5])
    } else if w.ends_with("ère") {
        format!("{}er", &w[..w.len() - 3])
    } else if w.ends_with('e') {
        w[..w.len() - 1].to_string()
    } else {
        w.to_string()
    }
}

/// Try to adapt a determiner to a gender/number.
fn determiner_form(text: &str, gender: &str, number: &str, det_gender: &str, det_number: &str) -> Option<String> {
    let lower = crate::strutil::lower(text);
    let mapped = match (lower.as_str(), gender, number) {
        ("le", "f", _) => Some("la"),
        ("la", "m", _) => Some("le"),
        ("un", "f", _) => Some("une"),
        ("une", "m", _) => Some("un"),
        ("le", _, "p") | ("la", _, "p") | ("un", _, "p") | ("une", _, "p") | ("au", _, "p") | ("du", _, "p") => Some("les"),
        ("ce", "f", _) => Some("cette"),
        ("cette", "m", _) => Some("ce"),
        ("mon", "f", _) => Some("ma"),
        ("ton", "f", _) => Some("ta"),
        ("son", "f", _) => Some("sa"),
        ("ma", "m", _) => Some("mon"),
        ("ta", "m", _) => Some("ton"),
        ("sa", "m", _) => Some("son"),
        ("ce", _, "p") | ("cette", _, "p") => Some("ces"),
        ("mes", "m", "s") => Some("mon"),
        ("mes", "f", "s") => Some("ma"),
        ("tes", "m", "s") => Some("ton"),
        ("tes", "f", "s") => Some("ta"),
        ("ses", "m", "s") => Some("son"),
        ("ses", "f", "s") => Some("sa"),
        ("aux", "m", "s") => Some("au"),
        ("aux", "f", "s") => Some("à la"),
        ("au", "f", _) => Some("à la"),
        ("du", "f", _) => Some("de la"),
        ("aucune", "m", _) => Some("aucun"),
        ("aucun", "f", _) => Some("aucune"),
        ("toute", "m", _) => Some("tout"),
        ("tout", "f", _) => Some("toute"),
        _ => {
            if det_gender == gender && det_number == number {
                None
            } else {
                None
            }
        }
    };
    mapped.map(|m| {
        if crate::strutil::starts_upper(text) {
            crate::strutil::capitalize(m)
        } else {
            m.to_string()
        }
    })
}

/// Parsed `args="k:v,k2:v2"` filter arguments.
#[derive(Default, Debug, Clone)]
pub struct FilterArgs {
    map: std::collections::HashMap<String, String>,
}

impl FilterArgs {
    /// Parse `args="k:v,k2:v2"` or `args="k:v k2:v2"` (both separators occur
    /// in the French rule files).
    pub fn parse(s: &str) -> FilterArgs {
        let mut map = std::collections::HashMap::new();
        for part in s.split(|c: char| c == ',' || c.is_whitespace()) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some((k, v)) = part.split_once(':') {
                map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        FilterArgs { map }
    }

    pub fn get_int(&self, k: &str) -> Option<usize> {
        self.map.get(k).and_then(|v| v.parse().ok())
    }

    pub fn get(&self, k: &str) -> Option<&str> {
        self.map.get(k).map(|s| s.as_str())
    }

    /// Re-serialize to the `k:v,k2:v2` form used in the XML.
    pub fn to_args_string(&self) -> String {
        let mut keys: Vec<&String> = self.map.keys().collect();
        keys.sort();
        keys.iter()
            .map(|k| format!("{}:{}", k, self.map[*k]))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// A filter attached to a rule.
#[derive(Clone, Debug)]
pub struct RuleFilter {
    pub class: String,
    pub args: FilterArgs,
}

impl RuleFilter {
    /// Apply the filter to a match.
    pub fn apply(&self, s: &Sentence, m: &Match) -> FilterOutcome {
        let short = self.class.rsplit('.').next().unwrap_or(&self.class);
        match short {
            "WordWithDeterminerFilter" => word_with_determiner(s, m, &self.args),
            // Filters that only de-duplicate or suppress suggestions leave the
            // match itself untouched.
            _ => FilterOutcome::keep(),
        }
    }
}

/// Find the token index for a pattern element (helper for filters).
pub fn elem_index(pat: &Pattern, m: &Match, no: usize) -> Option<usize> {
    let n = pat.elems.iter().take(no).filter(|e| matches!(e, PatternElem::Token(_))).count();
    m.per_elem.get(n.saturating_sub(1)).copied()
}
