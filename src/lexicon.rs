//! Word forms, part-of-speech tags and the lexicon that stores them.
//!
//! The lexicon maps every word form to a list of *senses* (`lemma`, `tags`),
//! e.g. `vais -> (aller, "V ind pres 1 s")`. The French dictionary has ~475k
//! forms and ~616k unique senses, so storage layout matters:
//!
//! * words are stored in a double-array trie over UTF-8 bytes,
//! * senses are interned (unique `(lemma, tag)` pairs are stored once),
//! * `lemma` and `tag` strings are interned separately too, because the tag
//!   vocabulary is tiny (~2 000 strings) while lemmas number ~100k,
//! * the per-word sense lists live in one contiguous `u32` array.
//!
//! The file format is little-endian `u32` blocks, so loading is one pass of
//! conversion and lookups are allocation-free.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

pub const MAGIC: &[u8; 8] = b"GRMLXIC2";

/// A word sense: interned lemma id + interned tag id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sense {
    pub lemma: u32,
    pub tag: u32,
}

/// Plain, in-memory collection of words while the dictionary is being built.
#[derive(Default)]
pub struct WordTable {
    lemmas: Vec<String>,
    lemma_index: std::collections::HashMap<String, u32>,
    tags: Vec<String>,
    tag_index: std::collections::HashMap<String, u32>,
    senses: Vec<Sense>,
    sense_index: std::collections::HashMap<(u32, u32), u32>,
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

    fn intern_lemma(&mut self, lemma: &str) -> u32 {
        if let Some(&i) = self.lemma_index.get(lemma) {
            return i;
        }
        let i = self.lemmas.len() as u32;
        self.lemmas.push(lemma.to_string());
        self.lemma_index.insert(lemma.to_string(), i);
        i
    }

    fn intern_tag(&mut self, tag: &str) -> u32 {
        if let Some(&i) = self.tag_index.get(tag) {
            return i;
        }
        let i = self.tags.len() as u32;
        self.tags.push(tag.to_string());
        self.tag_index.insert(tag.to_string(), i);
        i
    }

    /// Add a word form with an associated sense. Returns the sense id.
    pub fn add(&mut self, word: &str, lemma: &str, tag: &str) -> u32 {
        let l = self.intern_lemma(lemma);
        let t = self.intern_tag(tag);
        let sid = match self.sense_index.get(&(l, t)) {
            Some(&id) => id,
            None => {
                let id = self.senses.len() as u32;
                self.senses.push(Sense { lemma: l, tag: t });
                self.sense_index.insert((l, t), id);
                id
            }
        };
        let list = self.words.entry(word.to_string()).or_default();
        if !list.contains(&sid) {
            list.push(sid);
        }
        sid
    }

    pub fn add_many<'a, I: IntoIterator<Item = &'a str>>(&mut self, words: I, lemma: &str, tag: &str) {
        for w in words {
            self.add(w, lemma, tag);
        }
    }

    pub fn contains(&self, word: &str) -> bool {
        self.words.contains_key(word)
    }

    pub fn sense_count_hint(&self) -> usize {
        self.senses.len()
    }

    pub fn tag_count_hint(&self) -> usize {
        self.tags.len()
    }
}

/// Serialized (read-only) lexicon.
pub struct Lexicon {
    /// Double-array trie: `slot = base[state] ^ label`; `check[slot] == state`
    /// means the slot belongs to `state`.
    base: Vec<u32>,
    check: Vec<u32>,
    /// Per trie state: range into `sense_ids`.
    first: Vec<u32>,
    count: Vec<u32>,
    sense_ids: Vec<u32>,
    senses: Vec<Sense>,
    lemmas: Vec<String>,
    tags: Vec<String>,
}

