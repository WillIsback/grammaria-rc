//! String helpers used across the checker.

/// Lowercase according to Unicode.
pub fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// Uppercase according to Unicode.
pub fn upper(s: &str) -> String {
    s.to_uppercase()
}

/// Uppercase the first character only.
pub fn capitalize(s: &str) -> String {
    let mut it = s.chars();
    match it.next() {
        Some(c) => c.to_uppercase().collect::<String>() + it.as_str(),
        None => String::new(),
    }
}

/// Lowercase the first character only.
pub fn decapitalize(s: &str) -> String {
    let mut it = s.chars();
    match it.next() {
        Some(c) => c.to_lowercase().collect::<String>() + it.as_str(),
        None => String::new(),
    }
}

/// Exact (case-sensitive) comparison.
pub fn eq_ignore_case_cs(a: &str, b: &str) -> bool {
    a == b
}

pub fn eq_ignore_case(a: &str, b: &str) -> bool {
    if a.chars().count() != b.chars().count() {
        return false;
    }
    a.chars()
        .zip(b.chars())
        .all(|(x, y)| x == y || x.to_lowercase().eq(y.to_lowercase()))
}

/// True if the first char is uppercase.
pub fn starts_upper(s: &str) -> bool {
    s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
}

/// True if every upper-cased letter is uppercase (digits/punct allowed).
pub fn is_all_upper(s: &str) -> bool {
    let mut any = false;
    for c in s.chars() {
        if c.is_lowercase() {
            return false;
        }
        if c.is_uppercase() {
            any = true;
        }
    }
    any
}

/// Remove accents from a string (used to bridge dictionary variants).
pub fn strip_accents(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let r = match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' | 'ì' => 'i',
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
            'û' | 'ü' | 'ù' | 'ú' => 'u',
            'ç' => 'c',
            'ÿ' | 'ý' => 'y',
            'ñ' => 'n',
            'œ' => 'o',
            'æ' => 'a',
            'À' | 'Â' | 'Ä' | 'Á' | 'Ã' | 'Å' => 'A',
            'É' | 'È' | 'Ê' | 'Ë' => 'E',
            'Î' | 'Ï' | 'Í' | 'Ì' => 'I',
            'Ô' | 'Ö' | 'Ó' | 'Ò' | 'Õ' => 'O',
            'Û' | 'Ü' | 'Ù' | 'Ú' => 'U',
            'Ç' => 'C',
            'Ÿ' | 'Ý' => 'Y',
            'Ñ' => 'N',
            other => other,
        };
        out.push(r);
    }
    out
}

/// True if the string contains at least one vowel (crude, French-oriented).
pub fn has_vowel(s: &str) -> bool {
    s.chars().any(|c| "aeiouyàâäéèêëîïôöùûüœæ".contains(c.to_ascii_lowercase()) || "aeiouyàâäéèêëîïôöùûüœæ".contains(c))
}

/// Edit distance (Levenshtein) capped at `max`; returns `max + 1` if exceeded.
pub fn levenshtein_capped(a: &[char], b: &[char], max: usize) -> usize {
    if a.len().abs_diff(b.len()) > max {
        return max + 1;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        let mut row_min = cur[0];
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            row_min = row_min.min(cur[j]);
        }
        if row_min > max {
            return max + 1;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Optimal-string-alignment distance (Levenshtein + transposition), capped;
/// returns `max + 1` when the distance exceeds `max`.
pub fn damerau_capped(a: &[char], b: &[char], max: usize) -> usize {
    if a.len().abs_diff(b.len()) > max {
        return max + 1;
    }
    let n = a.len();
    let m = b.len();
    let mut prev2: Vec<usize> = Vec::new();
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0usize; m + 1];
    for i in 1..=n {
        cur[0] = i;
        let mut row_min = cur[0];
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(prev2[j - 2] + 1);
            }
            cur[j] = v;
            row_min = row_min.min(v);
        }
        if row_min > max {
            return max + 1;
        }
        prev2 = std::mem::take(&mut prev);
        prev = std::mem::replace(&mut cur, vec![0usize; m + 1]);
    }
    prev[m]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cases() {
        assert_eq!(capitalize("élan"), "Élan");
        assert_eq!(decapitalize("Élan"), "élan");
        assert!(is_all_upper("PARIS"));
        assert!(!is_all_upper("Paris"));
        assert!(eq_ignore_case("École", "école"));
    }

    #[test]
    fn accents() {
        assert_eq!(strip_accents("château à l'île"), "chateau a l'ile");
        let a: Vec<char> = "chats".chars().collect();
        let b: Vec<char> = "chats".chars().collect();
        assert_eq!(levenshtein_capped(&a, &b, 2), 0);
        let c: Vec<char> = "chets".chars().collect();
        assert_eq!(levenshtein_capped(&a, &c, 2), 1);
        let d: Vec<char> = "ctahs".chars().collect();
        assert_eq!(damerau_capped(&a, &d, 2), 2);
    }
}
