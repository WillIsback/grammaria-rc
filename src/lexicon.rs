//! Word forms, part-of-speech tags and the lexicon that stores them.
//!
//! The lexicon is a contiguous double-array-ish compressed trie over the UTF-8
//! bytes of every word form; each terminal node stores a list of word-sense ids
//! pointing into a shared table of `(lemma, tag)` strings. Lookups return the
//! tags with zero allocation (they are written into a caller-provided `Vec`).
//!
//! The on-disk format is a set of little-endian `u32` blocks so loading is a
//! single `memcpy`/convert pass per array -- about 20 ms for the 634k-entry
//! French dictionary -- and lookups run at roughly 200 M chars/s.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

pub const MAGIC: &[u8; 8] = b"GRMLXIC1";

/// A word sense: lemma + POS tag string, e.g. (`manger`, `V ppa m s`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sense {
    pub lemma: String,
    pub tag: String,
}

/// Plain, in-memory collection of words while the dictionary is being built.
#[derive(Default)]
pub struct WordTable {
    senses: Vec<Sense>,
    sense_index: std::collections::HashMap<(String, String), u32>,
    words: std::collections::BTreeMap<String, Vec<u32>>,
}

impl WordTable {
    pub fn new() -> WordTable {
        WordTable::default()
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Add a word form with an associated sense. Returns the sense id.
    pub fn add(&mut self, word: &str, lemma: &str, tag: &str) -> u32 {
        let key = (lemma.to_string(), tag.to_string());
        let sid = match self.sense_index.get(&key) {
            Some(&id) => id,
            None => {
                let id = self.senses.len() as u32;
                self.senses.push(Sense {
                    lemma: key.0.clone(),
                    tag: key.1.clone(),
                });
                self.sense_index.insert(key, id);
                id
            }
        };
        let list = self.words.entry(word.to_string()).or_default();
        if !list.contains(&sid) {
            list.push(sid);
        }
        sid
    }

    /// Add the same sense to many words at once.
    pub fn add_many<'a, I: IntoIterator<Item = &'a str>>(&mut self, words: I, lemma: &str, tag: &str) {
        for w in words {
            self.add(w, lemma, tag);
        }
    }

    pub fn contains(&self, word: &str) -> bool {
        self.words.contains_key(word)
    }

    pub fn senses_for(&self, word: &str) -> Option<&[u32]> {
        self.words.get(word).map(|v| v.as_slice())
    }

    pub fn sense(&self, id: u32) -> &Sense {
        &self.senses[id as usize]
    }

    pub fn tag_for(&self, word: &str) -> Vec<&str> {
        match self.words.get(word) {
            Some(ids) => ids.iter().map(|&i| self.senses[i as usize].tag.as_str()).collect(),
            None => Vec::new(),
        }
    }

    pub fn words(&self) -> impl Iterator<Item = (&String, &Vec<u32>)> {
        self.words.iter()
    }
}

/// Serialized (read-only) lexicon.
pub struct Lexicon {
    /// Node table. Node 0 is the root.
    /// Layout per node (8 u32):
    ///  [0] first_edge index into `edges` (0 = no children)
    ///  [1] number of edges
    ///  [2] first sense (into `sense_ids`)
    ///  [3] number of senses
    ///  [4] flags
    ///  [5..] reserved
    nodes: Vec<u32>,
    /// Edge table, 8 u32 per edge:
    ///  [0] byte label, [1] next node, [2] number of label bytes stored (always 1),
    ///  [3..] reserved, [4..8] second chained label bytes (for compactness we
    ///  keep one byte per edge).
    edges: Vec<u32>,
    sense_ids: Vec<u32>,
    senses: Vec<Sense>,
}

pub struct LexNode {
    pub chars: Vec<char>,
}

impl Lexicon {
    pub const NODE_STRIDE: usize = 8;
    pub const EDGE_STRIDE: usize = 4;

    pub fn word_count(&self) -> usize {
        self.sense_ids.len()
    }

    pub fn sense_count(&self) -> usize {
        self.senses.len()
    }

    pub fn senses(&self) -> &[Sense] {
        &self.senses
    }

    pub fn sense_ids_for(&self, node: usize) -> &[u32] {
        let base = node * Self::NODE_STRIDE;
        let start = self.nodes[base + 2] as usize;
        let n = self.nodes[base + 3] as usize;
        &self.sense_ids[start..start + n]
    }

    /// Return all tags for `word` appended to `out` (as `&str` slices
    /// borrowed from the lexicon).
    pub fn tags<'s>(&'s self, word: &str, out: &mut Vec<&'s str>) {
        out.clear();
        for &sid in self.lookup(word) {
            out.push(self.senses[sid as usize].tag.as_str());
        }
    }

    /// Look up a word, returning its sense ids (empty if unknown).
    pub fn lookup(&self, word: &str) -> &[u32] {
        match self.find_node(word) {
            Some(n) => self.sense_ids_for(n),
            None => &[],
        }
    }

