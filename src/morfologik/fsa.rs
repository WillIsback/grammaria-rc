//! Finite state automata in Morfologik's binary formats.
//!
//! Port of `morfologik.fsa.FSA`, `FSA5`, `CFSA2`, `FSAFlags`, `FSAHeader`,
//! `FSATraversal`, `MatchResult` and `ByteSequenceIterator`
//! (Morfologik 2.1.9, BSD-3-Clause, © Dawid Weiss and Marcin Miłkowski).
//!
//! An automaton is a byte array. A *node* and an *arc* are both offsets into
//! that array (after the file header), exactly as in the Java code; offset `0`
//! means "no arc" for arcs and "terminal" for target nodes. The automaton is
//! immutable once read, so it is `Send + Sync` and can be shared freely.
//!
//! The file is trusted, as in Java: a corrupt automaton makes an index go out
//! of bounds, which panics here where Java throws
//! `ArrayIndexOutOfBoundsException`.

use std::io;

/// `FSAFlags`: features of an automaton, as bits of the CFSA2 header word.
pub mod flags {
    /// Daciuk: flexible.
    pub const FLEXIBLE: u16 = 1 << 0;
    /// Daciuk: stop bit in use.
    pub const STOPBIT: u16 = 1 << 1;
    /// Daciuk: next bit in use.
    pub const NEXTBIT: u16 = 1 << 2;
    /// Daciuk: tails compression.
    pub const TAILS: u16 = 1 << 3;
    /// Nodes carry the count of sequences below them (perfect hashing).
    pub const NUMBERS: u16 = 1 << 8;
    /// The automaton was built with separators.
    pub const SEPARATORS: u16 = 1 << 9;
    /// Every bit defined above.
    pub const ALL: u16 = FLEXIBLE | STOPBIT | NEXTBIT | TAILS | NUMBERS | SEPARATORS;
}

/// `FSAHeader.FSA_MAGIC`: the four bytes `\fsa`.
const FSA_MAGIC: [u8; 4] = [b'\\', b'f', b's', b'a'];

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// The operations of `morfologik.fsa.FSA` that the readers implement.
///
/// The algorithms of this module ([`match_sequence`], [`SequenceIter`]) are
/// generic over this trait, so they are compiled once per format without
/// dynamic dispatch.
pub trait Automaton {
    /// The identifier of the root node, `0` if the automaton is empty.
    fn root_node(&self) -> usize;
    /// The first arc leaving `node`, `0` if it has none.
    fn first_arc(&self, node: usize) -> usize;
    /// The arc after `arc` in the same node, `0` if `arc` is the last one.
    fn next_arc(&self, arc: usize) -> usize;
    /// The arc of `node` labelled `label`, `0` if there is none.
    fn arc(&self, node: usize, label: u8) -> usize;
    /// The label of `arc`.
    fn arc_label(&self, arc: usize) -> u8;
    /// True if `arc` ends a sequence stored in the automaton.
    fn is_arc_final(&self, arc: usize) -> bool;
    /// True if `arc` has no target node.
    fn is_arc_terminal(&self, arc: usize) -> bool;
    /// The node `arc` points to. Meaningless (`0`) for a terminal arc.
    fn end_node(&self, arc: usize) -> usize;
    /// The [`flags`] the automaton was built with.
    fn flags(&self) -> u16;
    /// The number of sequences reachable from `node`; `None` if the automaton
    /// was not compiled with [`flags::NUMBERS`].
    fn right_language_count(&self, node: usize) -> Option<usize>;

    /// `FSA.getArcCount`: the number of arcs leaving `node`.
    fn arc_count(&self, node: usize) -> usize {
        let mut count = 0;
        let mut arc = self.first_arc(node);
        while arc != 0 {
            count += 1;
            arc = self.next_arc(arc);
        }
        count
    }
}

// ---------------------------------------------------------------------------
// FSA5
// ---------------------------------------------------------------------------

/// Port of `morfologik.fsa.FSA5`: Jan Daciuk's FSA version 5 (the output of
/// `fsa_build` with the flexible, stop-bit and next-bit options).
pub struct Fsa5 {
    /// The automaton body (everything after the header).
    arcs: Vec<u8>,
    /// Size in bytes of the per-node sequence counter, `0` without NUMBERS.
    node_data_length: usize,
    /// Number of bytes of a goto field (flags included).
    gtl: usize,
    /// Filler byte declared in the header (unused by lookups).
    pub filler: u8,
    /// Annotation byte declared in the header (unused by lookups).
    pub annotation: u8,
    flags: u16,
}

