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
/// v3: `first`/`count` replaced by a bitset of sense-bearing slots plus a
/// dense `(first,count)` table indexed by the slot's rank among them.
///
/// The v2 layout spent 8 bytes per *slot* on those two arrays while only 27 %
/// of slots (475 097 of 1 777 184) carry readings; the dense form costs
/// `nslots/8` for the bitset plus 8 bytes per bearing slot.
pub const MAGIC_V3: &[u8; 8] = b"GRMLXIC3";

/// Prefix-sum granularity of the v3 slot->rank index, in slots (bits).
const RANK_STRIDE: usize = 1024;

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
///
/// Storage is either the owning form used while building (`Owned`) or a raw
/// file image loaded into memory (`Raw`). In `Raw` mode every array is read
/// straight out of the bytes, so loading is one `read()` with no decode pass —
/// that removes ~22 ms from every start-up for the 37 MB French lexicon.
pub struct Lexicon {
    kind: Storage,
}

enum Storage {
    Owned(OwnedLexicon),
    Raw(RawLexicon),
}

/// Build-time representation (dense arrays).
struct OwnedLexicon {
    base: Vec<u32>,
    check: Vec<u32>,
    first: Vec<u32>,
    count: Vec<u32>,
    sense_ids: Vec<u32>,
    senses: Vec<Sense>,
    lemmas: Vec<String>,
    tags: Vec<String>,
}

/// Loaded representation: byte offsets into the file image.
struct RawLexicon {
    bytes: Vec<u8>,
    nslots: usize,
    base_off: usize,
    check_off: usize,
    /// v2 only: direct per-slot arrays.
    first_off: usize,
    count_off: usize,
    /// v3: bitset of slots that carry readings, and a dense `(first,count)`
    /// table indexed by the slot's rank among those.
    v3: bool,
    bits_off: usize,
    dense_off: usize,
    /// v3: u32 prefix sums every `RANK_STRIDE` slots.
    rank_off: usize,
    sense_ids_off: usize,
    nsense_ids: usize,
    senses_off: usize,
    nsenses: usize,
    lemmas_off: usize,
    nlemmas: usize,
    tags_off: usize,
    ntags: usize,
}

