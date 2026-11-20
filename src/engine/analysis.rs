//! From a text to tagged sentences, before disambiguation.
//!
//! Port of the analysis half of `org.languagetool.JLanguageTool`
//! (LanguageTool, LGPL-2.1-or-later): `sentenceTokenize`,
//! `getRawAnalyzedSentence` (with `replaceSoftHyphens`), `analyzeText` /
//! `analyzeSentences` and `markAsParagraphEnd`, for French
//! (`org.languagetool.language.French`: SRX sentence tokenizer,
//! `FrenchWordTokenizer`, `FrenchTagger`, no chunker).
//!
//! ```text
//! text ── sentences() ── raw_sentence() ── [disambiguation] ── mark_as_paragraph_end()
//! ```
//!
//! The disambiguator is not part of this module: [`Analyzer::analyze_with`]
//! takes it as a function from the raw sentence to the final one, and
//! [`Analyzer::analyze_raw`] is the same pipeline without it.

use std::io;
use std::path::Path;
use std::sync::Arc;

use super::sentences::SrxSentenceTokenizer;
use super::tagger::{FrenchTagger, WordDictionary};
use super::token::{Reading, Readings, Sentence, SENTENCE_START};
use super::tokenize::FrenchWordTokenizer;

/// `Language.getIgnoredCharactersRegex()`: `[­]`, the soft hyphen.
const SOFT_HYPHEN: char = '\u{ad}';

/// Sentence splitting, word tokenizing and tagging for French.
pub struct Analyzer {
    tagger: Arc<FrenchTagger>,
    word_tokenizer: FrenchWordTokenizer,
    sentence_tokenizer: SrxSentenceTokenizer,
}

impl Analyzer {
    /// `assets_dir` holds LanguageTool's French resources: `segment.srx`,
    /// `added.txt`, `added_custom.txt`, `removed.txt`, `removed_custom.txt`.
    pub fn new(dictionary: Arc<dyn WordDictionary>, assets_dir: &Path) -> io::Result<Analyzer> {
        let tagger = Arc::new(FrenchTagger::new(dictionary, assets_dir)?);
        let srx = std::fs::read_to_string(assets_dir.join("segment.srx"))?;
        let sentence_tokenizer =
            SrxSentenceTokenizer::new(&srx, "fr").map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(Analyzer::from_parts(tagger, sentence_tokenizer))
    }

    pub fn from_parts(tagger: Arc<FrenchTagger>, sentence_tokenizer: SrxSentenceTokenizer) -> Analyzer {
        let for_tokenizer = tagger.clone();
        Analyzer {
            tagger,
            word_tokenizer: FrenchWordTokenizer::new(move |word| for_tokenizer.is_tagged(word)),
            sentence_tokenizer,
        }
    }

    pub fn tagger(&self) -> &FrenchTagger {
        &self.tagger
    }

    pub fn word_tokenizer(&self) -> &FrenchWordTokenizer {
        &self.word_tokenizer
    }

    pub fn sentence_tokenizer(&self) -> &SrxSentenceTokenizer {
        &self.sentence_tokenizer
    }