impl Fsa5 {
    /// Header version byte.
    pub const VERSION: u8 = 5;
    const BIT_FINAL_ARC: u8 = 1 << 0;
    const BIT_LAST_ARC: u8 = 1 << 1;
    const BIT_TARGET_NEXT: u8 = 1 << 2;
    const ADDRESS_OFFSET: usize = 1;

    /// Reads the part of the file that follows the magic and version bytes.
    fn read_body(body: &[u8]) -> io::Result<Fsa5> {
        if body.len() < 3 {
            return Err(invalid("Truncated FSA5 header.".to_string()));
        }
        let (filler, annotation, hgtl) = (body[0], body[1], body[2]);
        let mut fl = flags::FLEXIBLE | flags::STOPBIT | flags::NEXTBIT;
        if hgtl & 0xf0 != 0 {
            fl |= flags::NUMBERS;
        }
        Ok(Fsa5 {
            arcs: body[3..].to_vec(),
            node_data_length: usize::from(hgtl >> 4),
            gtl: usize::from(hgtl & 0x0f),
            filler,
            annotation,
            flags: fl,
        })
    }

    #[inline]
    fn is_arc_last(&self, arc: usize) -> bool {
        self.arcs[arc + Self::ADDRESS_OFFSET] & Self::BIT_LAST_ARC != 0
    }

    #[inline]
    fn is_next_set(&self, arc: usize) -> bool {
        self.arcs[arc + Self::ADDRESS_OFFSET] & Self::BIT_TARGET_NEXT != 0
    }

    /// `decodeFromBytes`: an `n`-byte little-endian integer.
    #[inline]
    fn decode_from_bytes(&self, start: usize, n: usize) -> usize {
        let mut r = 0usize;
        for i in (0..n).rev() {
            r = r << 8 | usize::from(self.arcs[start + i]);
        }
        r
    }

    #[inline]
    fn destination_node_offset(&self, arc: usize) -> usize {
        if self.is_next_set(arc) {
            // The target node follows this arc.
            self.skip_arc(arc)
        } else {
            // The address shares its low three bits with the arc flags.
            self.decode_from_bytes(arc + Self::ADDRESS_OFFSET, self.gtl) >> 3
        }
    }

    #[inline]
    fn skip_arc(&self, offset: usize) -> usize {
        offset
            + if self.is_next_set(offset) {
                1 + 1 // label + flags
            } else {
                1 + self.gtl // label + flags/address
            }
    }
}

impl Automaton for Fsa5 {
    fn root_node(&self) -> usize {
        // Skip the dummy node marking the terminating state.
        let epsilon_node = self.skip_arc(self.first_arc(0));
        // And follow the epsilon node's first (and only) arc.
        self.destination_node_offset(self.first_arc(epsilon_node))
    }

    #[inline]
    fn first_arc(&self, node: usize) -> usize {
        self.node_data_length + node
    }

    #[inline]
    fn next_arc(&self, arc: usize) -> usize {
        if self.is_arc_last(arc) {
            0
        } else {
            self.skip_arc(arc)
        }
    }

    #[inline]
    fn arc(&self, node: usize, label: u8) -> usize {
        let mut arc = self.first_arc(node);
        while arc != 0 {
            if self.arc_label(arc) == label {
                return arc;
            }
            arc = self.next_arc(arc);
        }
        0
    }

    #[inline]
    fn arc_label(&self, arc: usize) -> u8 {
        self.arcs[arc]
    }

    #[inline]
    fn is_arc_final(&self, arc: usize) -> bool {
        self.arcs[arc + Self::ADDRESS_OFFSET] & Self::BIT_FINAL_ARC != 0
    }

    #[inline]
    fn is_arc_terminal(&self, arc: usize) -> bool {
        self.destination_node_offset(arc) == 0
    }

    #[inline]
    fn end_node(&self, arc: usize) -> usize {
        self.destination_node_offset(arc)
    }

    fn flags(&self) -> u16 {
        self.flags
    }

