//! Reader for Morfologik binary dictionaries, as shipped by LanguageTool.
//!
//! LanguageTool stores its dictionaries as finite state automata built with
//! [Morfologik](https://github.com/morfologik/morfologik-stemming): a `.dict`
//! file (the automaton) and a `.info` file (its metadata). This module reads
//! them directly, in pure Rust and without dependencies, and behaves like the
//! Java library, results in the same order included:
//!
//! * [`fsa`] — the automata (`FSA5` and `CFSA2` formats) and their traversal;
//! * [`metadata`] — the `.info` file: separator, stem encoder, frequency
//!   flag, and the `fsa.dict.speller.*` attributes;
//! * [`dictionary`] — [`Dictionary`]: word → `(stem, tag)` lookup
//!   (`DictionaryLookup`), as LanguageTool's `MorfologikTagger` uses it, and
//!   enumeration of all entries;
//! * [`speller`] — [`Speller`]: `is_misspelled`, suggestions ordered by edit
//!   distance then frequency, run-on words (`morfologik.speller.Speller`).
//!
//! The same [`Dictionary`] type reads the tagger dictionary
//! (`french.dict`: word → lemma + POS tag + frequency) and the synthesis
//! dictionary (`french_synth.dict`: `lemma|tag` → inflected forms, returned
//! as the "stems" of the lookup).
//!
//! ```no_run
//! use grammaria::morfologik::{Dictionary, Speller, WordData};
//!
//! let dict = Dictionary::load("assets/fr/lt/french.dict", "assets/fr/lt/french.info")?;
//! let mut readings: Vec<WordData> = Vec::new();
//! dict.lookup("suis", &mut readings); // suivre/V imp pres 2 s, ..., être/V etre ind pres 1 s
//!
//! let mut speller = Speller::new(&dict);
//! assert!(speller.is_misspelled("acceuil"));
//! assert_eq!(speller.find_replacements("acceuil"), ["accueil"]);
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! A [`Dictionary`] is immutable and `Send + Sync`. Loading one is reading
//! its file (about 2 ms and 1.3 MB for `french.dict`). Code that looks up
//! many words should take a [`Lookup`] from [`Dictionary::lookup_session`]
//! and reuse it and its output vector: nothing is allocated per word then.
//!
//! The code is a port of Morfologik 2.1.9 (BSD-3-Clause, © Dawid Weiss and
//! Marcin Miłkowski); each file names the classes it comes from. It is
//! tested against the outputs of the Java library, see
//! `tools/morfologik/README.md`.

pub mod dictionary;
pub mod encoders;
pub mod fsa;
pub mod metadata;
pub mod speller;
pub mod unicode;
mod unicode_tables;

pub use dictionary::{Dictionary, Lookup, WordData};
pub use fsa::{Automaton, Fsa, MatchKind, MatchResult, SequenceIter};
pub use metadata::{DictionaryMetadata, EncoderType};
pub use speller::{CandidateData, Speller};
