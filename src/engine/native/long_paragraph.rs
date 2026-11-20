//! Port of `org.languagetool.rules.LongParagraphRule` (with
//! `Tools.isParagraphEnd`): a paragraph with more words than the limit. Off
//! by default and tagged `picky`.

use super::super::checker::NativeRule;
use super::super::rule::{RuleContext, RuleInfo, RuleMatch};
use super::super::token::Sentence;
use super::{messages, rule_info, rule_match, STYLE};

const DEFAULT_MAX_WORDS: usize = 220;

pub struct LongParagraphRule {
    info: RuleInfo,
    max_words: usize,
    message: String,
}

impl LongParagraphRule {
    pub fn new() -> LongParagraphRule {
        LongParagraphRule::with_max_words(DEFAULT_MAX_WORDS)
    }

    pub fn with_max_words(max_words: usize) -> LongParagraphRule {
        let limit = max_words.to_string();
        let mut info = rule_info("TOO_LONG_PARAGRAPH", &messages::LONG_PARAGRAPH_RULE_DESC.replace("{0}", &limit), &STYLE, "style");
        info.default_off = true;
        info.tags = vec!["picky".to_string()];
        LongParagraphRule { info, max_words, message: messages::LONG_PARAGRAPH_RULE_MSG.replace("{0}", &limit) }
    }
}

impl Default for LongParagraphRule {
    fn default() -> Self {
        Self::new()
    }
}

/// `Tools.isParagraphEnd`, a single line break not ending a paragraph (the
/// default of the sentence tokenizer).
fn is_paragraph_end(sentences: &[Sentence], n: usize) -> bool {
    if n + 1 >= sentences.len() {
        return true;
    }
    let text: &str = &sentences[n].text;
    if text.ends_with("\n\n") || text.ends_with("\n\r\n\r") || text.ends_with("\r\n\r\n") {
        return true;
    }
    let next: &str = &sentences[n + 1].text;
    next.starts_with('\n') || next.starts_with("\r\n")
}

impl NativeRule for LongParagraphRule {
    fn info(&self) -> &RuleInfo {
        &self.info
    }

    fn is_text_level(&self) -> bool {
        true
    }

    fn match_text(&self, rule: usize, sentences: &[Sentence], _ctx: &RuleContext) -> Vec<RuleMatch> {
        let mut matches = Vec::new();
        let mut pos = 0;
        let mut start_pos = 0;
        let mut end_pos = 0;
        let mut word_count = 0;
        let mut para_has_linebreaks = false;
        for (n, sentence) in sentences.iter().enumerate() {
            let paragraph_end = is_paragraph_end(sentences, n);
            if !paragraph_end && sentence.text.trim_start_matches('\n').contains('\n') {
                para_has_linebreaks = true;
            }
            for i in 0..sentence.len() {
                let token = sentence.tok(i);
                if !token.is_whitespace && !token.is_sent_start && !token.is_non_word() {
                    word_count += 1;
                    // The match covers the two words at the limit.
                    if word_count == self.max_words {
                        end_pos = token.end_pos() + pos;
                    } else if word_count + 1 == self.max_words {
                        start_pos = token.start_pos + pos;
                    }
                }
            }
            if paragraph_end {
                // + 5: no match almost at the end of the paragraph.
                if word_count > self.max_words + 5 && !para_has_linebreaks {
                    matches.extend(rule_match(rule, start_pos, end_pos, &self.message, "", Vec::new()));
                }
                word_count = 0;
                para_has_linebreaks = false;
            }
            pos += sentence.text.len();
        }
        if word_count > self.max_words {
            matches.extend(rule_match(rule, start_pos, end_pos, &self.message, "", Vec::new()));
        }
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{context, plain};
    use super::*;

    fn check(sentences: &[&str]) -> Vec<(usize, usize)> {
        let sentences: Vec<Sentence> = sentences.iter().map(|s| plain(s)).collect();
        LongParagraphRule::with_max_words(4).match_text(0, &sentences, &context()).into_iter().map(|m| (m.from_pos, m.to_pos)).collect()
    }

    #[test]
    fn marks_the_words_at_the_limit() {
        // Ten words: the third and the fourth are marked.
        assert_eq!(check(&["Un deux trois. ", "Quatre cinq six sept huit neuf dix."]), vec![(8, 21)]);
        // Nine words: too close to the limit.
        assert!(check(&["Un deux trois. ", "Quatre cinq six sept huit neuf."]).is_empty());
        // Two paragraphs of five words.
        assert!(check(&["Un deux trois quatre cinq.\n\n", "Un deux trois quatre cinq."]).is_empty());
    }

    #[test]
    fn is_off_by_default() {
        let rule = LongParagraphRule::new();
        assert!(rule.info().default_off);
        assert!(rule.info().description.contains("220"));
    }
}