    fn right_language_count(&self, node: usize) -> Option<usize> {
        if self.flags & flags::NUMBERS == 0 {
            return None;
        }
        Some(self.decode_from_bytes(node, self.node_data_length))
    }
}

// ---------------------------------------------------------------------------
// CFSA2
// ---------------------------------------------------------------------------

/// Port of `morfologik.fsa.CFSA2`: the compact automaton format, version 2
/// (header version byte `0xc6`).
///
/// An arc starts with a flags byte `N L F M M M M M`: target node follows the
/// last arc of this node (`N`), last arc of the node (`L`), final arc (`F`)
/// and a 5-bit index into the table of the 31 most frequent labels (`M`, `0`
/// meaning that the label is the next byte). Unless `N` is set, a
/// variable-length target address follows.
pub struct Cfsa2 {
    /// The automaton body (everything after the header and the label table).
    arcs: Vec<u8>,
    /// Labels for the indexed arcs; entry `0` is unused.
    label_mapping: [u8; 32],
    flags: u16,
    /// True if nodes are prefixed with the count of their sequences.
    has_numbers: bool,
}

impl Cfsa2 {
    /// Header version byte.
    pub const VERSION: u8 = 0xc6;
    const BIT_TARGET_NEXT: u8 = 1 << 7;
    const BIT_LAST_ARC: u8 = 1 << 6;
    const BIT_FINAL_ARC: u8 = 1 << 5;
    const LABEL_INDEX_MASK: u8 = (1 << 5) - 1;

    /// Reads the part of the file that follows the magic and version bytes.
    fn read_body(body: &[u8]) -> io::Result<Cfsa2> {
        if body.len() < 3 {
            return Err(invalid("Truncated CFSA2 header.".to_string()));
        }
        // Flags are a big-endian short (DataInputStream.readShort).
        let flag_bits = u16::from(body[0]) << 8 | u16::from(body[1]);
        if flag_bits & !flags::ALL != 0 {
            return Err(invalid(format!("Unrecognized flags: 0x{flag_bits:x}")));
        }
        let mapping_size = usize::from(body[2]);
        if body.len() < 3 + mapping_size {
            return Err(invalid("Truncated CFSA2 label mapping.".to_string()));
        }
        let mut label_mapping = [0u8; 32];
        for (i, &b) in body[3..3 + mapping_size].iter().take(32).enumerate() {
            label_mapping[i] = b;
        }
        Ok(Cfsa2 {
            arcs: body[3 + mapping_size..].to_vec(),
            label_mapping,
            flags: flag_bits,
            has_numbers: flag_bits & flags::NUMBERS != 0,
        })
    }

    #[inline]
    fn is_arc_last(&self, arc: usize) -> bool {
        self.arcs[arc] & Self::BIT_LAST_ARC != 0
    }

    #[inline]
    fn is_next_set(&self, arc: usize) -> bool {
        self.arcs[arc] & Self::BIT_TARGET_NEXT != 0
    }

    #[inline]
    fn destination_node_offset(&self, mut arc: usize) -> usize {
        if self.is_next_set(arc) {
            // Follow until the last arc of this state...
            while !self.is_arc_last(arc) {
                arc = self.skip_arc(arc);
            }
            // ...and return the byte right after it.
            self.skip_arc(arc)
        } else {
            // The v-coded address starts at the next byte (indexed label) or
            // after the next byte (explicit label).
            let explicit = self.arcs[arc] & Self::LABEL_INDEX_MASK == 0;
            self.read_vint(arc + if explicit { 2 } else { 1 })
        }
    }

    /// Skips an arc according to its layout.
    #[inline]
    fn skip_arc(&self, mut offset: usize) -> usize {
        let flag = self.arcs[offset];
        offset += 1;
        // Explicit label?
        if flag & Self::LABEL_INDEX_MASK == 0 {
            offset += 1;
        }
        // Explicit goto?
        if flag & Self::BIT_TARGET_NEXT == 0 {
            offset = self.skip_vint(offset);
        }
        offset
    }

