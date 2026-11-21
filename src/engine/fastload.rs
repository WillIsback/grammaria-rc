//! Loading a large rule file on several threads.
//!
//! A rule file is a prelude (entities, unification and phrase definitions)
//! followed by categories whose children — rules and rule groups — do not
//! depend on one another. The file is cut between such children, each piece
//! is given the prelude and its category's opening tag, and the pieces are
//! parsed in parallel; the rules come back in file order. When the file does
//! not have that shape, it is parsed in one go.

use std::sync::Arc;

use super::rule::RuleFilter;
use super::unifier::UnifierConfiguration;
use super::xmlrules::{load_grammar, GrammarRule};

/// A filter factory that threads can share.
pub type SharedFilterFactory<'a> = &'a (dyn Fn(&str) -> Option<Arc<dyn RuleFilter>> + Sync);

/// One child of a category: the category's opening tag and the child's span.
struct Unit<'a> {
    category: &'a str,
    body: &'a str,
}

/// End of the markup starting at `at` (`<...>`), honouring quoted attribute
/// values; `None` when it is not closed.
fn tag_end(bytes: &[u8], at: usize) -> Option<usize> {
    let mut i = at + 1;
    let mut quote = 0u8;
    while i < bytes.len() {
        let b = bytes[i];
        if quote != 0 {
            if b == quote {
                quote = 0;
            }
        } else if b == b'"' || b == b'\'' {
            quote = b;
        } else if b == b'>' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// The prelude (up to the first category) and the children of all the
/// categories. `None` when the structure is not the expected one.
fn split(xml: &str) -> Option<(&str, Vec<Unit<'_>>)> {
    let bytes = xml.as_bytes();
    let mut units = Vec::new();
    let mut prelude_end: Option<usize> = None;
    let mut category: Option<&str> = None;
    // Depth inside the current category, and where its current child began.
    let mut depth = 0usize;
    let mut child_start = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        if bytes[i..].starts_with(b"<!--") {
            i = find(bytes, b"-->", i + 4)? + 3;
            continue;
        }
        if bytes[i..].starts_with(b"<![CDATA[") {
            i = find(bytes, b"]]>", i + 9)? + 3;
            continue;
        }
        if bytes[i..].starts_with(b"<!DOCTYPE") {
            // The internal subset ends with "]>".
            i = find(bytes, b"]>", i)? + 2;
            continue;
        }
        if bytes[i..].starts_with(b"<?") {
            i = find(bytes, b"?>", i + 2)? + 2;
            continue;
        }
        let end = tag_end(bytes, i)?;
        let closing = bytes[i + 1] == b'/';
        let self_closing = bytes[end - 2] == b'/';
        let name_start = i + 1 + closing as usize;
        let name_len = bytes[name_start..end]
            .iter()
            .position(|b| b.is_ascii_whitespace() || *b == b'>' || *b == b'/')
            .unwrap_or(0);
        let name = &xml[name_start..name_start + name_len];
        match category {
            None => {
                if name == "category" && !closing {
                    if self_closing {
                        return None;
                    }
                    prelude_end.get_or_insert(i);
                    category = Some(&xml[i..end]);
                    depth = 0;
                } else if prelude_end.is_some() && !(closing && name == "rules") {
                    // Something else between two categories: not handled.
                    return None;
                }
            }
            Some(open) => {
                if closing {
                    if depth == 0 {
                        if name != "category" {
                            return None;
                        }
                        category = None;
                    } else {
                        depth -= 1;
                        if depth == 0 {
                            units.push(Unit {
                                category: open,
                                body: &xml[child_start..end],
                            });
                        }
                    }
                } else {
                    if depth == 0 {
                        child_start = i;
                        if self_closing {
                            units.push(Unit {
                                category: open,
                                body: &xml[i..end],
                            });
                        }
                    }
                    if !self_closing {
                        depth += 1;
                    }
                }
            }
        }
        i = end;
    }
    if category.is_some() || units.is_empty() {
        return None;
    }
    Some((&xml[..prelude_end?], units))
}

/// [`load_grammar`], on up to `threads` threads.
pub fn load_grammar_parallel(
    xml: &str,
    source_file: &str,
    is_style_file: bool,
    factory: SharedFilterFactory,
    unifier: &mut UnifierConfiguration,
    threads: usize,
) -> Vec<GrammarRule> {
    // Small files are not worth cutting.
    let split = if threads > 1 && xml.len() > 1 << 20 {
        split(xml)
    } else {
        None
    };
    let Some((prelude, units)) = split else {
        return load_grammar(
            xml,
            source_file,
            is_style_file,
            &|class| factory(class),
            unifier,
        );
    };
    // Contiguous groups of children of about the same size.
    let total: usize = units.iter().map(|u| u.body.len()).sum();
    let target = total / threads + 1;
    let mut pieces: Vec<String> = Vec::new();
    let mut piece = String::new();
    let mut size = 0;
    let mut open: Option<&str> = None;
    for unit in &units {
        if size >= target {
            piece.push_str("</category></rules>");
            pieces.push(std::mem::take(&mut piece));
            size = 0;
            open = None;
        }
        if piece.is_empty() {
            piece.push_str(prelude);
        }
        if open.map(|o| o.as_ptr()) != Some(unit.category.as_ptr()) {
            if open.is_some() {
                piece.push_str("</category>");
            }
            piece.push_str(unit.category);
            open = Some(unit.category);
        }
        piece.push_str(unit.body);
        piece.push('\n');
        size += unit.body.len();
    }
    if !piece.is_empty() {
        piece.push_str("</category></rules>");
        pieces.push(piece);
    }
    let base = unifier.clone();
    let mut results: Vec<(Vec<GrammarRule>, UnifierConfiguration)> = std::thread::scope(|scope| {
        let handles: Vec<_> = pieces
            .iter()
            .map(|piece| {
                let mut local = base.clone();
                scope.spawn(move || {
                    let rules = load_grammar(
                        piece,
                        source_file,
                        is_style_file,
                        &|class| factory(class),
                        &mut local,
                    );
                    (rules, local)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("rule loader panicked"))
            .collect()
    });
    // Every piece read the same definitions from the prelude.
    if let Some((_, first)) = results.first_mut() {
        *unifier = std::mem::take(first);
    }
    results.into_iter().flat_map(|(rules, _)| rules).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<Arc<dyn RuleFilter>> {
        None
    }

    #[test]
    fn splits_between_the_children_of_categories() {
        let xml = r#"<?xml version="1.0"?><!DOCTYPE rules [ <!ENTITY a "x>y"> ]>
<rules lang="fr"><unification feature="n"/>
<category id="A" name="a"><!-- <category> in a comment -->
  <rule id="R1"><pattern><token regexp="yes">a>b</token></pattern><message>m</message></rule>
  <rulegroup id="G"><rule><pattern><token>c</token></pattern><message>m</message></rule></rulegroup>
</category>
<category id="B" name="b"><rule id="R2"><pattern><token>d</token></pattern><message>m</message></rule></category>
</rules>"#;
        let (prelude, units) = split(xml).expect("regular structure");
        assert!(prelude.ends_with("<unification feature=\"n\"/>\n"));
        assert_eq!(units.len(), 3);
        assert!(
            units[0].body.starts_with("<rule id=\"R1\">") && units[0].body.ends_with("</rule>")
        );
        assert!(
            units[1].body.starts_with("<rulegroup id=\"G\">")
                && units[1].body.ends_with("</rulegroup>")
        );
        assert_eq!(units[2].category, "<category id=\"B\" name=\"b\">");
    }

    #[test]
    fn the_french_rule_files_load_the_same_in_parallel() {
        for (name, style) in [("grammar.xml", false), ("style.xml", true)] {
            let Ok(xml) = std::fs::read_to_string(format!("assets/fr/lt/{name}")) else {
                return;
            };
            let mut u1 = UnifierConfiguration::default();
            let mut u2 = UnifierConfiguration::default();
            let sequential = load_grammar(&xml, name, style, &none, &mut u1);
            let parallel = load_grammar_parallel(&xml, name, style, &none, &mut u2, 4);
            assert_eq!(sequential.len(), parallel.len());
            for (a, b) in sequential.iter().zip(&parallel) {
                assert_eq!(a.info().full_id(), b.info().full_id());
                assert_eq!(a.info().category_id, b.info().category_id);
                assert_eq!(a.info().tags, b.info().tags);
                assert_eq!(a.info().priority, b.info().priority);
            }
            assert_eq!(format!("{u1:?}").len(), format!("{u2:?}").len());
        }
    }
}