impl RawLexicon {
    fn from_bytes(bytes: Vec<u8>) -> io::Result<RawLexicon> {
        let is_v3 = bytes.len() >= 28 && &bytes[..8] == MAGIC_V3;
        if !is_v3 && (bytes.len() < 28 || &bytes[..8] != MAGIC) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad lexicon magic"));
        }
        let meta = |i: usize| read_u32(&bytes, 8 + i * 4) as usize;
        let nslots = meta(0);
        let nsense_ids = meta(1);
        let nsenses = meta(2);
        let nlemmas = meta(3);
        let ntags = meta(4);
        let base_off = 28;
        let check_off = base_off + nslots * 4;
        if is_v3 {
            let bits_off = check_off + nslots * 4;
            let bits_len = (nslots + 7) / 8;
            let nblocks = nslots / RANK_STRIDE + 1;
            let rank_off = bits_off + bits_len;
            let dense_off = rank_off + nblocks * 4;
            // The dense table has one 8-byte entry per *sense-bearing* slot,
            // not per slot: count them from the bitset.
            let mut bearing = 0usize;
            for b in 0..bits_len {
                bearing += bytes[bits_off + b].count_ones() as usize;
            }
            let sense_ids_off = dense_off + bearing * 8;
            let senses_off = sense_ids_off + nsense_ids * 4;
            let lemmas_off = senses_off + nsenses * 8;
            let lemmas_blob = lemmas_off + (nlemmas + 1) * 4;
            let blob_len = read_u32(&bytes, lemmas_off + nlemmas * 4) as usize;
            let tags_off = lemmas_blob + blob_len;
            return Ok(RawLexicon {
                bytes, nslots, base_off, check_off,
                first_off: 0, count_off: 0,
                v3: true, bits_off, dense_off, rank_off,
                sense_ids_off, nsense_ids, senses_off, nsenses,
                lemmas_off, nlemmas, tags_off, ntags,
            });
        }
        let first_off = check_off + nslots * 4;
        let count_off = first_off + nslots * 4;
        let sense_ids_off = count_off + nslots * 4;
        let senses_off = sense_ids_off + nsense_ids * 4;
        let lemmas_off = senses_off + nsenses * 8;
        let lemmas_blob = lemmas_off + (nlemmas + 1) * 4;
        let blob_len = if nlemmas < nlemmas + 1 {
            read_u32(&bytes, lemmas_off + nlemmas * 4) as usize
        } else {
            0
        };
        let tags_off = lemmas_blob + blob_len;
        if bytes.len() < tags_off + (ntags + 1) * 4 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated lexicon"));
        }
        Ok(RawLexicon {
            bytes,
            nslots,
            base_off,
            check_off,
            first_off,
            count_off,
            v3: false,
            bits_off: 0,
            dense_off: 0,
            rank_off: 0,
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

    #[inline]
    fn u32_at(&self, off: usize) -> u32 {
        read_u32(&self.bytes, off)
    }

    #[inline]
    fn base(&self, slot: usize) -> u32 {
        self.u32_at(self.base_off + slot * 4)
    }

    #[inline]
    fn check(&self, slot: usize) -> u32 {
        self.u32_at(self.check_off + slot * 4)
    }

    /// Is bit `slot` set in the v3 bitset?
    #[inline]
    fn bit(&self, slot: usize) -> bool {
        let byte = self.bytes[self.bits_off + slot / 8];
        byte & (1 << (slot % 8)) != 0
    }

    /// Number of sense-bearing slots before `slot` (its rank in the dense
    /// table).
    ///
    /// `rank_base` holds a prefix sum every `RANK_STRIDE` bits, so this costs
    /// at most `RANK_STRIDE/8` byte loads instead of a scan from slot 0.
    fn rank(&self, slot: usize) -> u32 {
        let words_per_block = RANK_STRIDE / 8;
        let block = slot / RANK_STRIDE;
        let mut n = self.u32_at(self.rank_off + block * 4);
        let start = block * words_per_block;
        let end = slot / 8;
        for b in start..end {
            n += self.bytes[self.bits_off + b].count_ones();
        }
        let rest = slot % 8;
        if rest > 0 {
            let byte = self.bytes[self.bits_off + end];
            n += (byte & ((1u16 << rest) - 1) as u8).count_ones();
        }
        n
    }

    #[inline]
    fn first(&self, slot: usize) -> u32 {
        if self.v3 {
            let r = self.rank(slot) as usize;
            self.u32_at(self.dense_off + r * 8)
        } else {
            self.u32_at(self.first_off + slot * 4)
        }
    }

    #[inline]
    fn count(&self, slot: usize) -> u32 {
        if self.v3 {
            if !self.bit(slot) {
                return 0;
            }
            let r = self.rank(slot) as usize;
            self.u32_at(self.dense_off + r * 8 + 4)
        } else {
            self.u32_at(self.count_off + slot * 4)
        }
    }

    #[inline]
    fn sense_id(&self, i: usize) -> u32 {
        self.u32_at(self.sense_ids_off + i * 4)
    }

    #[inline]
    fn sense(&self, id: usize) -> (u32, u32) {
        let o = self.senses_off + id * 8;
        (self.u32_at(o), self.u32_at(o + 4))
    }

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

    #[inline]
    fn lemma(&self, id: u32) -> &str {
        self.string(self.lemmas_off, self.nlemmas, id as usize)
    }

    #[inline]
    fn tag(&self, id: u32) -> &str {
        self.string(self.tags_off, self.ntags, id as usize)
    }
}

impl Lexicon {
    pub fn sense_count(&self) -> usize {
        match &self.kind {
            Storage::Owned(o) => o.senses.len(),
            Storage::Raw(r) => r.nsenses,
        }
    }

    pub fn tag_count(&self) -> usize {
        match &self.kind {
            Storage::Owned(o) => o.tags.len(),
            Storage::Raw(r) => r.ntags,
        }
    }

    pub fn lemma(&self, id: u32) -> &str {
        match &self.kind {
            Storage::Owned(o) => &o.lemmas[id as usize],
            Storage::Raw(r) => r.lemma(id),
        }
    }

    pub fn tag(&self, id: u32) -> &str {
        match &self.kind {
            Storage::Owned(o) => &o.tags[id as usize],
            Storage::Raw(r) => r.tag(id),
        }
    }

    pub fn sense(&self, id: u32) -> (u32, u32) {
        match &self.kind {
            Storage::Owned(o) => (o.senses[id as usize].lemma, o.senses[id as usize].tag),
            Storage::Raw(r) => r.sense(id as usize),
        }
    }

    /// Sense ids for `word` (empty slice if unknown).
    pub fn sense_ids(&self, word: &str) -> Vec<u32> {
        match self.find_state(word) {
            Some(slot) => match &self.kind {
                Storage::Owned(o) => {
                    let f = o.first[slot] as usize;
                    let n = o.count[slot] as usize;
                    o.sense_ids[f..f + n].to_vec()
                }
                Storage::Raw(r) => {
                    let f = r.first(slot) as usize;
                    let n = r.count(slot) as usize;
                    (0..n).filter_map(|k| r.sense_id(f + k).into()).collect()
                }
            },
            None => Vec::new(),
        }
    }

    /// True if `word` is a known word form.
    pub fn contains(&self, word: &str) -> bool {
        self.find_state(word).is_some()
    }

    /// Tags for a word, appended to `out`.
    pub fn tags<'s>(&'s self, word: &str, out: &mut Vec<&'s str>) {
        out.clear();
        for sid in self.sense_ids(word) {
            out.push(self.tag(self.sense(sid).1));
        }
    }

    /// `(lemma, tag)` pairs for a word, appended to `out`.
    pub fn morphs<'s>(&'s self, word: &str, out: &mut Vec<(&'s str, &'s str)>) {
        out.clear();
        for sid in self.sense_ids(word) {
            let (l, t) = self.sense(sid);
            out.push((self.lemma(l), self.tag(t)));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.sense_count() == 0
    }

    pub fn word_count(&self) -> usize {
        match &self.kind {
            Storage::Owned(o) => o.count.iter().filter(|&&c| c > 0).count(),
            Storage::Raw(_) => {
                // Number of trie slots that carry readings.
                let n = self.nslots();
                (0..n).filter(|&s| self.count_at(s) > 0).count()
            }
        }
    }

    fn nslots(&self) -> usize {
        match &self.kind {
            Storage::Owned(o) => o.base.len(),
            Storage::Raw(r) => r.nslots,
        }
    }

    #[inline]
    fn count_at(&self, slot: usize) -> u32 {
        match &self.kind {
            Storage::Owned(o) => o.count[slot],
            Storage::Raw(r) => r.count(slot),
        }
    }

    fn find_state(&self, word: &str) -> Option<usize> {
        let nslots = self.nslots();
        if nslots == 0 {
            return None;
        }
        let mut s = 1usize; // root slot
        for &b in word.as_bytes() {
            let base = match &self.kind {
                Storage::Owned(o) => o.base[s] as usize,
                Storage::Raw(r) => r.base(s) as usize,
            };
            if base == 0 {
                return None;
            }
            let next = base + b as usize;
            if next >= nslots {
                return None;
            }
            let owner = match &self.kind {
                Storage::Owned(o) => o.check[next] as usize,
                Storage::Raw(r) => r.check(next) as usize,
            };
            if owner != s {
                return None;
            }
            s = next;
        }
        if self.count_at(s) > 0 {
            Some(s)
        } else {
            None
        }
    }

    /// All words starting with `prefix` (bounded by `limit`).
    pub fn prefix_words(&self, prefix: &str, limit: usize) -> Vec<String> {
        let nslots = self.nslots();
        // Walk down to the node for `prefix`.
        let mut s = 1usize;
        for &b in prefix.as_bytes() {
            let base = match &self.kind {
                Storage::Owned(o) => o.base[s] as usize,
                Storage::Raw(r) => r.base(s) as usize,
            };
            if base == 0 {
                return Vec::new();
            }
            let next = base + b as usize;
            if next >= nslots {
                return Vec::new();
            }
            let owner = match &self.kind {
                Storage::Owned(o) => o.check[next] as usize,
                Storage::Raw(r) => r.check(next) as usize,
            };
            if owner != s {
                return Vec::new();
            }
            s = next;
        }
        let mut out = Vec::new();
        let mut buf: Vec<u8> = prefix.as_bytes().to_vec();
        self.collect_prefix(s, &mut buf, &mut out, limit);
        out
    }

    fn collect_prefix(&self, node: usize, buf: &mut Vec<u8>, out: &mut Vec<String>, limit: usize) {
        if out.len() >= limit {
            return;
        }
        if self.count_at(node) > 0 {
            if let Ok(w) = std::str::from_utf8(buf) {
                out.push(w.to_string());
            }
        }
        // Scan labels; `base + label` addressing means labels are discovered by
        // probing `check[]`, so we scan the candidate window.
        let base = match &self.kind {
            Storage::Owned(o) => o.base[node] as usize,
            Storage::Raw(r) => r.base(node) as usize,
        };
        if base == 0 {
            return;
        }
        let nslots = self.nslots();
        let mut children: Vec<(u8, usize)> = Vec::new();
        // The trie only stores ASCII/Latin UTF-8 bytes (0..=255).
        let mut label = 1usize;
        while label <= 255 {
            let slot = base + label;
            if slot >= nslots {
                break;
            }
            let owner = match &self.kind {
                Storage::Owned(o) => o.check[slot] as usize,
                Storage::Raw(r) => r.check(slot) as usize,
            };
            if owner == node {
                children.push((label as u8, slot));
            }
            label += 1;
        }
        for (b, slot) in children {
            let len = buf.len();
            buf.push(b);
            self.collect_prefix(slot, buf, out, limit);
            buf.truncate(len);
            if out.len() >= limit {
                return;
            }
        }
    }
}