    /// Reads a v-int: 7 bits per byte, least significant first, high bit set
    /// on every byte but the last.
    #[inline]
    fn read_vint(&self, mut offset: usize) -> usize {
        let mut b = self.arcs[offset];
        let mut value = usize::from(b & 0x7f);
        let mut shift = 7;
        while b & 0x80 != 0 {
            offset += 1;
            b = self.arcs[offset];
            value |= usize::from(b & 0x7f) << shift;
            shift += 7;
        }
        value
    }

    #[inline]
    fn skip_vint(&self, mut offset: usize) -> usize {
        while self.arcs[offset] & 0x80 != 0 {
            offset += 1;
        }
        offset + 1
    }
}

impl Automaton for Cfsa2 {
    fn root_node(&self) -> usize {
        // Skip the dummy node marking the terminating state (the epsilon node
        // is at offset 0).
        self.destination_node_offset(self.first_arc(0))
    }

    #[inline]
    fn first_arc(&self, node: usize) -> usize {
        if self.has_numbers {
            self.skip_vint(node)
        } else {
            node
        }
    }

    #[inline]
    fn next_arc(&self, arc: usize) -> usize {
        if self.is_arc_last(arc) {
            0
        } else {
            self.skip_arc(arc)
        }
    }

    #[inline]
    fn arc(&self, node: usize, label: u8) -> usize {
        let mut arc = self.first_arc(node);
        while arc != 0 {
            if self.arc_label(arc) == label {
                return arc;
            }
            arc = self.next_arc(arc);
        }
        0
    }

    #[inline]
    fn arc_label(&self, arc: usize) -> u8 {
        let index = self.arcs[arc] & Self::LABEL_INDEX_MASK;
        if index > 0 {
            self.label_mapping[usize::from(index)]
        } else {
            self.arcs[arc + 1]
        }
    }

    #[inline]
    fn is_arc_final(&self, arc: usize) -> bool {
        self.arcs[arc] & Self::BIT_FINAL_ARC != 0
    }

    #[inline]
    fn is_arc_terminal(&self, arc: usize) -> bool {
        self.destination_node_offset(arc) == 0
    }

    #[inline]
    fn end_node(&self, arc: usize) -> usize {
        self.destination_node_offset(arc)
    }

    fn flags(&self) -> u16 {
        self.flags
    }

    fn right_language_count(&self, node: usize) -> Option<usize> {
        if !self.has_numbers {
            return None;
        }
        Some(self.read_vint(node))
    }
}

// ---------------------------------------------------------------------------
// FSA (any supported version)
// ---------------------------------------------------------------------------

/// An automaton in any supported format, as returned by `FSA.read`.
pub enum Fsa {
    /// FSA version 5.
    Fsa5(Fsa5),
    /// CFSA version 2 (`0xc6`), the format of LanguageTool's dictionaries.
    Cfsa2(Cfsa2),
}

impl Fsa {
    /// `FSA.read`: reads an automaton from the contents of a `.dict` file.
    ///
    /// Supported versions are FSA5 (`0x05`) and CFSA2 (`0xc6`); the older
    /// CFSA (`0xc5`) is rejected.
    pub fn read(bytes: &[u8]) -> io::Result<Fsa> {
        if bytes.len() < 4 || bytes[..4] != FSA_MAGIC {
            return Err(invalid(
                "Invalid file header, probably not an FSA.".to_string(),
            ));
        }
        let Some(&version) = bytes.get(4) else {
            return Err(invalid("Truncated file, no version number.".to_string()));
        };
        let body = &bytes[5..];
        match version {
            Fsa5::VERSION => Ok(Fsa::Fsa5(Fsa5::read_body(body)?)),
            Cfsa2::VERSION => Ok(Fsa::Cfsa2(Cfsa2::read_body(body)?)),
            v => Err(invalid(format!("Unsupported automaton version: 0x{v:02x}"))),
        }
    }

    /// The name of the format, for diagnostics.
    pub fn format_name(&self) -> &'static str {
        match self {
            Fsa::Fsa5(_) => "FSA5",
            Fsa::Cfsa2(_) => "CFSA2",
        }
    }

    /// The size in bytes of the automaton body held in memory.
    pub fn size_in_bytes(&self) -> usize {
        match self {
            Fsa::Fsa5(a) => a.arcs.len(),
            Fsa::Cfsa2(a) => a.arcs.len(),
        }
    }

    /// `FSA.getSequences(node)`: every sequence reachable from `node`.
    pub fn sequences_from(&self, node: usize) -> SequenceIter<'_, Fsa> {
        SequenceIter::new(self, node)
    }

    /// `FSA.getSequences()`: every sequence stored in the automaton.
    pub fn sequences(&self) -> SequenceIter<'_, Fsa> {
        SequenceIter::new(self, self.root_node())
    }

    /// `FSATraversal.match` from the root node.
    pub fn match_sequence(&self, sequence: &[u8]) -> MatchResult {
        match_sequence(self, sequence, self.root_node())
    }
}

