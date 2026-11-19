//! Zero-copy, memory-mapped lexicon.
//!
//! The "full" [`crate::lexicon::Lexicon`] deserialises 37 MB of trie arrays and
//! sense tables into owned `Vec`s, which costs ~50 ms at every start-up (read +
//! little-endian decode). That dominates the run time of a short check.
//!
//! This module keeps the file bytes in memory and decodes integers on demand,
//! so loading is a single `read()` (~27 ms, mostly the kernel copying the file)
//! and no per-element allocation happens. Lookup is still a double-array trie
//! walk; only the arithmetic changed (byte offsets instead of indexed `u32`s).
//!
//! File layout (identical to `lexicon::Lexicon::save`):
//!
//! ```text
//!   magic "GRMLXIC2"                     8 bytes
//!   [nslots, nsense_ids, nsenses, nlemmas, ntags]   5 x u32
//!   base[nslots]  check[nslots]  first[nslots]  count[nslots]
//!   sense_ids[nsense_ids]
//!   senses[nsenses]  (lemma_id, tag_id) pairs
//!   lemmas: offsets[nlemmas+1] then blob
//!   tags:   offsets[ntags+1]   then blob
//! ```

use std::fs;
use std::io;
use std::path::Path;

pub const MAGIC: &[u8; 8] = b"GRMLXIC2";

/// A lexicon backed by a raw byte buffer.
pub struct MappedLexicon {
    bytes: Vec<u8>,
    /// Byte offset of each of the four per-slot arrays, and their count.
    nslots: usize,
    base_off: usize,
    check_off: usize,
    first_off: usize,
    count_off: usize,
    sense_ids_off: usize,
    nsense_ids: usize,
    senses_off: usize,
    nsenses: usize,
    /// Offsets table + blob for lemmas and tags.
    lemmas_off: usize,
    nlemmas: usize,
    tags_off: usize,
    ntags: usize,
}

impl MappedLexicon {
    /// Load `path` into memory (single read, no decode pass).
    pub fn load<P: AsRef<Path>>(path: P) -> io::Result<MappedLexicon> {
        let bytes = fs::read(path)?;
        MappedLexicon::from_bytes(bytes)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> io::Result<MappedLexicon> {
        if bytes.len() < 28 || &bytes[..8] != MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad lexicon magic"));
        }
        let meta = |i: usize| u32::from_le_bytes(bytes[8 + i * 4..12 + i * 4].try_into().unwrap()) as usize;
        let nslots = meta(0);
        let nsense_ids = meta(1);
        let nsenses = meta(2);
        let nlemmas = meta(3);
        let ntags = meta(4);
        let base_off = 28;
        let check_off = base_off + nslots * 4;
        let first_off = check_off + nslots * 4;
        let count_off = first_off + nslots * 4;
        let sense_ids_off = count_off + nslots * 4;
        let senses_off = sense_ids_off + nsense_ids * 4;
        let lemmas_off = senses_off + nsenses * 8;
        let lemmas_blob = lemmas_off + (nlemmas + 1) * 4;
        let blob_len = read_u32(&bytes, lemmas_off + nlemmas * 4) as usize;
        let tags_off = lemmas_blob + blob_len;
        Ok(MappedLexicon {
            bytes,
            nslots,
            base_off,
            check_off,
            first_off,
            count_off,
            sense_ids_off,
            nsense_ids,
            senses_off,
            nsenses,
            lemmas_off,
            nlemmas,
            tags_off,
            ntags,
        })
    }