    /// `JLanguageTool.sentenceTokenize`: the sentences of `text`. They cover
    /// the text: each one keeps the whitespace that follows it.
    pub fn sentences<'t>(&self, text: &'t str) -> Vec<&'t str> {
        self.sentence_tokenizer.tokenize(text)
    }

    /// The sentences of `text` as byte ranges.
    pub fn sentence_ranges(&self, text: &str) -> Vec<(usize, usize)> {
        self.sentence_tokenizer.ranges(text, false)
    }

    /// `JLanguageTool.getRawAnalyzedSentence`, as a token list: the tagged
    /// tokens of one sentence, whitespace included, after a `SENT_START`
    /// token. `start_pos` is a byte offset into `sentence`.
    pub fn raw_tokens(&self, sentence: &str) -> Vec<Readings> {
        let mut tokens = self.word_tokenizer.tokenize(sentence);

        // Byte offset of each token in the sentence. The tokens cover the
        // sentence char for char, but not always byte for byte: the tokenizer
        // reads U+2010 and U+2011 as "-". (LanguageTool derives the positions
        // from the lengths of the tagged tokens and corrects them for the
        // soft hyphens taken out below; here they are read off the text.)
        let mut starts = Vec::with_capacity(tokens.len());
        let mut byte = 0;
        for token in &tokens {
            starts.push(byte);
            if sentence.as_bytes()[byte..].starts_with(token.as_bytes()) {
                byte += token.len();
            } else {
                byte += sentence[byte..].chars().take(token.chars().count()).map(char::len_utf8).sum::<usize>();
            }
        }

        // replaceSoftHyphens(): the tagger sees the words without them
        let mut soft_hyphen_tokens: Vec<(usize, String)> = Vec::new();
        for (i, token) in tokens.iter_mut().enumerate() {
            if token.contains(SOFT_HYPHEN) {
                let cleaned = token.replace(SOFT_HYPHEN, "");
                soft_hyphen_tokens.push((i, std::mem::replace(token, cleaned)));
            }
        }

        let mut token_array = Vec::with_capacity(tokens.len() + 1);
        token_array.push(Readings::new(vec![Reading::from_strs("", Some(SENTENCE_START), None)], 0));
        let mut soft = soft_hyphen_tokens.into_iter().peekable();
        for (i, token) in tokens.iter().enumerate() {
            let mut readings = self.tagger.tag_token(token);
            readings.start_pos = starts[i];
            if i > 0 {
                let previous = token_array[i].token.clone();
                readings.set_whitespace_before(&previous);
            }
            if soft.peek().map_or(false, |(index, _)| *index == i) {
                if let Some((_, original)) = soft.next() {
                    // The token as written is kept as an untagged reading (and
                    // becomes the token text, being the longer one).
                    readings.add_reading(Reading::from_strs(&original, None, None));
                }
            }
            token_array.push(readings);
        }

        // make SENT_END appear at last not whitespace token
        let count = token_array.len();
        let mut last_token = count - 1;
        for i in 0..count - 1 {
            if !token_array[last_token - i].is_whitespace {
                last_token -= i;
                break;
            }
        }
        token_array[last_token].set_sent_end();
        if count == last_token + 1 && token_array[last_token].is_linebreak {
            token_array[last_token].set_paragraph_end();
        }
        token_array
    }

    /// `JLanguageTool.getRawAnalyzedSentence`: one sentence, tagged but not
    /// disambiguated. This is what a disambiguator takes.
    pub fn raw_sentence(&self, sentence: &str) -> Sentence {
        Sentence::new(Arc::from(sentence), self.raw_tokens(sentence), Vec::new())
    }

    /// `JLanguageTool.analyzeText`, with `disambiguate` in the place of the
    /// language's disambiguator: it gets each raw sentence and returns the
    /// sentence to keep (its `pre_disambig` tokens included). The last
    /// sentence is then marked as the end of the paragraph.
    pub fn analyze_with<F>(&self, text: &str, mut disambiguate: F) -> Vec<Sentence>
    where
        F: FnMut(Sentence) -> Sentence,
    {
        let sentences = self.sentences(text);
        let mut out = Vec::with_capacity(sentences.len());
        for sentence in sentences {
            out.push(disambiguate(self.raw_sentence(sentence)));
        }
        if let Some(last) = out.last_mut() {
            mark_as_paragraph_end(last);
        }
        out
    }

    /// The sentences of `text`, tagged but not disambiguated; `pre_disambig`
    /// is left empty.
    pub fn analyze_raw(&self, text: &str) -> Vec<Sentence> {
        self.analyze_with(text, |raw| raw)
    }
}

