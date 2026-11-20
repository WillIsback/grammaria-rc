//! Grammaria: a fast French grammar and spelling checker in Rust.
//!
//! The engine runs LanguageTool's French rule files and dictionaries
//! unchanged (see `engine` for the pipeline, a port of LanguageTool's, and
//! `morfologik` for the dictionary reader), plus this project's own rules.
//! `api` is what a host application uses; see `THIRD_PARTY_LICENSES.md` for
//! the origin of the data and of the ported logic.

pub mod api;
pub mod engine;
pub mod morfologik;
pub mod re;
pub mod xml;