impl Lexicon {
    pub fn debug_dump_compact(&self, word: &str) {
        let mut s = 1usize;
        print!("traverse {}: ", word);
        for &b in word.as_bytes() {
            match self.transition(s, b) {
                Some(n) => { print!("+{}={} ", b as char, n); s = n; }
                None => { print!("FAIL at {} (base {}, check {})", b as char, self.base[s], self.check.get(s).copied().unwrap_or(0)); break; }
            }
        }
        println!(" state {} count {}", s, self.count[s]);
        for (i, (&b, &c)) in self.base.iter().zip(self.check.iter()).enumerate() {
            if c != 0 || i == 1 { println!("slot {} base {} check {} first {} count {}", i, b, c, self.first[i], self.count[i]); }
        }
    }

    pub fn check_at(&self, i: usize) -> u32 { self.check[i] }

    pub fn debug_dump(&self) {
        println!("base len {} check len {}", self.base.len(), self.check.len());
        for (i, (&b, &c)) in self.base.iter().zip(self.check.iter()).enumerate() { if c != 0 || b != 0 { println!("slot {} base {} check {}", i, b, c); } }
        for (i, (&b, &c)) in self.base.iter().zip(self.check.iter()).enumerate() {
            println!("slot {} base {} check {} first {} count {}", i, b, c, self.first[i], self.count[i]);
        }
    }

    pub fn sense_count(&self) -> usize {
        self.senses.len()
    }

    pub fn tag_count(&self) -> usize {
        self.tags.len()
    }

    pub fn lemma(&self, id: u32) -> &str {
        &self.lemmas[id as usize]
    }

    pub fn tag(&self, id: u32) -> &str {
        &self.tags[id as usize]
    }

    pub fn sense(&self, id: u32) -> (u32, u32) {
        let s = self.senses[id as usize];
        (s.lemma, s.tag)
    }

    /// All sense ids for a word (empty slice if unknown).
    pub fn lookup(&self, word: &str) -> &[u32] {
        match self.find_state(word) {
            Some(s) => {
                let f = self.first[s] as usize;
                let n = self.count[s] as usize;
                &self.sense_ids[f..f + n]
            }
            None => &[],
        }
    }

    /// True if `word` is a known word form.
    pub fn contains(&self, word: &str) -> bool {
        !self.lookup(word).is_empty()
    }

