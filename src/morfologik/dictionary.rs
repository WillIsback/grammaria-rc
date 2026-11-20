//! A Morfologik dictionary: an automaton plus its metadata, and word lookup.
//!
//! Port of `morfologik.stemming.Dictionary`, `DictionaryLookup`,
//! `DictionaryIterator` and `WordData` (Morfologik 2.1.9, BSD-3-Clause), and
//! of the way LanguageTool reads them in
//! `org.languagetool.tagging.MorfologikTagger.tag` (frequency byte stripped
//! from the tag).
//!
//! A [`Dictionary`] is immutable and `Send + Sync`: load it once and share it.
//! [`Dictionary::lookup`] can be called from any thread. A [`Lookup`] is the
//! equivalent of Java's `DictionaryLookup` object: it owns the scratch buffers
//! of a lookup, so that a thread which tags many tokens allocates nothing per
//! word beyond the strings it returns.

use std::borrow::Cow;
use std::io;
use std::path::Path;

use super::encoders;
use super::fsa::{match_sequence, Automaton, Fsa, MatchKind, SequenceIter};
use super::metadata::DictionaryMetadata;

/// One reading of a word: its stem (lemma) and its tag.
///
/// Java returns `null` for a missing stem or tag; here they are empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct WordData {
    /// The decoded stem.
    pub stem: String,
    /// The tag. From [`Dictionary::lookup`], without the frequency byte; from
    /// [`Dictionary::lookup_raw`], exactly as stored.
    pub tag: String,
}

/// A dictionary: `morfologik.stemming.Dictionary`.
pub struct Dictionary {
    /// The automaton holding the entries.
    pub fsa: Fsa,
    /// The contents of the `.info` file.
    pub metadata: DictionaryMetadata,
    /// Cached `fsa.root_node()`.
    root: usize,
}

impl Dictionary {
    /// Reads a dictionary from its `.dict` and `.info` files.
    pub fn load(dict_path: impl AsRef<Path>, info_path: impl AsRef<Path>) -> io::Result<Dictionary> {
        let dict = std::fs::read(dict_path)?;
        let info = std::fs::read_to_string(info_path)?;
        Self::from_bytes(&dict, &info)
    }

    /// `Dictionary.read(Path)`: reads a `.dict` file and the `.info` file of
    /// the same name beside it.
    pub fn load_with_sibling_info(dict_path: impl AsRef<Path>) -> io::Result<Dictionary> {
        let dict_path = dict_path.as_ref();
        let name = dict_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid dictionary path"))?;
        let info = dict_path.with_file_name(DictionaryMetadata::expected_metadata_file_name(name));
        Self::load(dict_path, info)
    }

    /// `Dictionary.read(InputStream, InputStream)`: builds a dictionary from
    /// the contents of its two files.
    pub fn from_bytes(dict: &[u8], info: &str) -> io::Result<Dictionary> {
        Ok(Self::new(Fsa::read(dict)?, DictionaryMetadata::parse(info)?))
    }

    /// A dictionary from an automaton and metadata already in memory.
    pub fn new(fsa: Fsa, metadata: DictionaryMetadata) -> Dictionary {
        let root = fsa.root_node();
        Dictionary { fsa, metadata, root }
    }

    /// The root node of the automaton.
    #[inline]
    pub fn root_node(&self) -> usize {
        self.root
    }

