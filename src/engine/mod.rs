//! LanguageTool-compatible analysis and rule engine.
//!
//! The modules below follow LanguageTool's pipeline stage by stage, so that a
//! rule file written for LanguageTool behaves the same here:
//!
//! ```text
//! text ── sentence splitting ── word tokenizing ── tagging ── disambiguation
//!      ── pattern rules + native rules ── match filtering
//! ```
//!
//! Each stage is checked against a reference implementation by the benchmark
//! in `tools/bench/`.

pub mod strtools;
pub mod token;
