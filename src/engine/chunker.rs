//! Multiword expressions ("capture d'écran", proper names...): tokens that
//! belong to a listed expression get a reading for the expression as a whole.
//!
//! Port of `org.languagetool.tagging.disambiguation.MultiWordChunker`
//! (LanguageTool, LGPL-2.1-or-later), including its index arithmetic.

use std::collections::HashMap;
use std::sync::Arc;

use super::strtools;
use super::token::{Reading, Readings, Tag};
use crate::re::{self, Flags};

const MAX_TOKENS_IN_MULTIWORD: usize = 20;

/// `MultiWordChunker.tagForNotAddingTags`: the entry only protects the
/// expression from the speller.
pub const TAG_FOR_NOT_ADDING_TAGS: &str = "_NONE_";

struct Entry {
    pos_tag: String,
    lemma: Arc<str>,
}

pub struct MultiWordChunker {
    start_space: HashMap<String, usize>,
    start_no_space: HashMap<char, usize>,
    full_space: HashMap<String, Entry>,
    full_no_space: HashMap<String, Entry>,
    add_ignore_spelling: bool,
    remove_previous_tags: bool,
}

pub struct ChunkerSettings<'a> {
    pub allow_first_capitalized: bool,
    pub allow_all_uppercase: bool,
    /// Every line is an expression carrying this tag (no tag column).
    pub default_tag: Option<&'a str>,
    pub ignore_spelling: bool,
    pub remove_previous_tags: bool,
}

/// `StringTools.isCamelCase`.
fn is_camel_case(token: &str) -> bool {
    let b = token.as_bytes();
    let lower = b.iter().take_while(|c| c.is_ascii_lowercase()).count();
    lower >= 1 && b.len() >= lower + 2 && b[lower].is_ascii_uppercase() && b[lower + 1..].iter().all(|c| c.is_ascii_alphabetic())
}