    /// A reusable lookup object (Java's `new DictionaryLookup(dictionary)`).
    pub fn lookup_session(&self) -> Lookup<'_> {
        Lookup::new(self)
    }

    /// The readings of `word` as LanguageTool's `MorfologikTagger.tag` returns
    /// them: in Morfologik's order, the frequency byte removed from the tag
    /// when the dictionary includes frequencies. `out` is overwritten (its
    /// strings are reused).
    pub fn lookup(&self, word: &str, out: &mut Vec<WordData>) {
        Lookup::new(self).lookup(word, out);
    }

    /// The readings of `word` exactly as `DictionaryLookup.lookup` returns
    /// them (the tag keeps its frequency byte). `out` is overwritten.
    pub fn lookup_raw(&self, word: &str, out: &mut Vec<WordData>) {
        Lookup::new(self).lookup_raw(word, out);
    }

    /// The frequency class of `word`, see [`Lookup::frequency`].
    pub fn frequency(&self, word: &str) -> i32 {
        Lookup::new(self).frequency(word)
    }

    /// True if `word` has at least one reading.
    pub fn contains(&self, word: &str) -> bool {
        let mut found = false;
        Lookup::new(self).for_each(word, |_, _| found = true);
        found
    }

    /// Port of `DictionaryIterator`: calls `f(word, stem, tag)` for every
    /// entry of the dictionary, in the automaton's order. The tag is raw
    /// (frequency byte included). An entry without separator is reported with
    /// an empty stem and tag (Java throws).
    pub fn for_each_entry(&self, mut f: impl FnMut(&str, &str, &str)) {
        let separator = self.metadata.separator;
        let encoder = self.metadata.encoder_type;
        let prefix_bytes = encoders::prefix_bytes(encoder);
        let mut stem = Vec::new();
        let mut it = SequenceIter::new(&self.fsa, self.root);
        while let Some(entry) = it.next() {
            let Some(sep) = entry.iter().position(|&b| b == separator) else {
                f(&String::from_utf8_lossy(entry), "", "");
                continue;
            };
            let word = &entry[..sep];
            let rest = &entry[sep + 1..];
            let stem_end = split_stem(rest, prefix_bytes, separator);
            encoders::decode(encoder, word, &rest[..stem_end], &mut stem);
            let tag = rest.get(stem_end + 1..).unwrap_or(&[]);
            f(
                &String::from_utf8_lossy(word),
                &String::from_utf8_lossy(&stem),
                &String::from_utf8_lossy(tag),
            );
        }
    }
}

/// The offset of the separator that ends the encoded stem in `rest`
/// (`rest.len()` if there is none). The control bytes are skipped: one of
/// them may have the value of the separator.
#[inline]
fn split_stem(rest: &[u8], prefix_bytes: usize, separator: u8) -> usize {
    let mut pos = prefix_bytes.min(rest.len());
    while pos < rest.len() && rest[pos] != separator {
        pos += 1;
    }
    pos
}

/// Port of `DictionaryLookup`: looks words up, reusing its buffers.
///
/// Not shareable between threads (it is mutable); create one per thread with
/// [`Dictionary::lookup_session`]. Creating one costs nothing.
pub struct Lookup<'a> {
    dict: &'a Dictionary,
    /// Walks the entries of a word (the sequences after its separator).
    iter: SequenceIter<'a, Fsa>,
    /// Decoded stem of the current entry.
    stem: Vec<u8>,
}

impl<'a> Lookup<'a> {
    /// A lookup object over `dict`.
    pub fn new(dict: &'a Dictionary) -> Lookup<'a> {
        Lookup {
            dict,
            iter: SequenceIter::empty(&dict.fsa),
            stem: Vec::new(),
        }
    }

    /// The dictionary this object reads.
    pub fn dictionary(&self) -> &'a Dictionary {
        self.dict
    }

    /// The core of `DictionaryLookup.lookup`: calls `f(stem, raw_tag)` for
    /// every reading of `word`, in the automaton's order. Both slices are
    /// UTF-8 bytes valid only during the call; the tag keeps the frequency
    /// byte if the dictionary has one. Nothing is allocated once the buffers
    /// have grown.
    #[inline]
    pub fn for_each(&mut self, word: &str, mut f: impl FnMut(&[u8], &[u8])) {
        let meta = &self.dict.metadata;
        let fsa = &self.dict.fsa;
        let separator = meta.separator;

        let word: Cow<str> = if meta.input_conversion_pairs.is_empty() {
            Cow::Borrowed(word)
        } else {
            Cow::Owned(apply_replacements(word, &meta.input_conversion_pairs))
        };
        let bytes = word.as_bytes();
        // No valid input can contain the separator.
        if bytes.contains(&separator) {
            return;
        }

        // Try to find a partial match in the dictionary.
        let m = match_sequence(fsa, bytes, self.dict.root);
        if m.kind != MatchKind::SequenceIsAPrefix {
            return;
        }
        let arc = fsa.arc(m.node, separator);
        if arc == 0 || fsa.is_arc_final(arc) {
            return;
        }
        // There is such a word in the dictionary. Return its base forms.
        let encoder = meta.encoder_type;
        let prefix_bytes = encoders::prefix_bytes(encoder);
        self.iter.restart_from(fsa.end_node(arc));
        while let Some(entry) = self.iter.next() {
            let sep_pos = split_stem(entry, prefix_bytes, separator);
            encoders::decode(encoder, bytes, &entry[..sep_pos], &mut self.stem);
            // Skip the separator; what follows is the tag.
            let tag = entry.get(sep_pos + 1..).unwrap_or(&[]);
            f(&self.stem, tag);
        }
    }

