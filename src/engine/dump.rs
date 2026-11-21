//! Reading and writing token dumps in the shape of the reference checker
//! (`tools/bench/oracle_ltjs.mjs`), so that each stage of the pipeline can be
//! fed the reference's input and compared with the reference's output.
//!
//! One text per line: an array of sentences, each an array of tokens
//! `["token", [["lemma","POSTAG"],...], flags, [chunk tags]]` where flags is
//! 1 (immunized) | 2 (ignored by the speller) | 4 (whitespace before).

use std::sync::Arc;

use super::json::{self, Value};
use super::token::{Reading, Readings, Sentence, PARAGRAPH_END};
use crate::api::json_escape;

/// Build sentences from one dump line. The `PARA_END` reading is dropped: the
/// pipeline adds it back after disambiguation, as LanguageTool does.
pub fn sentences_from_dump(line: &str) -> Vec<Sentence> {
    parse_dump(line, false)
        .into_iter()
        .map(|(text, tokens)| Sentence::new(Arc::from(text.as_str()), tokens.clone(), tokens))
        .collect()
}

/// Sentences as the rules see them: the disambiguated tokens of one dump
/// line, with the tagger's output (`raw_line`) as their pre-disambiguation
/// state, which `raw_pos` rules look at.
pub fn sentences_from_dumps(line: &str, raw_line: Option<&str>) -> Vec<Sentence> {
    let mut raw = raw_line
        .map(|l| parse_dump(l, true))
        .unwrap_or_default()
        .into_iter();
    parse_dump(line, true)
        .into_iter()
        .map(|(text, tokens)| {
            let pre = match raw.next() {
                Some((_, pre)) if pre.len() == tokens.len() => pre,
                _ => tokens.clone(),
            };
            Sentence::new(Arc::from(text.as_str()), tokens, pre)
        })
        .collect()
}

fn parse_dump(line: &str, keep_paragraph_end: bool) -> Vec<(String, Vec<Readings>)> {
    let Some(v) = json::parse(line) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for s in v.as_arr() {
        let mut tokens: Vec<Readings> = Vec::new();
        let mut text = String::new();
        let mut prev: Option<Arc<str>> = None;
        for t in s.as_arr() {
            let parts = t.as_arr();
            let token = parts.first().and_then(Value::as_str).unwrap_or("");
            let readings: Vec<Reading> = parts
                .get(1)
                .map(|r| {
                    r.as_arr()
                        .iter()
                        .map(|rd| {
                            let rd = rd.as_arr();
                            Reading::from_strs(
                                token,
                                rd.get(1).and_then(Value::as_str),
                                rd.first().and_then(Value::as_str),
                            )
                        })
                        .filter(|r| keep_paragraph_end || r.pos() != Some(PARAGRAPH_END))
                        .collect()
                })
                .unwrap_or_default();
            let readings = if readings.is_empty() {
                vec![Reading::from_strs(token, None, None)]
            } else {
                readings
            };
            let mut rs = Readings::new(readings, text.len());
            let flags = parts.get(2).map(Value::as_i64).unwrap_or(0);
            if flags & 4 != 0 {
                if let Some(p) = &prev {
                    rs.set_whitespace_before(p);
                }
            }
            rs.immunized = flags & 1 != 0;
            rs.ignored_by_speller = flags & 2 != 0;
            rs.chunk_tags = parts
                .get(3)
                .map(|c| {
                    c.as_arr()
                        .iter()
                        .filter_map(Value::as_str)
                        .map(Arc::from)
                        .collect()
                })
                .unwrap_or_default();
            text.push_str(token);
            prev = Some(rs.token.clone());
            tokens.push(rs);
        }
        out.push((text, tokens));
    }
    out
}

/// One dump line for the sentences of a text.
pub fn dump_sentences(sentences: &[Sentence], pre_disambig: bool) -> String {
    let mut out = String::from("[");
    for (si, s) in sentences.iter().enumerate() {
        if si > 0 {
            out.push(',');
        }
        out.push('[');
        let tokens = if pre_disambig {
            &s.pre_disambig
        } else {
            &s.tokens
        };
        for (ti, t) in tokens.iter().enumerate() {
            if ti > 0 {
                out.push(',');
            }
            let readings: Vec<String> = t
                .readings
                .iter()
                .map(|r| {
                    let lemma = match &r.lemma {
                        Some(l) => format!("\"{}\"", json_escape(l)),
                        None => "null".to_string(),
                    };
                    let pos = match r.pos() {
                        Some(p) => format!("\"{}\"", json_escape(p)),
                        None => "null".to_string(),
                    };
                    format!("[{lemma},{pos}]")
                })
                .collect();
            let flags = (t.immunized as u8)
                | ((t.ignored_by_speller as u8) << 1)
                | ((t.whitespace_before as u8) << 2);
            let chunks: Vec<String> = t
                .chunk_tags
                .iter()
                .map(|c| format!("\"{}\"", json_escape(c)))
                .collect();
            out.push_str(&format!(
                "[\"{}\",[{}],{},[{}]]",
                json_escape(&t.token),
                readings.join(","),
                flags,
                chunks.join(",")
            ));
        }
        out.push(']');
    }
    out.push(']');
    out
}
