//! Grammaria: a fast French grammar and spelling checker in Rust.
//!
//! Rule files and data are taken from LanguageTool (LGPL) and are re-implemented
//! natively in Rust; see `THIRD_PARTY_LICENSES.md`.

pub mod lexicon;
pub mod pattern;
pub mod rules;
pub mod sentence;
pub mod strutil;
pub mod tagger;
pub mod tagregex;
pub mod tokenizer;
pub mod xml;