    /// `DictionaryLookup.lookup`: the readings of `word`, tags as stored.
    pub fn lookup_raw(&mut self, word: &str, out: &mut Vec<WordData>) {
        self.fill(word, out, false);
    }

    /// `MorfologikTagger.tag`: the readings of `word`; when the dictionary
    /// includes frequencies and a tag is longer than one character, its last
    /// character (the frequency class) is removed.
    pub fn lookup(&mut self, word: &str, out: &mut Vec<WordData>) {
        let strip = self.dict.metadata.frequency_included;
        self.fill(word, out, strip);
    }

    fn fill(&mut self, word: &str, out: &mut Vec<WordData>, strip_frequency: bool) {
        let mut n = 0;
        self.for_each(word, |stem, tag| {
            if n == out.len() {
                out.push(WordData::default());
            }
            let wd = &mut out[n];
            n += 1;
            wd.stem.clear();
            wd.stem.push_str(&String::from_utf8_lossy(stem));
            wd.tag.clear();
            wd.tag.push_str(&String::from_utf8_lossy(tag));
            if strip_frequency && wd.tag.chars().nth(1).is_some() {
                wd.tag.pop();
            }
        });
        out.truncate(n);
    }

    /// `Speller.getFrequency`: the frequency class of `word`, from `0` (rare)
    /// to `25` (frequent), taken from the last byte of its first entry.
    /// `0` if the word is unknown or the dictionary has no frequencies.
    pub fn frequency(&mut self, word: &str) -> i32 {
        let meta = &self.dict.metadata;
        if !meta.frequency_included {
            return 0;
        }
        let fsa = &self.dict.fsa;
        let m = match_sequence(fsa, word.as_bytes(), self.dict.root);
        if m.kind == MatchKind::SequenceIsAPrefix {
            let arc = fsa.arc(m.node, meta.separator);
            if arc != 0 && !fsa.is_arc_final(arc) {
                self.iter.restart_from(fsa.end_node(arc));
                if let Some(entry) = self.iter.next() {
                    // The last byte holds the frequency (Java: a signed byte).
                    let last = entry[entry.len() - 1] as i8;
                    return i32::from(last) - i32::from(b'A');
                }
            }
        }
        0
    }
}

/// `DictionaryLookup.applyReplacements`: applies the conversion pairs in
/// order, each one replacing every occurrence of its key.
///
/// As in Java, after a replacement the search for the same key resumes
/// `key.length()` UTF-16 units after the start of the replaced text.
pub fn apply_replacements(word: &str, replacements: &[(String, String)]) -> String {
    if replacements.is_empty() {
        return word.to_string();
    }
    let mut sb: Vec<u16> = word.encode_utf16().collect();
    for (key, value) in replacements {
        let key: Vec<u16> = key.encode_utf16().collect();
        let value: Vec<u16> = value.encode_utf16().collect();
        let mut index = index_of(&sb, &key, 0);
        while let Some(i) = index {
            sb.splice(i..i + key.len(), value.iter().copied());
            index = index_of(&sb, &key, (i + key.len()) as isize);
        }
    }
    String::from_utf16_lossy(&sb)
}

/// `StringBuilder.indexOf(needle, from)` on UTF-16 units.
pub(super) fn index_of(haystack: &[u16], needle: &[u16], from: isize) -> Option<usize> {
    let from = from.max(0) as usize;
    if from > haystack.len() {
        return if needle.is_empty() { Some(haystack.len()) } else { None };
    }
    if needle.is_empty() {
        return Some(from);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    (from..=haystack.len() - needle.len()).find(|&i| haystack[i..i + needle.len()] == *needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacements_follow_java() {
        let pairs = vec![("a".to_string(), "bb".to_string()), ("bbb".to_string(), "c".to_string())];
        assert_eq!(apply_replacements("xay", &pairs), "xbby");
        assert_eq!(apply_replacements("aa", &pairs), "cb");
        assert_eq!(apply_replacements("word", &[]), "word");
        let u = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert_eq!(index_of(&u("abcabc"), &u("bc"), 2), Some(4));
        assert_eq!(index_of(&u("abcabc"), &u("bc"), -3), Some(1));
        assert_eq!(index_of(&u("abc"), &u("abcd"), 0), None);
        assert_eq!(index_of(&u("abc"), &u("c"), 7), None);
    }

    #[test]
    fn dictionary_is_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<Dictionary>();
        fn check_send<T: Send>() {}
        check_send::<Lookup<'static>>();
    }
}