impl Lexicon {
    // ----- construction ---------------------------------------------------
    /// Build the double-array trie from the plain table.
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

        // Double-array placement over a slot space with headroom.
        let nstates = nodes.len() * 2 + 4096;
        let mut base = vec![0u32; nstates];
        let mut check = vec![0u32; nstates];
        let mut used = vec![false; nstates];
        used[0] = true; // slot 0 reserved
        let mut prev = vec![0u32; nstates];
        let mut next = vec![0u32; nstates];
        for i in 0..nstates {
            prev[i] = i.saturating_sub(1) as u32;
            next[i] = (i + 1).min(nstates - 1) as u32;
        }
        let mut first_free: usize = 1;
        let remove = |slot: usize, prev: &mut Vec<u32>, next: &mut Vec<u32>, first_free: &mut usize| {
            let p = prev[slot] as usize;
            let n = next[slot] as usize;
            next[p] = n as u32;
            prev[n] = p as u32;
            if *first_free == slot {
                *first_free = n;
            }
        };
        // Breadth-first state numbering keeps slots compact.
        let root = 0usize;
        let mut order: Vec<usize> = vec![root];
        let mut i = 0;
        while i < order.len() {
            let n = order[i];
            for &(_, c) in &nodes[n].children {
                order.push(c);
            }
            i += 1;
        }
        let mut slot_of = vec![usize::MAX; nodes.len()];
        // Root lives at slot 1.
        slot_of[root] = 1;
        let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
        queue.push_back(root);
        remove(1, &mut prev, &mut next, &mut first_free);
        while let Some(node) = queue.pop_front() {
            let mut labels: Vec<u8> = nodes[node].children.iter().map(|&(l, _)| l).collect();
            if labels.is_empty() {
                continue;
            }
            labels.sort_unstable();
            let max_label = *labels.last().unwrap() as usize;
            let mut b = first_free;
            loop {
                if b + max_label >= nstates {
                    panic!("double-array trie out of space (nstates={nstates})");
                }
                if !used[b] && labels.iter().all(|&l| !used[b + l as usize]) {
                    break;
                }
                b += 1;
            }
            base[slot_of[node]] = b as u32;
            used[b] = true;
            for &l in &labels {
                let slot = b + l as usize;
                used[slot] = true;
                check[slot] = slot_of[node] as u32;
            }
            // remove consumed slots from the free list
            let rm = |sl: usize, prev: &mut Vec<u32>, next: &mut Vec<u32>, first_free: &mut usize| {
                if sl == 0 || sl >= nstates {
                    return;
                }
                let p = prev[sl] as usize;
                let n = next[sl] as usize;
                if next[p] as usize == sl {
                    next[p] = n as u32;
                    prev[n] = p as u32;
                }
                if *first_free == sl {
                    *first_free = n.min(nstates - 1);
                }
            };
            rm(b, &mut prev, &mut next, &mut first_free);
            for &l in &labels {
                rm(b + l as usize, &mut prev, &mut next, &mut first_free);
            }
            while first_free < nstates && used[first_free] {
                first_free = next[first_free] as usize;
                if first_free >= nstates {
                    break;
                }
            }
            for &(l, child) in &nodes[node].children {
                slot_of[child] = b + l as usize;
                queue.push_back(child);
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
            kind: Storage::Owned(OwnedLexicon {
                base,
                check,
                first,
                count,
                sense_ids,
                senses: t.senses.clone(),
                lemmas: t.lemmas.clone(),
                tags: t.tags.clone(),
            }),
        }
    }