macro_rules! dispatch {
    ($self:ident, $a:ident => $e:expr) => {
        match $self {
            Fsa::Fsa5($a) => $e,
            Fsa::Cfsa2($a) => $e,
        }
    };
}

impl Automaton for Fsa {
    #[inline]
    fn root_node(&self) -> usize {
        dispatch!(self, a => a.root_node())
    }
    #[inline]
    fn first_arc(&self, node: usize) -> usize {
        dispatch!(self, a => a.first_arc(node))
    }
    #[inline]
    fn next_arc(&self, arc: usize) -> usize {
        dispatch!(self, a => a.next_arc(arc))
    }
    #[inline]
    fn arc(&self, node: usize, label: u8) -> usize {
        dispatch!(self, a => a.arc(node, label))
    }
    #[inline]
    fn arc_label(&self, arc: usize) -> u8 {
        dispatch!(self, a => a.arc_label(arc))
    }
    #[inline]
    fn is_arc_final(&self, arc: usize) -> bool {
        dispatch!(self, a => a.is_arc_final(arc))
    }
    #[inline]
    fn is_arc_terminal(&self, arc: usize) -> bool {
        dispatch!(self, a => a.is_arc_terminal(arc))
    }
    #[inline]
    fn end_node(&self, arc: usize) -> usize {
        dispatch!(self, a => a.end_node(arc))
    }
    fn flags(&self) -> u16 {
        dispatch!(self, a => a.flags())
    }
    fn right_language_count(&self, node: usize) -> Option<usize> {
        dispatch!(self, a => a.right_language_count(node))
    }
}

// ---------------------------------------------------------------------------
// Traversal
// ---------------------------------------------------------------------------

/// The kind of a [`MatchResult`] (the constants of `morfologik.fsa.MatchResult`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchKind {
    /// The automaton has exactly one match for the input sequence.
    ExactMatch,
    /// The automaton has no match for the input sequence.
    NoMatch,
    /// A prefix of the input sequence is stored in the automaton, or the
    /// sequence diverges from the automaton after its first byte.
    /// [`MatchResult::index`] is where the mismatch occurred.
    AutomatonHasPrefix,
    /// The whole sequence was consumed and it is a prefix of at least one
    /// sequence of the automaton. [`MatchResult::node`] is the node reached.
    SequenceIsAPrefix,
}

/// The result of [`match_sequence`]: port of `morfologik.fsa.MatchResult`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchResult {
    /// What was found.
    pub kind: MatchKind,
    /// Input sequence index, see [`MatchKind`].
    pub index: usize,
    /// Automaton node, see [`MatchKind`].
    pub node: usize,
}

/// `FSATraversal.match`: finds `sequence` in the automaton, starting at `node`.
#[inline]
pub fn match_sequence<A: Automaton>(fsa: &A, sequence: &[u8], mut node: usize) -> MatchResult {
    if node == 0 {
        return MatchResult {
            kind: MatchKind::NoMatch,
            index: 0,
            node,
        };
    }
    let end = sequence.len();
    for (i, &label) in sequence.iter().enumerate() {
        let arc = fsa.arc(node, label);
        if arc != 0 {
            if i + 1 == end && fsa.is_arc_final(arc) {
                // The automaton has an exact match.
                return MatchResult {
                    kind: MatchKind::ExactMatch,
                    index: i,
                    node,
                };
            }
            if fsa.is_arc_terminal(arc) {
                // The automaton contains a prefix of the input sequence.
                return MatchResult {
                    kind: MatchKind::AutomatonHasPrefix,
                    index: i + 1,
                    node,
                };
            }
            // Make a transition along the arc.
            node = fsa.end_node(arc);
        } else {
            let kind = if i > 0 {
                MatchKind::AutomatonHasPrefix
            } else {
                MatchKind::NoMatch
            };
            return MatchResult {
                kind,
                index: i,
                node,
            };
        }
    }
    // The sequence is a prefix of at least one sequence in the automaton.
    MatchResult {
        kind: MatchKind::SequenceIsAPrefix,
        index: 0,
        node,
    }
}

