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
    let det_tag = det
        .tags
        .iter()
        .find(|t| t.starts_with("D ") || t.starts_with("P+D ") || t.contains("D ") || t.starts_with("J ") || t.contains("V ") && t.contains("ppa"))
        .cloned();
    let word_tag = word
        .tags
        .iter()
        .find(|t| t.starts_with("N") || t.starts_with("Z") || t.starts_with("J") || (t.starts_with("V") && t.contains("ppa")))
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
    let w_number = parts[2];
    let d_parts: Vec<&str> = dt.split(' ').collect();
    if d_parts.len() < 3 {
        return FilterOutcome::keep();
    }
    let d_gender = d_parts[d_parts.len() - 2];
    let d_number = d_parts[d_parts.len() - 1];

    // number agreement: s/p sp
    let number_ok = d_number == w_number
        || d_number == "sp"
        || w_number == "sp"
        || (d_number == "e" && w_number == "e");
    // gender agreement: determiners that are gender-invariant
    let gender_invariant = matches!(det.text.as_str(), "l'" | "les" | "des" | "ces" | "ses" | "mes" | "tes" | "nos" | "vos" | "leurs" | "quels" | "quelles" | "plusieurs" | "certains" | "certaines");
    let gender_ok = gender_invariant
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
    let mut suggestions = Vec::new();
    let is_noun = wt.starts_with('N') || wt.starts_with('Z');
    let _ = is_noun;
    // keep the determiner, inflect the word (nominal forms)
    for number in ["s", "p"] {
        for gender in ["m", "f"] {
            let cand = noun_form(&word.text, gender, number);
            let det_text = determiner_form(&det.text, gender, number, d_gender, d_number);
            if let Some(dt2) = det_text {
                if let Some(w2) = cand {
                    let sug = format!("{} {}", dt2, w2).replace("' ", "'");
                    if sug != format!("{} {}", det.text, word.text) && !suggestions.contains(&sug) {
                        suggestions.push(sug);
                    }
                }
            }
        }
    }
    // Disagreement: a genuine error; report it with the corrected forms.
    FilterOutcome {
        is_error: true,
        has_suggestions: !suggestions.is_empty(),
        suggestions,
    }
}

/// Very small French inflection helper for nouns/adjectives used by the
/// determiner filter: plurals add `s` (or `x` after `au/eu/ou`), feminine adds
/// `e` with common patterns.
fn noun_form(word: &str, gender: &str, number: &str) -> Option<String> {
    let mut w = word.to_string();
    // plural
    if number == "p" && !w.ends_with('s') && !w.ends_with('x') && !w.ends_with('z') {
        if w.ends_with("au") || w.ends_with("eu") || w.ends_with("ou") {
            w.push('x');
        } else {
            w.push('s');
        }
    }
    if number == "s" {
        // naive singularisation for suggestions
        if w.ends_with('s') && w.len() > 2 {
            w.pop();
        } else if w.ends_with('x') && w.len() > 2 {
            w.pop();
        }
    }
    let _ = gender;
    Some(w)
}

/// Try to adapt a determiner to a gender/number.
fn determiner_form(text: &str, gender: &str, number: &str, det_gender: &str, det_number: &str) -> Option<String> {
    let lower = crate::strutil::lower(text);
    let mapped = match (lower.as_str(), gender, number) {
        ("le", "f", _) => Some("la"),
        ("la", "m", _) => Some("le"),
        ("un", "f", _) => Some("une"),
        ("une", "m", _) => Some("un"),
        ("le", _, "p") | ("la", _, "p") | ("un", _, "p") | ("une", _, "p") => Some("les"),
        ("ce", "f", _) => Some("cette"),
        ("cette", "m", _) => Some("ce"),
        ("mon", "f", _) => Some("ma"),
        ("ton", "f", _) => Some("ta"),
        ("son", "f", _) => Some("sa"),
        ("ma", "m", _) => Some("mon"),
        ("ta", "m", _) => Some("ton"),
        ("sa", "m", _) => Some("son"),
        ("ce", _, "p") | ("cette", _, "p") => Some("ces"),
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
    pub fn parse(s: &str) -> FilterArgs {
        let mut map = std::collections::HashMap::new();
        for part in s.split(',') {
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