    /// Drop trailing unused slots so the on-disk image is minimal.
    pub fn compact(mut self) -> Lexicon {
        if let Storage::Owned(o) = &mut self.kind {
            let mut n = o.base.len();
            while n > 2 && o.check[n - 1] == 0 && o.count[n - 1] == 0 && o.base[n - 1] == 0 {
                n -= 1;
            }
            o.base.truncate(n);
            o.check.truncate(n);
            o.first.truncate(n);
            o.count.truncate(n);
        }
        self
    }

    // ----- serialization -------------------------------------------------
    pub fn save<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let o = match &self.kind {
            Storage::Owned(o) => o,
            Storage::Raw(_) => {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot save a loaded lexicon"))
            }
        };
        let tmp = path.as_ref().with_extension("tmp");
        {
            let f = fs::File::create(&tmp)?;
            let mut w = io::BufWriter::with_capacity(1 << 20, f);
            w.write_all(MAGIC_V3)?;
            let nslots = o.base.len();
            let meta = [
                nslots as u32,
                o.sense_ids.len() as u32,
                o.senses.len() as u32,
                o.lemmas.len() as u32,
                o.tags.len() as u32,
            ];
            write_u32s(&mut w, &meta)?;
            write_u32s(&mut w, &o.base)?;
            write_u32s(&mut w, &o.check)?;
            // v3: a bitset of slots carrying readings, a prefix-sum index over
            // it, then a dense (first, count) table for those slots only.
            let mut bits = vec![0u8; (nslots + 7) / 8];
            for (slot, &c) in o.count.iter().enumerate() {
                if c != 0 {
                    bits[slot / 8] |= 1 << (slot % 8);
                }
            }
            w.write_all(&bits)?;
            let nblocks = nslots / RANK_STRIDE + 1;
            let mut rank = vec![0u32; nblocks];
            let mut running = 0u32;
            for b in 0..nblocks {
                rank[b] = running;
                let end = ((b + 1) * RANK_STRIDE / 8).min(bits.len());
                let start = (b * RANK_STRIDE / 8).min(bits.len());
                for byte in &bits[start..end] {
                    running += byte.count_ones();
                }
            }
            write_u32s(&mut w, &rank)?;
            let dense: Vec<u8> = o
                .count
                .iter()
                .enumerate()
                .filter(|(_, &c)| c != 0)
                .flat_map(|(slot, &c)| {
                    let mut v = [0u8; 8];
                    v[..4].copy_from_slice(&o.first[slot].to_le_bytes());
                    v[4..].copy_from_slice(&c.to_le_bytes());
                    v
                })
                .collect();
            w.write_all(&dense)?;
            write_u32s(&mut w, &o.sense_ids)?;
            let mut sb = Vec::with_capacity(o.senses.len() * 8);
            for s in &o.senses {
                sb.extend_from_slice(&s.lemma.to_le_bytes());
                sb.extend_from_slice(&s.tag.to_le_bytes());
            }
            w.write_all(&sb)?;
            write_strings(&mut w, &o.lemmas)?;
            write_strings(&mut w, &o.tags)?;
            w.flush()?;
        }
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Load a lexicon file. The bytes are kept as-is: no decode pass.
    pub fn load<P: AsRef<Path>>(path: P) -> io::Result<Lexicon> {
        let bytes = fs::read(path)?;
        Ok(Lexicon {
            kind: Storage::Raw(RawLexicon::from_bytes(bytes)?),
        })
    }