    /// Tags for a word, appended to `out`.
    pub fn tags<'s>(&'s self, word: &str, out: &mut Vec<&'s str>) {
        out.clear();
        for &sid in self.lookup(word) {
            out.push(self.tag(self.senses[sid as usize].tag));
        }
    }

    /// `(lemma, tag)` pairs for a word, appended to `out`.
    pub fn morphs<'s>(&'s self, word: &str, out: &mut Vec<(&'s str, &'s str)>) {
        out.clear();
        for &sid in self.lookup(word) {
            let s = &self.senses[sid as usize];
            out.push((self.lemma(s.lemma), self.tag(s.tag)));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.senses.is_empty()
    }

    pub fn word_count(&self) -> usize {
        self.count.iter().filter(|&&c| c > 0).count()
    }

    fn find_state(&self, word: &str) -> Option<usize> {
        let mut s = 1usize; // root slot
        for &b in word.as_bytes() {
            let next = self.transition(s, b)?;
            s = next;
        }
        if self.count[s] > 0 {
            Some(s)
        } else {
            None
        }
    }

    #[inline]
    fn transition(&self, s: usize, label: u8) -> Option<usize> {
        let base = self.base[s] as usize;
        if base == 0 {
            return None;
        }
        let next = base + label as usize;
        if next < self.check.len() && self.check[next] as usize == s {
            Some(next)
        } else {
            None
        }
    }

    /// All words starting with `prefix` (bounded by `limit`).
    pub fn prefix_words(&self, prefix: &str, limit: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut s = 1usize;
        for &b in prefix.as_bytes() {
            match self.transition(s, b) {
                Some(n) => s = n,
                None => return out,
            }
        }
        let mut buf = Vec::from(prefix.as_bytes());
        self.collect(s, &mut buf, &mut out, limit);
        out
    }

    fn collect(&self, s: usize, buf: &mut Vec<u8>, out: &mut Vec<String>, limit: usize) {
        if out.len() >= limit {
            return;
        }
        if self.count[s] > 0 {
            if let Ok(w) = std::str::from_utf8(buf) {
                out.push(w.to_string());
            }
        }
        let base = self.base[s] as usize;
        if base == 0 {
            return;
        }
        for label in 1u8..=255 {
            let next = base + label as usize;
            if next < self.check.len() && self.check[next] as usize == s {
                buf.push(label);
                self.collect(next, buf, out, limit);
                buf.pop();
                if out.len() >= limit {
                    return;
                }
            }
        }
    }

    // ----- construction ---------------------------------------------------

    /// Build the double-array trie from the plain table.
    ///
    /// A state's children live at `base[state] + label`. Bases are stored per
    /// *slot*; the slot of a child is fixed by its parent's base, so bases are
    /// chosen in one breadth-first pass while walking the plain trie.
    pub fn from_table(t: &WordTable) -> Lexicon {
        #[derive(Default)]
        struct N {
            children: Vec<(u8, usize)>,
            senses: Vec<u32>,
        }
        let mut nodes: Vec<N> = vec![N::default()];
        for (word, sids) in t.words.iter() {
            let mut cur = 0usize;
            for &b in word.as_bytes() {
                let pos = nodes[cur].children.iter().position(|&(l, _)| l == b);
                let next = match pos {
                    Some(p) => nodes[cur].children[p].1,
                    None => {
                        let id = nodes.len();
                        nodes.push(N::default());
                        nodes[cur].children.push((b, id));
                        id
                    }
                };
                cur = next;
            }
            nodes[cur].senses = sids.clone();
        }
        for n in nodes.iter_mut() {
            n.children.sort_unstable();
        }

        // Slot space with headroom for the double-array layout.
        //
        // Placement uses the classic "free list" technique: free slots form a
        // doubly linked list, and candidate bases are drawn from
        // `first_free_label` (the slot labelled with the state's first child
        // label). This makes construction effectively linear.
        let nstates = nodes.len() * 2 + 4096;
        let mut base = vec![0u32; nstates];
        let mut check = vec![0u32; nstates];
        let mut used = vec![false; nstates];
        used[0] = true; // slot 0 reserved
        // free list links (only meaningful while a slot is free)
        let mut prev = vec![0u32; nstates];
        let mut next = vec![0u32; nstates];
        for i in 0..nstates {
            prev[i] = i.saturating_sub(1) as u32;
            next[i] = (i + 1).min(nstates - 1) as u32;
        }
        let mut first_free: usize = 1;
        // Remove a free slot from the list.
        let remove = |slot: usize, prev: &mut Vec<u32>, next: &mut Vec<u32>, first_free: &mut usize| {
            let p = prev[slot] as usize;
            let n = next[slot] as usize;
            next[p] = n as u32;
            prev[n] = p as u32;
            if *first_free == slot {
                *first_free = n;
            }
        };

        let mut queue: std::collections::VecDeque<(usize, usize)> = std::collections::VecDeque::new();
        queue.push_back((0usize, 1usize));
        let mut slot_of: Vec<usize> = vec![usize::MAX; nodes.len()];
        slot_of[0] = 1;
        remove(1, &mut prev, &mut next, &mut first_free);
        let mut placed_states = 0usize;
        let dbg = std::env::var("GRM_DA_DEBUG").is_ok();
        while let Some((node, slot)) = queue.pop_front() {
            let labels: Vec<u8> = nodes[node].children.iter().map(|&(l, _)| l).collect();
            if labels.is_empty() {
                continue;
            }
            placed_states += 1;
            if dbg && placed_states % 50000 == 0 {
                eprintln!("placed {} states, first_free={}, nstates={}", placed_states, first_free, nstates);
            }
            // Candidate bases come from the first child label `labels[0]`:
            // base must be `free - labels[0]` for some free slot `free`.
            let l0 = labels[0] as usize;
            let mut cand = 0usize;
            let mut found = false;
            // Iterate free slots via the list, starting at first_free.
            let mut slot_iter = first_free;
            loop {
                if slot_iter >= nstates {
                    break;
                }
                if slot_iter >= l0 {
                    let b = slot_iter - l0;
                    if b >= 1 && b != slot && !used[b] {
                        let mut ok = true;
                        for &l in labels.iter().skip(1) {
                            let target = b + l as usize;
                            if target >= nstates || used[target] {
                                ok = false;
                                break;
                            }
                        }
                        if ok {
                            cand = b;
                            found = true;
                            break;
                        }
                    }
                }
                let nx = next[slot_iter] as usize;
                if nx <= slot_iter {
                    break;
                }
                slot_iter = nx;
            }
            if !found {
                panic!("double-array trie out of space (nstates={nstates})");
            }
            base[slot] = cand as u32;
            used[cand] = true;
            // consume target slots
            for &l in &labels {
                let target = cand + l as usize;
                used[target] = true;
                check[target] = slot as u32;
            }
            // rebuild the free list lazily: remove every consumed slot
            // (cand and all targets) if still in the list
            let mut remove_slot = |sl: usize, prev: &mut Vec<u32>, next: &mut Vec<u32>, first_free: &mut usize| {
                if sl == 0 || sl >= nstates {
                    return;
                }
                let p = prev[sl] as usize;
                let n = next[sl] as usize;
                // only unlink if still linked (prev points forward to sl)
                if n < nstates && next[p] as usize == sl {
                    next[p] = n as u32;
                    prev[n] = p as u32;
                }
                if *first_free == sl {
                    *first_free = n.min(nstates - 1);
                }
            };
            remove_slot(cand, &mut prev, &mut next, &mut first_free);
            for &l in &labels {
                remove_slot(cand + l as usize, &mut prev, &mut next, &mut first_free);
            }
            while first_free < nstates && used[first_free] {
                first_free = next[first_free] as usize;
                if first_free >= nstates {
                    break;
                }
            }
            for &(l, child) in &nodes[node].children {
                let cslot = cand + l as usize;
                slot_of[child] = cslot;
                queue.push_back((child, cslot));
            }
        }

        let mut first = vec![0u32; nstates];
        let mut count = vec![0u32; nstates];
        let mut sense_ids = Vec::new();
        for (node, &slot) in slot_of.iter().enumerate() {
            if slot == usize::MAX {
                continue;
            }
            if !nodes[node].senses.is_empty() {
                first[slot] = sense_ids.len() as u32;
                count[slot] = nodes[node].senses.len() as u32;
                sense_ids.extend_from_slice(&nodes[node].senses);
            }
        }
        Lexicon {
            base,
            check,
            first,
            count,
            sense_ids,
            senses: t.senses.clone(),
            lemmas: t.lemmas.clone(),
            tags: t.tags.clone(),
        }
    }

    // ----- serialization -------------------------------------------------
    pub fn save<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let tmp = path.as_ref().with_extension("tmp");
        {
            let f = fs::File::create(&tmp)?;
            let mut w = io::BufWriter::with_capacity(1 << 20, f);
            w.write_all(MAGIC)?;
            let meta = [
                self.base.len() as u32,
                self.sense_ids.len() as u32,
                self.senses.len() as u32,
                self.lemmas.len() as u32,
                self.tags.len() as u32,
            ];
            write_u32s(&mut w, &meta)?;
            write_u32s(&mut w, &self.base)?;
            write_u32s(&mut w, &self.check)?;
            write_u32s(&mut w, &self.first)?;
            write_u32s(&mut w, &self.count)?;
            write_u32s(&mut w, &self.sense_ids)?;
            let mut sb = Vec::with_capacity(self.senses.len() * 8);
            for s in &self.senses {
                sb.extend_from_slice(&s.lemma.to_le_bytes());
                sb.extend_from_slice(&s.tag.to_le_bytes());
            }
            w.write_all(&sb)?;
            write_strings(&mut w, &self.lemmas)?;
            write_strings(&mut w, &self.tags)?;
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
        if bytes.len() < 28 || &bytes[..8] != MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad lexicon magic"));
        }
        let meta = read_u32s(&bytes[8..28]);
        let nbase = meta[0] as usize;
        let nsense_ids = meta[1] as usize;
        let nsenses = meta[2] as usize;
        let nlemmas = meta[3] as usize;
        let ntags = meta[4] as usize;
        let mut p = 28usize;
        let mut take = |n: usize| -> Vec<u32> {
            let v = read_u32s(&bytes[p..p + n * 4]);
            p += n * 4;
            v
        };
        let base = take(nbase);
        let check = take(nbase);
        let first = take(nbase);
        let count = take(nbase);
        let sense_ids = take(nsense_ids);
        let mut senses = Vec::with_capacity(nsenses);
        for i in 0..nsenses {
            let l = u32::from_le_bytes(bytes[p + i * 8..p + i * 8 + 4].try_into().unwrap());
            let t = u32::from_le_bytes(bytes[p + i * 8 + 4..p + i * 8 + 8].try_into().unwrap());
            senses.push(Sense { lemma: l, tag: t });
        }
        p += nsenses * 8;
        let (lemmas, np) = read_strings(bytes, p, nlemmas)?;
        let (tags, _) = read_strings(bytes, np, ntags)?;
        Ok(Lexicon {
            base,
            check,
            first,
            count,
            sense_ids,
            senses,
            lemmas,
            tags,
        })
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
    b.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
}