/// `FSATraversal.perfectHash`: the rank of `sequence` among the sequences
/// reachable from `node`, in the automaton's order.
///
/// `Ok(hash)` for a stored sequence; otherwise the [`MatchKind`] explaining
/// the failure. Requires an automaton compiled with [`flags::NUMBERS`]
/// (`NoMatch` is returned without it) and a non-empty sequence.
pub fn perfect_hash<A: Automaton>(fsa: &A, sequence: &[u8], node: usize) -> Result<usize, MatchKind> {
    if sequence.is_empty() || fsa.flags() & flags::NUMBERS == 0 {
        return Err(MatchKind::NoMatch);
    }
    let mut hash = 0usize;
    let end = sequence.len() - 1;
    let mut seq_index = 0;
    let mut label = sequence[seq_index];

    // Seek through the current node's labels, looking for 'label'.
    let mut arc = fsa.first_arc(node);
    while arc != 0 {
        if fsa.arc_label(arc) == label {
            if fsa.is_arc_final(arc) {
                if seq_index == end {
                    return Ok(hash);
                }
                hash += 1;
            }
            if fsa.is_arc_terminal(arc) {
                // The automaton contains a prefix of the input sequence.
                return Err(MatchKind::AutomatonHasPrefix);
            }
            // The sequence is a prefix of one of the stored sequences.
            if seq_index == end {
                return Err(MatchKind::SequenceIsAPrefix);
            }
            // Make a transition along the arc, go to the target's first arc.
            arc = fsa.first_arc(fsa.end_node(arc));
            seq_index += 1;
            label = sequence[seq_index];
            continue;
        }
        if fsa.is_arc_final(arc) {
            hash += 1;
        }
        if !fsa.is_arc_terminal(arc) {
            hash += fsa.right_language_count(fsa.end_node(arc)).unwrap_or(0);
        }
        arc = fsa.next_arc(arc);
    }
    if seq_index > 0 {
        Err(MatchKind::AutomatonHasPrefix)
    } else {
        // Labels of this node ended without a match on the sequence.
        Err(MatchKind::NoMatch)
    }
}

/// Port of `morfologik.fsa.ByteSequenceIterator`: enumerates, depth first and
/// in arc order, every sequence leading from a node to a final arc.
///
/// This is a lending iterator: [`SequenceIter::next`] returns a slice of an
/// internal buffer that is overwritten by the following call, so the buffers
/// are allocated once and reused ([`SequenceIter::restart_from`]).
pub struct SequenceIter<'a, A: Automaton> {
    fsa: &'a A,
    /// Labels of the path being explored.
    buffer: Vec<u8>,
    /// One pending arc per level of the path (`0`: level exhausted).
    arcs: Vec<usize>,
}

impl<'a, A: Automaton> SequenceIter<'a, A> {
    /// An iterator over the sequences reachable from `node`.
    pub fn new(fsa: &'a A, node: usize) -> Self {
        let mut it = SequenceIter {
            fsa,
            buffer: Vec::new(),
            arcs: Vec::new(),
        };
        if node != 0 && fsa.first_arc(node) != 0 {
            it.restart_from(node);
        }
        it
    }

    /// An iterator that yields nothing until [`SequenceIter::restart_from`].
    pub fn empty(fsa: &'a A) -> Self {
        SequenceIter {
            fsa,
            buffer: Vec::new(),
            arcs: Vec::new(),
        }
    }

    /// Restarts the iteration from `node`, keeping the allocated buffers.
    pub fn restart_from(&mut self, node: usize) -> &mut Self {
        self.arcs.clear();
        self.buffer.clear();
        self.arcs.push(self.fsa.first_arc(node));
        self
    }