    /// Build a lexicon from an owned file image (no copy).
    pub fn from_vec(bytes: Vec<u8>) -> io::Result<Lexicon> {
        Ok(Lexicon {
            kind: Storage::Raw(RawLexicon::from_bytes(bytes)?),
        })
    }

    /// Build a lexicon from a borrowed file image (copies the bytes).
    pub fn from_bytes(bytes: &[u8]) -> io::Result<Lexicon> {
        Lexicon::from_vec(bytes.to_vec())
    }
}

fn write_u32s<W: Write>(w: &mut W, v: &[u32]) -> io::Result<()> {
    let mut buf: Vec<u8> = Vec::with_capacity(v.len() * 4);
    for x in v {
        buf.extend_from_slice(&x.to_le_bytes());
    }
    w.write_all(&buf)
}

fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
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

/// Process-wide lexicon handle, installed by the checker so that helper code
/// (e.g. rule filters that synthesise corrected forms) can validate words.
static GLOBAL: std::sync::OnceLock<&'static Lexicon> = std::sync::OnceLock::new();

/// Install the process-wide lexicon (idempotent).
pub fn set_global_lexicon(lx: &'static Lexicon) {
    let _ = GLOBAL.set(lx);
}

/// The process-wide lexicon, if installed.
pub fn global_lexicon() -> Option<&'static Lexicon> {
    GLOBAL.get().copied()
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