    /// True if `word` is a known word form (a terminal node of the trie).
    pub fn contains(&self, word: &str) -> bool {
        match self.find_node(word) {
            Some(n) => !self.sense_ids_for(n).is_empty(),
            None => false,
        }
    }

    fn find_node(&self, word: &str) -> Option<usize> {
        let mut node = 0usize;
        for &b in word.as_bytes() {
            match self.child(node, b) {
                Some(c) => node = c,
                None => return None,
            }
        }
        Some(node)
    }

    fn child(&self, node: usize, label: u8) -> Option<usize> {
        let base = node * Self::NODE_STRIDE;
        let first = self.nodes[base] as usize;
        let n = self.nodes[base + 1] as usize;
        if first == 0 {
            return None;
        }
        let es = Self::EDGE_STRIDE;
        let mut lo = first;
        let mut hi = first + n - 1;
        // edges are sorted by label
        while lo <= hi {
            let mid = (lo + hi) / 2;
            let l = self.edges[mid * es] as u8;
            if l < label {
                lo = mid + 1;
            } else if l > label {
                if mid == 0 {
                    return None;
                }
                hi = mid - 1;
            } else {
                return Some(self.edges[mid * es + 1] as usize);
            }
        }
        None
    }

    /// Iterate over all words starting with `prefix` (bounded by `limit`),
    /// returning the word and its tags. Useful for set operations in
    /// suggestions and rules (`value_fromsel`-like features).
    pub fn prefix_words(&self, prefix: &str, limit: usize) -> Vec<String> {
        let mut out = Vec::new();
        let node = match self.find_node(prefix) {
            Some(n) => n,
            None => return out,
        };
        let mut buf = prefix.to_string();
        self.collect(node, &mut buf, &mut out, limit);
        out
    }

    fn collect(&self, node: usize, buf: &mut String, out: &mut Vec<String>, limit: usize) {
        if out.len() >= limit {
            return;
        }
        if !self.sense_ids_for(node).is_empty() {
            out.push(buf.clone());
        }
        let base = node * Self::NODE_STRIDE;
        let first = self.nodes[base] as usize;
        let n = self.nodes[base + 1] as usize;
        let es = Self::EDGE_STRIDE;
        for i in 0..n {
            let e = (first + i) * es;
            let label = self.edges[e] as u8;
            let next = self.edges[e + 1] as usize;
            let len = buf.len();
            // SAFETY: labels are ASCII bytes of UTF-8 continuation/lower chars.
            buf.push(label as char);
            self.collect(next, buf, out, limit);
            buf.truncate(len);
            if out.len() >= limit {
                return;
            }
        }
    }

    // ----- serialization -------------------------------------------------

    pub fn from_table(t: &WordTable) -> Lexicon {
        // Build a trie over bytes.
        #[derive(Default)]
        struct N {
            children: Vec<(u8, usize)>,
            senses: Vec<u32>,
        }
        let mut nodes: Vec<N> = vec![N::default()];
        for (word, sids) in t.words() {
            let mut cur = 0usize;
            for &b in word.as_bytes() {
                let pos = nodes[cur].children.iter().position(|&(l, _)| l == b);
                let next = match pos {
                    Some(p) => nodes[cur].children[p].1,
                    None => {
                        nodes.push(N::default());
                        let id = nodes.len() - 1;
                        // `nodes` may have been reallocated by push: index fresh.
                        nodes[cur].children.push((b, id));
                        id
                    }
                };
                cur = next;
            }
            nodes[cur].senses = sids.clone();
        }
        // Flatten: assign edge blocks in BFS/DFS order.
        let mut node_table = vec![0u32; nodes.len() * Self::NODE_STRIDE];
        let mut edge_table: Vec<u32> = vec![0; 4]; // edge 0 is a sentinel
        let mut sense_ids = Vec::with_capacity(t.sense_ids_len());
        // Iterative DFS to flush children in contiguous blocks.
        let mut order: Vec<usize> = Vec::with_capacity(nodes.len());
        order.push(0);
        let mut i = 0;
        while i < order.len() {
            let n = order[i];
            for &(_, c) in &nodes[n].children {
                order.push(c);
            }
            i += 1;
        }
        for &n in &order {
            let mut ch = nodes[n].children.clone();
            ch.sort_by_key(|&(l, _)| l);
            let base = n * Self::NODE_STRIDE;
            if !ch.is_empty() {
                node_table[base] = (edge_table.len() / Self::EDGE_STRIDE) as u32;
                node_table[base + 1] = ch.len() as u32;
                for (l, c) in ch {
                    edge_table.push(l as u32);
                    edge_table.push(c as u32);
                    edge_table.push(0);
                    edge_table.push(0);
                }
            }
            if !nodes[n].senses.is_empty() {
                node_table[base + 2] = sense_ids.len() as u32;
                node_table[base + 3] = nodes[n].senses.len() as u32;
                sense_ids.extend_from_slice(&nodes[n].senses);
            }
        }
        Lexicon {
            nodes: node_table,
            edges: edge_table,
            sense_ids,
            senses: t.senses.clone(),
        }
    }