impl MultiWordChunker {
    /// Build the chunker from the text of a multiword file.
    pub fn new(content: &str, settings: &ChunkerSettings) -> MultiWordChunker {
        let mut c = MultiWordChunker {
            start_space: HashMap::new(),
            start_no_space: HashMap::new(),
            full_space: HashMap::new(),
            full_no_space: HashMap::new(),
            add_ignore_spelling: settings.ignore_spelling,
            remove_previous_tags: settings.remove_previous_tags,
        };
        let mut separator = re::compile("\t", Flags::default()).ok();
        for raw in content.lines() {
            let line = raw.trim();
            if let Some(sep) = line.strip_prefix("#separatorRegExp=") {
                separator = re::compile(sep, Flags::default()).ok();
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = line.split('#').next().unwrap_or("").trim();
            // String.split(regex): trailing empty parts are dropped.
            let chars: Vec<char> = line.chars().collect();
            let mut parts: Vec<String> = Vec::new();
            let mut last = 0;
            if let Some(sep) = &separator {
                for m in sep.find_all(&chars) {
                    let (a, b) = m.spans[0].unwrap();
                    if b > a {
                        parts.push(chars[last..a].iter().collect());
                        last = b;
                    }
                }
            }
            parts.push(chars[last..].iter().collect());
            while parts.len() > 1 && parts.last().is_some_and(|p| p.is_empty()) {
                parts.pop();
            }
            let (original, tag) = match settings.default_tag {
                Some(t) => {
                    if parts.len() != 1 {
                        continue;
                    }
                    (parts[0].clone(), t.to_string())
                }
                None => {
                    if parts.len() != 2 {
                        continue;
                    }
                    (parts[0].clone(), parts[1].trim().to_string())
                }
            };
            if original.is_empty() {
                continue;
            }
            let contains_space = original.find(' ').is_some_and(|p| p > 0);
            let mut variants = vec![original.clone()];
            {
                let map = if contains_space { &c.full_space } else { &c.full_no_space };
                if settings.allow_all_uppercase && !is_camel_case(&original) {
                    let upper = original.to_uppercase();
                    if !map.contains_key(&upper) && original != upper {
                        variants.push(upper);
                    }
                }
                if settings.allow_first_capitalized {
                    let cap = strtools::uppercase_first_char(&original);
                    if !map.contains_key(&cap) && original != cap {
                        variants.push(cap);
                    }
                }
            }
            for variant in variants {
                let entry = Entry { pos_tag: tag.clone(), lemma: Arc::from(original.as_str()) };
                if !contains_space {
                    let first = variant.chars().next().unwrap();
                    let len = strtools::utf16_len(&variant);
                    let slot = c.start_no_space.entry(first).or_insert(len);
                    if *slot < len {
                        *slot = len;
                    }
                    c.full_no_space.insert(variant, entry);
                } else {
                    let tokens: Vec<&str> = variant.split(' ').collect();
                    // String.split drops trailing empty strings.
                    let count = tokens.iter().rposition(|t| !t.is_empty()).map(|p| p + 1).unwrap_or(0);
                    let first = tokens[0].to_string();
                    let slot = c.start_space.entry(first).or_insert(count);
                    if *slot < count {
                        *slot = count;
                    }
                    c.full_space.insert(variant, entry);
                }
            }
        }
        c
    }

    fn add(target: &mut Readings, token: Arc<str>, tag: &str, lemma: Arc<str>) {
        target.add_reading(Reading::new(token, Some(Tag::new(tag.trim())), Some(lemma)));
    }

    fn add_boundary(target: &mut Readings, entry: &Entry, token: Arc<str>, is_last: bool) {
        let tag = if is_last { format!("</{}>", entry.pos_tag) } else { format!("<{}>", entry.pos_tag) };
        MultiWordChunker::add(target, token, &tag, entry.lemma.clone());
    }

    /// `disambiguate`: annotate the tokens of every listed expression found.
    pub fn disambiguate(&self, tokens: &mut Vec<Readings>) {
        let n = tokens.len();
        for i in 0..n {
            if tokens[i].token.is_empty() {
                continue;
            }
            let mut tok = tokens[i].token.to_string();
            let mut k = i + 1;
            while k < n && !tokens[k].is_whitespace {
                tok.push_str(&tokens[k].token);
                k += 1;
            }
            if let Some(&len) = self.start_space.get(&tok) {
                let mut final_len = 0usize;
                let mut key = String::new();
                let mut j = i;
                let mut len_counter = 0usize;
                while j < n && j - i < MAX_TOKENS_IN_MULTIWORD {
                    if !tokens[j].is_whitespace {
                        key.push_str(&tokens[j].token);
                        if let Some(at) = self.full_space.get(&key) {
                            if at.pos_tag != TAG_FOR_NOT_ADDING_TAGS {
                                if final_len == 0 {
                                    let t = tokens[j].token.clone();
                                    MultiWordChunker::add(&mut tokens[i], t, &at.pos_tag, at.lemma.clone());
                                } else {
                                    let t = tokens[i].token.clone();
                                    MultiWordChunker::add_boundary(&mut tokens[i], at, t, false);
                                    let t = tokens[final_len].token.clone();
                                    MultiWordChunker::add_boundary(&mut tokens[final_len], at, t, true);
                                }
                            }
                            if self.add_ignore_spelling {
                                if final_len == 0 {
                                    tokens[i].ignored_by_speller = true;
                                } else {
                                    for t in tokens.iter_mut().take(final_len + 1).skip(i) {
                                        t.ignored_by_speller = true;
                                    }
                                }
                            }
                        }
                    } else {
                        if j > 1 && !tokens[j - 1].is_whitespace {
                            key.push(' ');
                            len_counter += 1;
                        }
                        if len_counter == len {
                            break;
                        }
                    }
                    j += 1;
                    final_len = j;
                }
            }
            let first_char = tok.chars().next().unwrap();
            if self.start_no_space.contains_key(&first_char) {
                let mut j = i;
                let mut key = String::new();
                while j < n && !tokens[j].is_whitespace && j - i < MAX_TOKENS_IN_MULTIWORD {
                    key.push_str(&tokens[j].token);
                    if let Some(at) = self.full_no_space.get(&key) {
                        if at.pos_tag != TAG_FOR_NOT_ADDING_TAGS {
                            if i == j {
                                let low_priority = at.pos_tag == "NPCN000";
                                if !low_priority || tokens[i].readings.is_empty() || tokens[i].is_pos_tag_unknown {
                                    let t = tokens[j].token.clone();
                                    MultiWordChunker::add(&mut tokens[i], t, &at.pos_tag, at.lemma.clone());
                                }
                            } else {
                                let t = tokens[i].token.clone();
                                MultiWordChunker::add_boundary(&mut tokens[i], at, t, false);
                                let t = tokens[j].token.clone();
                                MultiWordChunker::add_boundary(&mut tokens[j], at, t, true);
                            }
                        }
                        if self.add_ignore_spelling {
                            for t in tokens.iter_mut().take(j + 1).skip(i) {
                                t.ignored_by_speller = true;
                            }
                        }
                    }
                    j += 1;
                }
            }
        }
        if self.remove_previous_tags {
            self.remove_previous(tokens);
        }
    }

    /// `removePreviousTags`: the tokens of an expression keep only the
    /// expression's tag (`<N f s>` ... `</N f s>` becomes `N f s`, then
    /// adjective tags for the following tokens).
    fn remove_previous(&self, tokens: &mut [Readings]) {
        let mut pos_tag = String::new();
        let mut lemma: Arc<str> = Arc::from("");
        let mut next_pos_tag = String::new();
        let mut i = 0;
        while i < tokens.len() {
            if !tokens[i].is_whitespace {
                if !next_pos_tag.is_empty() {
                    let new = Reading::new(tokens[i].token.clone(), Some(Tag::new(&next_pos_tag)), Some(lemma.clone()));
                    if tokens[i].has_pos_tag_and_lemma(&format!("</{pos_tag}>"), &lemma) {
                        next_pos_tag.clear();
                        lemma = Arc::from("");
                    }
                    tokens[i] = Readings::with_readings(&tokens[i], vec![new]);
                } else if let Some(at) = self.multi_word_reading(tokens, i) {
                    let at_tag = at.pos().unwrap_or("");
                    pos_tag = at_tag[1..at_tag.len() - 1].to_string();
                    lemma = at.lemma.clone().unwrap_or_else(|| Arc::from(""));
                    let closing = format!("</{pos_tag}>");
                    let opening = format!("<{pos_tag}>");
                    if tokens[i].has_pos_tag_and_lemma(&closing, &lemma) {
                        if let Some(r) = tokens[i].readings.iter().find(|r| r.pos() == Some(closing.as_str())).cloned() {
                            tokens[i].remove_reading(&r);
                        }
                        if let Some(r) = tokens[i].readings.iter().find(|r| r.pos() == Some(opening.as_str())).cloned() {
                            tokens[i].remove_reading(&r);
                        }
                        tokens[i].add_reading(Reading::new(at.token.clone(), Some(Tag::new(&pos_tag)), Some(lemma.clone())));
                        next_pos_tag.clear();
                        lemma = Arc::from("");
                    } else {
                        let new = Reading::new(at.token.clone(), Some(Tag::new(&pos_tag)), Some(lemma.clone()));
                        tokens[i] = Readings::with_readings(&tokens[i], vec![new]);
                        next_pos_tag = next_pos_tag_of(&pos_tag);
                    }
                }
            }
            i += 1;
        }
    }

    /// `getMultiWordAnalyzedToken`: the opening reading whose closing tag is
    /// the farthest away.
    fn multi_word_reading(&self, tokens: &[Readings], i: usize) -> Option<Reading> {
        let candidates: Vec<&Reading> = tokens[i]
            .readings
            .iter()
            .filter(|r| r.pos().is_some_and(|p| p.starts_with('<') && p.ends_with('>') && !p.starts_with("</")))
            .collect();
        let mut selected: Option<Reading> = None;
        let mut max_distance = 0usize;
        for at in candidates {
            let p = at.pos().unwrap();
            let closing = format!("</{}", &p[1..]);
            let Some(lemma) = at.lemma.as_deref() else { continue };
            let mut distance = 1;
            while i + distance < tokens.len() {
                if tokens[i + distance].has_pos_tag_and_lemma(&closing, lemma) {
                    if distance > max_distance {
                        max_distance = distance;
                        selected = Some(at.clone());
                    }
                    // (the "low priority tag" test of the original never
                    // holds for French tags: an equal distance replaces)
                    if distance == max_distance {
                        selected = Some(at.clone());
                    }
                    break;
                }
                distance += 1;
            }
        }
        selected
    }
}

/// `getNextPosTag`: after the head noun, the other tokens count as adjectives.
fn next_pos_tag_of(postag: &str) -> String {
    if let Some(rest) = postag.strip_prefix("NC") {
        if rest.len() >= 2 {
            return format!("AQ0{}0", &rest[..2]);
        }
    }
    if let Some(rest) = postag.strip_prefix("N ") {
        return format!("J {rest}");
    }
    postag.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sentence(words: &[&str]) -> Vec<Readings> {
        let mut out = vec![Readings::new(vec![Reading::from_strs("", Some("SENT_START"), None)], 0)];
        let mut pos = 0;
        for (i, w) in words.iter().enumerate() {
            if i > 0 {
                out.push(Readings::untagged(" ", pos));
                pos += 1;
            }
            out.push(Readings::untagged(w, pos));
            pos += w.len();
        }
        out
    }

    fn tags(t: &Readings) -> Vec<&str> {
        t.readings.iter().filter_map(|r| r.pos()).collect()
    }

    #[test]
    fn multiword_gets_the_expression_tag_and_previous_tags_are_removed() {
        let c = MultiWordChunker::new(
            "#separatorRegExp=[\\t;]\npoint presse; N m s\nhome page;N f s\n",
            &ChunkerSettings { allow_first_capitalized: true, allow_all_uppercase: true, default_tag: None, ignore_spelling: false, remove_previous_tags: true },
        );
        let mut s = sentence(&["Le", "point", "presse", "commence"]);
        c.disambiguate(&mut s);
        assert_eq!(tags(&s[3]), vec!["N m s"]);
        assert_eq!(s[3].readings[0].lemma.as_deref(), Some("point presse"));
        // The following token of the expression is tagged as an adjective.
        assert_eq!(tags(&s[5]), vec!["J m s"]);
        assert!(tags(&s[7]).is_empty());
        // Capitalised variant
        let mut s = sentence(&["Point", "presse"]);
        c.disambiguate(&mut s);
        assert_eq!(tags(&s[1]), vec!["N m s"]);
    }

    #[test]
    fn global_list_only_protects_from_the_speller() {
        let c = MultiWordChunker::new(
            "Microsoft Entra\npeer2peer\n",
            &ChunkerSettings { allow_first_capitalized: false, allow_all_uppercase: true, default_tag: Some(TAG_FOR_NOT_ADDING_TAGS), ignore_spelling: true, remove_previous_tags: false },
        );
        let mut s = sentence(&["Avec", "Microsoft", "Entra", "et", "peer2peer"]);
        c.disambiguate(&mut s);
        assert!(s[3].ignored_by_speller && s[5].ignored_by_speller);
        assert!(s[9].ignored_by_speller);
        assert!(!s[1].ignored_by_speller && !s[7].ignored_by_speller);
        assert!(tags(&s[3]).is_empty(), "no tag is added");
    }
}