/// `JLanguageTool.markAsParagraphEnd`: the last token of the last sentence
/// of a text takes a `PARA_END` reading, whitespace or not, and so does its
/// pre-disambiguation twin.
pub fn mark_as_paragraph_end(sentence: &mut Sentence) {
    let Some(last) = sentence.tokens.len().checked_sub(1) else { return };
    sentence.tokens[last].set_paragraph_end();
    if let Some(twin) = sentence.pre_disambig.get_mut(last) {
        twin.set_paragraph_end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tagger::{ManualTagger, TextDictionary};
    use crate::engine::token::{PARAGRAPH_END, SENTENCE_END};

    const SRX: &str = r#"<srx><header cascade="yes"></header><body><languagerules>
<languagerule languagerulename="Two"><rule break="yes"><beforebreak>\r?\n[ \t\n\x0B\f\r]*\r?\n[\t]*</beforebreak><afterbreak></afterbreak></rule></languagerule>
<languagerule languagerulename="French"><rule break="yes"><beforebreak>[\.!?…][\s ]+</beforebreak><afterbreak></afterbreak></rule></languagerule>
</languagerules><maprules>
<languagemap languagepattern="[a-z]{2,3}_two" languagerulename="Two"></languagemap>
<languagemap languagepattern="(FR|fr).*" languagerulename="French"></languagemap>
</maprules></body></srx>"#;

    const DUMP: &str = "\
.\t.\tM fin
Il\til\tR pers suj 3 m s
dort\tdormir\tV ind pres 3 s
il\til\tR pers suj 3 m s
l'\tle\tD e s
mot\tmot\tN m s
peut-être\tpeut-être\tA
";

    fn analyzer() -> Analyzer {
        let dict = Arc::new(TextDictionary::from_text(DUMP.to_string()));
        let tagger = Arc::new(FrenchTagger::with_manual_data(dict, ManualTagger::default(), ManualTagger::default()));
        Analyzer::from_parts(tagger, SrxSentenceTokenizer::new(SRX, "fr").unwrap())
    }

    fn tags(t: &Readings) -> Vec<&str> {
        t.readings.iter().map(|r| r.pos().unwrap_or("-")).collect()
    }

    #[test]
    fn raw_sentence_frames_and_flags_the_tokens() {
        let a = analyzer();
        let s = a.raw_sentence("Il dort. ");
        let texts: Vec<&str> = s.tokens.iter().map(|t| &*t.token).collect();
        assert_eq!(texts, ["", "Il", " ", "dort", ".", " "]);
        assert!(s.tokens[0].is_sent_start);
        assert_eq!(s.tokens.iter().map(|t| t.start_pos).collect::<Vec<_>>(), [0, 0, 2, 3, 7, 8]);
        assert_eq!(s.tokens.iter().map(|t| t.whitespace_before).collect::<Vec<_>>(), [false, false, false, true, false, false]);
        // SENT_END goes to the last token that is not whitespace, and the
        // period is looked up twice (it has no lower case).
        assert_eq!(tags(&s.tokens[4]), ["M fin", "M fin", SENTENCE_END]);
        assert!(s.tokens[4].is_sent_end && !s.tokens[5].is_sent_end);
        assert_eq!(s.len(), 4, "non-whitespace tokens, SENT_START included");
    }

    #[test]
    fn positions_are_bytes_of_the_sentence_text() {
        let a = analyzer();
        // "’" is three bytes but the token reads "l'"; U+2010 is read as "-".
        let s = a.raw_sentence("l’été peut\u{2010}être");
        let texts: Vec<&str> = s.tokens.iter().map(|t| &*t.token).collect();
        assert_eq!(texts, ["", "l'", "été", " ", "peut-être"]);
        assert_eq!(s.tokens.iter().map(|t| t.start_pos).collect::<Vec<_>>(), [0, 0, 4, 9, 10]);
        assert_eq!(tags(&s.tokens[4]), ["A", SENTENCE_END]);
        assert_eq!(&s.text[s.tokens[2].start_pos..s.tokens[2].start_pos + 5], "été");
    }

    #[test]
    fn soft_hyphens_are_ignored_by_the_tagger() {
        let a = analyzer();
        let s = a.raw_sentence("un mo\u{ad}t dort");
        let t = &s.tokens[3];
        assert_eq!(&*t.token, "mo\u{ad}t", "the token keeps the soft hyphen");
        assert_eq!(&*t.readings[0].token, "mot");
        assert_eq!(tags(t), ["N m s", "-"]);
        assert_eq!(s.tokens[5].start_pos, 9, "positions count the soft hyphen");
        // An unknown word: the untagged reading of the clean word is replaced.
        let s = a.raw_sentence("xy\u{ad}z");
        assert_eq!(s.tokens[1].readings.len(), 1);
        assert_eq!(&*s.tokens[1].readings[0].token, "xy\u{ad}z");
    }

    #[test]
    fn whitespace_only_sentences() {
        let a = analyzer();
        let s = a.raw_sentence(" ");
        assert_eq!(tags(&s.tokens[1]), [SENTENCE_END]);
        let s = a.raw_sentence("\n");
        assert_eq!(tags(&s.tokens[1]), [SENTENCE_END, PARAGRAPH_END]);
        let s = a.raw_sentence("");
        assert_eq!(tags(&s.tokens[0]), [SENTENCE_START, SENTENCE_END]);
    }

    #[test]
    fn analyze_raw_splits_and_marks_the_paragraph_end() {
        let a = analyzer();
        assert_eq!(a.sentences("Il dort. Il dort.\n\nIl dort"), ["Il dort. ", "Il dort.\n\n", "Il dort"]);
        assert_eq!(a.sentence_ranges("Il dort. Il"), [(0, 9), (9, 11)]);
        let sentences = a.analyze_raw("Il dort. Il dort.\n");
        assert_eq!(sentences.len(), 2);
        assert!(!sentences[0].tokens.last().unwrap().is_para_end);
        let last = sentences[1].tokens.last().unwrap();
        assert_eq!(&*last.token, "\n");
        assert_eq!(tags(last), [PARAGRAPH_END], "PARA_END goes to the very last token");
        assert_eq!(tags(&sentences[1].tokens[4]), ["M fin", "M fin", SENTENCE_END]);
        assert!(a.analyze_raw("").is_empty());
    }

    #[test]
    fn the_disambiguation_seam_sees_every_raw_sentence() {
        let a = analyzer();
        let mut seen = Vec::new();
        let out = a.analyze_with("Il dort. Il dort.", |raw| {
            seen.push(raw.text.to_string());
            // What a disambiguator does: keep the raw tokens aside.
            let before = raw.tokens.clone();
            Sentence::new(raw.text, raw.tokens, before)
        });
        assert_eq!(seen, ["Il dort. ", "Il dort."]);
        let last = &out[1];
        assert!(last.tokens.last().unwrap().is_para_end && last.pre_disambig.last().unwrap().is_para_end);
    }
}