    /// Number of word forms in the lexicon.
    pub fn len(&self) -> usize {
        self.count_ranges()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn count_ranges(&self) -> usize {
        // Number of slots that carry at least one sense.
        let mut n = 0;
        for s in 0..self.nslots {
            if self.count(s) > 0 {
                n += 1;
            }
        }
        n
    }

    #[inline]
    fn u32_at(&self, off: usize) -> u32 {
        u32::from_le_bytes(self.bytes[off..off + 4].try_into().unwrap())
    }

    #[inline]
    fn base(&self, slot: usize) -> u32 {
        self.u32_at(self.base_off + slot * 4)
    }

    #[inline]
    fn check(&self, slot: usize) -> u32 {
        self.u32_at(self.check_off + slot * 4)
    }

    #[inline]
    fn first(&self, slot: usize) -> u32 {
        self.u32_at(self.first_off + slot * 4)
    }

    #[inline]
    fn count(&self, slot: usize) -> u32 {
        self.u32_at(self.count_off + slot * 4)
    }

    /// Walk the trie, returning the slot for `word` if it is a known form.
    fn find_slot(&self, word: &str) -> Option<usize> {
        if self.nslots == 0 {
            return None;
        }
        let mut slot = 1usize;
        for &b in word.as_bytes() {
            let base = self.base(slot) as usize;
            if base == 0 {
                return None;
            }
            let next = base + b as usize;
            if next >= self.nslots || self.check(next) as usize != slot {
                return None;
            }
            slot = next;
        }
        if self.count(slot) > 0 {
            Some(slot)
        } else {
            None
        }
    }

    /// All tags for `word` (empty slice if unknown).
    pub fn tags(&self, word: &str) -> Vec<&str> {
        let mut out = Vec::new();
        if let Some(slot) = self.find_slot(word) {
            let first = self.first(slot) as usize;
            let n = self.count(slot) as usize;
            for k in 0..n {
                let sid = first + k;
                if sid >= self.nsense_ids {
                    break;
                }
                let sense_id = self.u32_at(self.sense_ids_off + sid * 4) as usize;
                if sense_id >= self.nsenses {
                    continue;
                }
                let tag_id = self.u32_at(self.senses_off + sense_id * 8 + 4) as usize;
                out.push(self.string(self.tags_off, self.ntags, tag_id));
            }
        }
        out
    }

    /// All `(lemma, tag)` pairs for `word`.
    pub fn morphs(&self, word: &str) -> Vec<(&str, &str)> {
        let mut out = Vec::new();
        if let Some(slot) = self.find_slot(word) {
            let first = self.first(slot) as usize;
            let n = self.count(slot) as usize;
            for k in 0..n {
                let sid = first + k;
                if sid >= self.nsense_ids {
                    break;
                }
                let sense_id = self.u32_at(self.sense_ids_off + sid * 4) as usize;
                if sense_id >= self.nsenses {
                    continue;
                }
                let lemma_id = self.u32_at(self.senses_off + sense_id * 8) as usize;
                let tag_id = self.u32_at(self.senses_off + sense_id * 8 + 4) as usize;
                out.push((
                    self.string(self.lemmas_off, self.nlemmas, lemma_id),
                    self.string(self.tags_off, self.ntags, tag_id),
                ));
            }
        }
        out
    }

    /// True when `word` is a known form.
    pub fn contains(&self, word: &str) -> bool {
        self.find_slot(word).is_some()
    }

    /// Read string #`id` from an offsets-table + blob region.
    #[inline]
    fn string(&self, table_off: usize, n: usize, id: usize) -> &str {
        if id >= n {
            return "";
        }
        let start = self.u32_at(table_off + id * 4) as usize;
        let end = self.u32_at(table_off + (id + 1) * 4) as usize;
        let blob = table_off + (n + 1) * 4;
        std::str::from_utf8(&self.bytes[blob + start..blob + end]).unwrap_or("")
    }
}

fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexicon::{Lexicon, WordTable};

    fn build_file(dir: &Path) -> std::path::PathBuf {
        let mut t = WordTable::new();
        t.add("chat", "chat", "N m s");
        t.add("chats", "chat", "N m p");
        t.add("mange", "manger", "V ind pres 3 s");
        t.add("mange", "manger", "V sub pres 1 s");
        t.add("être", "être", "V etre inf");
        let lx = Lexicon::from_table(&t).compact();
        let p = dir.join("mapped_test.lex");
        lx.save(&p).unwrap();
        p
    }

    fn tmpdir() -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("grammaria_mapled_{}_{}", std::process::id(), n));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn matches_owned_lexicon() {
        let dir = tmpdir();
        let path = build_file(&dir);
        let mapped = MappedLexicon::load(&path).unwrap();
        let owned = Lexicon::load(&path).unwrap();

        for w in ["chat", "chats", "mange", "être", "inconnu", "cha"] {
            let mut a: Vec<_> = mapped.tags(w);
            let mut b = Vec::new();
            owned.tags(w, &mut b);
            a.sort();
            let mut b2 = b.clone();
            b2.sort();
            assert_eq!(a, b2, "tags mismatch for {w}");
            assert_eq!(mapped.contains(w), owned.contains(w), "contains mismatch for {w}");
        }

        let morphs = mapped.morphs("mange");
        assert_eq!(morphs.len(), 2);
        assert!(morphs.iter().any(|(l, t)| *l == "manger" && *t == "V ind pres 3 s"));
    }

    #[test]
    fn rejects_bad_magic() {
        assert!(MappedLexicon::from_bytes(b"not a lexicon at all".to_vec()).is_err());
    }
}
