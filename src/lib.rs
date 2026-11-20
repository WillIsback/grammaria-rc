#![allow(private_interfaces)]
//! Grammaria: a fast French grammar and spelling checker in Rust.
//!
//! Rule files and data are taken from LanguageTool (LGPL) and are re-implemented
//! natively in Rust; see `THIRD_PARTY_LICENSES.md`.
//!
//! `private_interfaces` is allowed because the public API deliberately exposes
//! a few internals (`RegexCache::get`, `ValueMatcher::Regex`) for diagnostics
//! and tests; exposing the `Regex` type itself would enlarge the public surface
//! for no benefit.

pub mod api;
pub mod engine;
pub mod filters;
pub mod lexicon;
pub mod morfologik;
pub mod pattern;
pub mod re;
pub mod rulecache;
pub mod rules;
pub mod sentence;
pub mod strutil;
pub mod sym;
pub mod tagger;
pub mod tagregex;
pub mod tokenizer;
pub mod xml;