fn write_strings<W: Write>(w: &mut W, v: &[String]) -> io::Result<()> {
    let mut blob: Vec<u8> = Vec::new();
    let mut offsets: Vec<u32> = Vec::with_capacity(v.len() + 1);
    for s in v {
        offsets.push(blob.len() as u32);
        blob.extend_from_slice(s.as_bytes());
    }
    offsets.push(blob.len() as u32);
    write_u32s(w, &offsets)?;
    w.write_all(&blob)
}

fn read_strings(bytes: &[u8], p: usize, n: usize) -> io::Result<(Vec<String>, usize)> {
    let offsets = read_u32s(&bytes[p..p + (n + 1) * 4]);
    let blob_start = p + (n + 1) * 4;
    let blob_len = *offsets.last().unwrap_or(&0) as usize;
    let blob = &bytes[blob_start..blob_start + blob_len];
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let a = offsets[i] as usize;
        let b = offsets[i + 1] as usize;
        let s = std::str::from_utf8(&blob[a..b])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
            .to_string();
        out.push(s);
    }
    Ok((out, blob_start + blob_len))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build() -> Lexicon {
        let mut t = WordTable::new();
        t.add("chat", "chat", "N m s");
        t.add("chats", "chat", "N m p");
        t.add("château", "château", "N m s");
        t.add("cheval", "cheval", "N m s");
        t.add("ch", "ch", "I");
        t.add("chat", "chat", "I"); // homograph
        Lexicon::from_table(&t)
    }

    fn tags_of<'a>(lx: &'a Lexicon, w: &str) -> Vec<&'a str> {
        let mut v = Vec::new();
        lx.tags(w, &mut v);
        v
    }

    #[test]
    fn lookup() {
        let lx = build();
        assert!(lx.contains("chat"));
        assert!(lx.contains("château"));
        assert!(!lx.contains("cha"));
        assert!(!lx.contains("chatte"));
        let mut tt = tags_of(&lx, "chat");
        tt.sort();
        assert_eq!(tt, vec!["I", "N m s"]);
        assert_eq!(tags_of(&lx, "chats"), vec!["N m p"]);
    }

    #[test]
    fn prefix() {
        let lx = build();
        let mut w = lx.prefix_words("ch", 100);
        w.sort();
        assert_eq!(w, vec!["ch", "chat", "chats", "cheval", "château"]);
    }

    #[test]
    fn roundtrip() {
        let lx = build();
        let tmp = std::env::temp_dir().join("grammaria_lx2.bin");
        lx.save(&tmp).unwrap();
        let l2 = Lexicon::load(&tmp).unwrap();
        let mut tt = tags_of(&l2, "chat");
        tt.sort();
        assert_eq!(tt, vec!["I", "N m s"]);
        assert_eq!(tags_of(&l2, "chats"), vec!["N m p"]);
        assert_eq!(l2.prefix_words("ch", 100).len(), 5);
    }
}
