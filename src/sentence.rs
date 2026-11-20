//! Analysed sentences: the tokens produced by the tagger plus their POS tags.

use crate::tokenizer::{Token, TokenType};

/// A word with its lemma and tag list.
#[derive(Clone, Debug)]
pub struct AnalyzedToken {
    pub text: String,
    /// Byte range relative to the sentence.
    pub start: usize,
    pub end: usize,
    pub kind: TokenType,
    /// POS tags, in the LanguageTool format (`N m s`, `V ind pres 3 s`, ...).
    pub tags: Vec<String>,
    /// Lemmas parallel to `tags` (same index).
    pub lemmas: Vec<String>,
    /// Spacing before the token (used by `spacebefore`).
    pub spaces_before: u32,
    /// True for multiword chunks like `c'est` (chunked by the chunker).
    pub chunked: bool,
}

impl AnalyzedToken {
    pub fn new_for_test(text: &str) -> AnalyzedToken {
        AnalyzedToken {
            text: text.to_string(),
            start: 0,
            end: text.len(),
            kind: TokenType::Word,
            tags: Vec::new(),
            lemmas: Vec::new(),
            spaces_before: 1,
            chunked: false,
        }
    }

    pub fn from_token(t: &Token) -> AnalyzedToken {
        AnalyzedToken {
            text: t.text.clone(),
            start: t.start,
            end: t.end,
            kind: t.kind,
            tags: Vec::new(),
            lemmas: Vec::new(),
            spaces_before: t.spaces_before,
            chunked: false,
        }
    }

    /// All tags joined (used for debugging and the tagger output format).
    pub fn tag_string(&self) -> String {
        self.tags.join(",")
    }

    pub fn add(&mut self, lemma: &str, tag: &str) {
        if let Some(p) = self.tags.iter().position(|t| t == tag) {
            if !self.lemmas[p].split('|').any(|l| l == lemma) {
                self.lemmas[p].push('|');
                self.lemmas[p].push_str(lemma);
            }
            return;
        }
        self.tags.push(tag.to_string());
        self.lemmas.push(lemma.to_string());
    }

    /// Drop every reading whose tag satisfies `drop`, keeping `tags` and
    /// `lemmas` aligned (they are parallel vectors: one lemma per tag).
    pub fn remove_readings(&mut self, drop: impl Fn(&str) -> bool) {
        let mut i = 0;
        while i < self.tags.len() {
            if drop(&self.tags[i]) {
                self.tags.remove(i);
                if i < self.lemmas.len() {
                    self.lemmas.remove(i);
                }
            } else {
                i += 1;
            }
        }
    }

    pub fn remove_tag(&mut self, tag: &str) {
        if let Some(i) = self.tags.iter().position(|t| t == tag) {
            self.tags.remove(i);
            self.lemmas.remove(i);
        }
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// First lemma for a tag matching `pred`.
    pub fn lemma_for<F: Fn(&str) -> bool>(&self, pred: F) -> Option<&str> {
        self.tags
            .iter()
            .position(|t| pred(t))
            .map(|i| self.lemmas[i].as_str())
    }
}

/// A sentence ready for rule evaluation.
#[derive(Clone, Debug)]
pub struct Sentence {
    /// Sentence text (without leading whitespace).
    pub text: String,
    /// Offset of the sentence in the original document.
    pub offset: usize,
    pub tokens: Vec<AnalyzedToken>,
}

impl Sentence {
    pub fn new(text: &str, offset: usize) -> Sentence {
        Sentence {
            text: text.to_string(),
            offset,
            tokens: Vec::new(),
        }
    }

    /// Rebuild the sentence text from the tokens (used after disambiguation
    /// rewrites multiwords into spaces).
    pub fn refresh_text(&mut self) {
        // Text stays as originally tokenized; tokens rewrite in place.
    }
}