    /// The next sequence, `None` when the iteration is over.
    #[allow(clippy::should_implement_trait)]
    #[inline]
    pub fn next(&mut self) -> Option<&[u8]> {
        while let Some(&arc) = self.arcs.last() {
            let last_index = self.arcs.len() - 1;
            if arc == 0 {
                // Remove the current node from the queue.
                self.arcs.pop();
                continue;
            }
            // Go to the next arc, but leave it on the stack so that the
            // recursion depth stays accurate.
            self.arcs[last_index] = self.fsa.next_arc(arc);

            self.buffer.truncate(last_index);
            self.buffer.push(self.fsa.arc_label(arc));

            if !self.fsa.is_arc_terminal(arc) {
                // Recursively descend into the arc's node.
                self.arcs.push(self.fsa.first_arc(self.fsa.end_node(arc)));
            }
            if self.fsa.is_arc_final(arc) {
                return Some(&self.buffer[..last_index + 1]);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-assembled FSA5 with the sequences "a" and "ab".
    ///
    /// gtl = 1: an arc is `label, flags|address<<3` (two bytes).
    fn tiny_fsa5() -> Vec<u8> {
        let mut v = b"\\fsa".to_vec();
        v.push(5); // version
        v.extend_from_slice(&[b'_', b'+', 0x01]); // filler, annotation, gtl = 1
        // offset 0: dummy terminating node (one arc, last, target 0).
        v.extend_from_slice(&[0, 0b010]);
        // offset 2: epsilon node, its arc points to the root (offset 4).
        v.extend_from_slice(&[b'^', 0b010 | (4 << 3)]);
        // offset 4: root: 'a', final + last, target = offset 6.
        v.extend_from_slice(&[b'a', 0b011 | (6 << 3)]);
        // offset 6: 'b', final + last, terminal.
        v.extend_from_slice(&[b'b', 0b011]);
        v
    }

    /// A hand-assembled CFSA2 with the sequences "a" and "ab".
    fn tiny_cfsa2() -> Vec<u8> {
        let mut v = b"\\fsa".to_vec();
        v.push(0xc6);
        v.extend_from_slice(&[0x00, 0x07]); // flags: FLEXIBLE | STOPBIT | NEXTBIT
        v.extend_from_slice(&[2, 0, b'a']); // label mapping: index 1 = 'a'
        // offset 0: epsilon node: explicit label '^', last, NEXT → root at 2.
        v.extend_from_slice(&[0x80 | 0x40, b'^']);
        // offset 2: root: indexed label 'a' (1), final, last, goto 4.
        v.extend_from_slice(&[0x40 | 0x20 | 1, 4]);
        // offset 4: explicit label 'b', final, last, goto 0 (terminal).
        v.extend_from_slice(&[0x40 | 0x20, b'b', 0]);
        v
    }

    fn all_sequences(fsa: &Fsa) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut it = fsa.sequences();
        while let Some(s) = it.next() {
            out.push(s.to_vec());
        }
        out
    }

    #[test]
    fn reads_both_formats() {
        for (bytes, name) in [(tiny_fsa5(), "FSA5"), (tiny_cfsa2(), "CFSA2")] {
            let fsa = Fsa::read(&bytes).unwrap();
            assert_eq!(fsa.format_name(), name);
            assert_eq!(all_sequences(&fsa), vec![b"a".to_vec(), b"ab".to_vec()]);
            assert_eq!(fsa.match_sequence(b"ab").kind, MatchKind::ExactMatch);
            assert_eq!(fsa.match_sequence(b"a").kind, MatchKind::ExactMatch);
            assert_eq!(fsa.match_sequence(b"b").kind, MatchKind::NoMatch);
            assert_eq!(
                fsa.match_sequence(b"abc").kind,
                MatchKind::AutomatonHasPrefix
            );
            assert_eq!(fsa.match_sequence(b"").kind, MatchKind::SequenceIsAPrefix);
            let root = fsa.root_node();
            assert_eq!(fsa.arc_count(root), 1);
            let arc = fsa.arc(root, b'a');
            assert_ne!(arc, 0);
            assert!(fsa.is_arc_final(arc) && !fsa.is_arc_terminal(arc));
            assert_eq!(fsa.arc(root, b'z'), 0);
        }
    }

    #[test]
    fn rejects_other_files() {
        assert!(Fsa::read(b"hello world").is_err());
        assert!(Fsa::read(b"\\fsa").is_err());
        assert!(Fsa::read(b"\\fsa\xc5\0\0\0").is_err());
    }
}