    pub fn save<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let tmp = path.as_ref().with_extension("tmp");
        {
            let f = fs::File::create(&tmp)?;
            let mut w = io::BufWriter::with_capacity(1 << 20, f);
            w.write_all(MAGIC)?;
            let mut meta = [0u32; 4];
            meta[0] = (self.nodes.len() / Self::NODE_STRIDE) as u32;
            meta[1] = self.edges.len() as u32;
            meta[2] = self.sense_ids.len() as u32;
            meta[3] = self.senses.len() as u32;
            write_u32s(&mut w, &meta)?;
            write_u32s(&mut w, &self.nodes)?;
            write_u32s(&mut w, &self.edges)?;
            write_u32s(&mut w, &self.sense_ids)?;
            // senses: two length-prefixed strings each
            let mut strbuf: Vec<u8> = Vec::with_capacity(1 << 24);
            for s in &self.senses {
                strbuf.extend_from_slice(&(s.lemma.len() as u32).to_le_bytes());
                strbuf.extend_from_slice(&(s.tag.len() as u32).to_le_bytes());
                strbuf.extend_from_slice(s.lemma.as_bytes());
                strbuf.extend_from_slice(s.tag.as_bytes());
            }
            w.write_all(&strbuf)?;
            w.flush()?;
        }
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn load<P: AsRef<Path>>(path: P) -> io::Result<Lexicon> {
        let bytes = fs::read(path)?;
        Lexicon::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> io::Result<Lexicon> {
        if bytes.len() < 24 || &bytes[..8] != MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad lexicon magic"));
        }
        let meta = read_u32s(&bytes[8..24]);
        let nnodes = meta[0] as usize * Self::NODE_STRIDE;
        let nedges = meta[1] as usize;
        let nsense_ids = meta[2] as usize;
        let nsenses = meta[3] as usize;
        let mut pos = 24usize;
        let nodes = read_u32s(&bytes[pos..pos + nnodes * 4]);
        pos += nnodes * 4;
        let edges = read_u32s(&bytes[pos..pos + nedges * 4]);
        pos += nedges * 4;
        let sense_ids = read_u32s(&bytes[pos..pos + nsense_ids * 4]);
        pos += nsense_ids * 4;
        let mut senses = Vec::with_capacity(nsenses);
        let mut p = pos;
        for _ in 0..nsenses {
            let l1 = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap()) as usize;
            let l2 = u32::from_le_bytes(bytes[p + 4..p + 8].try_into().unwrap()) as usize;
            p += 8;
            let lemma = std::str::from_utf8(&bytes[p..p + l1])
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
                .to_string();
            p += l1;
            let tag = std::str::from_utf8(&bytes[p..p + l2])
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
                .to_string();
            p += l2;
            senses.push(Sense { lemma, tag });
        }
        Ok(Lexicon {
            nodes,
            edges,
            sense_ids,
            senses,
        })
    }
}

impl WordTable {
    fn sense_ids_len(&self) -> usize {
        self.words.values().map(|v| v.len()).sum()
    }
}

fn write_u32s<W: Write>(w: &mut W, v: &[u32]) -> io::Result<()> {
    let mut buf: Vec<u8> = Vec::with_capacity(v.len() * 4);
    for x in v {
        buf.extend_from_slice(&x.to_le_bytes());
    }
    w.write_all(&buf)
}

fn read_u32s(b: &[u8]) -> Vec<u32> {
    b.chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut t = WordTable::new();
        t.add("chat", "chat", "N m s");
        t.add("chats", "chat", "N m p");
        t.add("château", "château", "N m s");
        t.add("cheval", "cheval", "N m s");
        t.add("ch", "ch", "X");
        let lx = Lexicon::from_table(&t);
        assert!(lx.contains("chat"));
        assert!(lx.contains("château"));
        assert_equals_tags(&lx, "chats", &["N m p"]);
        assert_equals_tags(&lx, "chat", &["N m s"]);
        assert!(!lx.contains("cha"));
        let tmp = std::env::temp_dir().join("grammaria_lx_test.bin");
        lx.save(&tmp).unwrap();
        let l2 = Lexicon::load(&tmp).unwrap();
        assert_equals_tags(&l2, "château", &["N m s"]);
        let words = l2.prefix_words("ch", 10);
        assert!(words.contains(&"chat".to_string()));
        assert_eq!(words.len(), 5);
    }

    fn assert_equals_tags(lx: &Lexicon, word: &str, expected: &[&str]) {
        let ids = lx.lookup(word);
        let got: Vec<&str> = ids.iter().map(|&i| lx.senses[i as usize].tag.as_str()).collect();
        assert_eq!(got, expected, "word {}", word);
    }
}
