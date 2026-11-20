//! An automaton built in memory from a word list.
//!
//! Stands for what `MorfologikMultiSpeller.getDictionary` does with
//! `morfologik.fsa.builders.FSABuilder` and `CFSA2Serializer`: the lines of
//! the plain-text spelling files are sorted (unsigned byte order) and turned
//! into a minimal deterministic automaton, which is then read back as a
//! Morfologik dictionary with the metadata of the binary dictionary.
//!
//! The result here is serialized in the FSA5 layout rather than CFSA2 (both
//! are read by `crate::morfologik`). What the speller observes is the same:
//! the set of accepted byte sequences, and the order of the arcs leaving each
//! state, which is the order of their labels in both cases (`FSABuilder` adds
//! arcs in input order and the serializers keep it), so traversals meet the
//! words in the same order.

use std::collections::HashMap;
use std::io;

use crate::morfologik::Fsa;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Arc {
    label: u8,
    is_final: bool,
    /// Index of the target state plus one; `0` when the arc has no target.
    target: u32,
}

/// `FSABuilder.build` on sorted input, then serialization: the bytes of an
/// FSA5 automaton accepting exactly `words`.
///
/// `words` must be sorted in unsigned byte order; duplicates and empty
/// sequences are skipped.
pub fn build_fsa5(words: &[&[u8]]) -> Vec<u8> {
    // Daciuk's incremental construction for sorted input: the states on the
    // path of the last word stay open; a state is frozen (and merged with an
    // equal one) as soon as the next word leaves its path.
    let mut states: Vec<Box<[Arc]>> = Vec::new();
    let mut registry: HashMap<Box<[Arc]>, u32> = HashMap::new();
    let mut path: Vec<Vec<Arc>> = vec![Vec::new()];
    let mut previous: &[u8] = &[];

    fn freeze(arcs: Vec<Arc>, states: &mut Vec<Box<[Arc]>>, registry: &mut HashMap<Box<[Arc]>, u32>) -> u32 {
        if arcs.is_empty() {
            return 0;
        }
        let key: Box<[Arc]> = arcs.into_boxed_slice();
        if let Some(&id) = registry.get(&key) {
            return id;
        }
        states.push(key.clone());
        let id = states.len() as u32;
        registry.insert(key, id);
        id
    }

    for &word in words {
        if word.is_empty() || word == previous {
            continue;
        }
        debug_assert!(previous < word, "input must be sorted");
        let common = previous.iter().zip(word).take_while(|(a, b)| a == b).count();
        // Freeze the states below the common prefix, deepest first.
        while path.len() > common + 1 {
            let arcs = path.pop().unwrap_or_default();
            let id = freeze(arcs, &mut states, &mut registry);
            if let Some(last) = path.last_mut().and_then(|s| s.last_mut()) {
                last.target = id;
            }
        }
        for (i, &label) in word.iter().enumerate().skip(common) {
            path[i].push(Arc { label, is_final: i == word.len() - 1, target: 0 });
            path.push(Vec::new());
        }
        previous = word;
    }
    while path.len() > 1 {
        let arcs = path.pop().unwrap_or_default();
        let id = freeze(arcs, &mut states, &mut registry);
        if let Some(last) = path.last_mut().and_then(|s| s.last_mut()) {
            last.target = id;
        }
    }
    let root = freeze(path.pop().unwrap_or_default(), &mut states, &mut registry);
    drop(registry);

    // FSA5 layout: every arc is a label followed by `gtl` bytes holding the
    // flags (low three bits) and the offset of the target state. The body
    // starts with a dummy terminal state and the epsilon state whose only arc
    // leads to the root.
    const BIT_FINAL: u64 = 1;
    const BIT_LAST: u64 = 2;
    let total_arcs: usize = states.iter().map(|s| s.len()).sum::<usize>() + 2;
    let mut gtl = 1usize;
    while (total_arcs * (1 + gtl)) as u64 >= 1u64 << (8 * gtl - 3) {
        gtl += 1;
    }
    let arc_size = 1 + gtl;
    let mut offsets: Vec<usize> = Vec::with_capacity(states.len());
    let mut offset = 2 * arc_size;
    for s in &states {
        offsets.push(offset);
        offset += s.len() * arc_size;
    }
    let address = |target: u32| -> u64 {
        if target == 0 {
            0
        } else {
            offsets[target as usize - 1] as u64
        }
    };
    let mut out: Vec<u8> = Vec::with_capacity(8 + offset);
    out.extend_from_slice(b"\\fsa");
    out.push(5); // version
    out.push(b'_'); // filler
    out.push(b'+'); // annotation
    out.push(gtl as u8); // no node data, goto length
    let mut emit = |label: u8, flags: u64, target: u64| {
        out.push(label);
        let combined = target << 3 | flags;
        for i in 0..gtl {
            out.push((combined >> (8 * i)) as u8);
        }
    };
    emit(0, BIT_LAST, 0);
    emit(b'^', BIT_LAST, address(root));
    for s in &states {
        for (i, arc) in s.iter().enumerate() {
            let mut flags = 0;
            if arc.is_final {
                flags |= BIT_FINAL;
            }
            if i == s.len() - 1 {
                flags |= BIT_LAST;
            }
            emit(arc.label, flags, address(arc.target));
        }
    }
    out
}

/// The automaton of a word list (any order, duplicates allowed), as
/// `MorfologikMultiSpeller.getDictionary` builds it.
pub fn fsa_from_lines(lines: &[String]) -> io::Result<Fsa> {
    let mut sorted: Vec<&[u8]> = lines.iter().map(|l| l.as_bytes()).collect();
    sorted.sort_unstable();
    sorted.dedup();
    Fsa::read(&build_fsa5(&sorted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::morfologik::MatchKind;

    fn words_of(fsa: &Fsa) -> Vec<String> {
        let mut out = Vec::new();
        let mut it = fsa.sequences();
        while let Some(seq) = it.next() {
            out.push(String::from_utf8(seq.to_vec()).unwrap());
        }
        out
    }

    #[test]
    fn accepts_exactly_the_words_in_order() {
        let lines: Vec<String> =
            ["chat", "chats", "chien", "a", "ab", "Joe Biden", "école", "chat", "zèbre", "abc", "b"].iter().map(|s| s.to_string()).collect();
        let fsa = fsa_from_lines(&lines).unwrap();
        let mut expected: Vec<&[u8]> = lines.iter().map(|l| l.as_bytes()).collect();
        expected.sort_unstable();
        expected.dedup();
        let expected: Vec<String> = expected.into_iter().map(|b| String::from_utf8(b.to_vec()).unwrap()).collect();
        assert_eq!(words_of(&fsa), expected);
        assert_eq!(fsa.match_sequence(b"chat").kind, MatchKind::ExactMatch);
        assert_eq!(fsa.match_sequence("école".as_bytes()).kind, MatchKind::ExactMatch);
        assert_eq!(fsa.match_sequence(b"cha").kind, MatchKind::SequenceIsAPrefix);
        assert_ne!(fsa.match_sequence(b"chatte").kind, MatchKind::ExactMatch);
        assert_ne!(fsa.match_sequence(b"x").kind, MatchKind::ExactMatch);
    }

    #[test]
    fn shares_suffixes_and_handles_many_words() {
        // 3 000 words with shared endings: more than one byte of address.
        let mut lines: Vec<String> = Vec::new();
        for a in 0..30 {
            for b in 0..100 {
                lines.push(format!("mot{a}x{b}ement"));
            }
        }
        let fsa = fsa_from_lines(&lines).unwrap();
        assert_eq!(words_of(&fsa).len(), 3000);
        assert_eq!(fsa.match_sequence(b"mot29x99ement").kind, MatchKind::ExactMatch);
        assert_ne!(fsa.match_sequence(b"mot30x0ement").kind, MatchKind::ExactMatch);
        // Minimal: far fewer bytes than a trie of 3 000 * 13 arcs.
        assert!(fsa.size_in_bytes() < 3000 * 13);
    }

    #[test]
    fn empty_list() {
        let fsa = fsa_from_lines(&[]).unwrap();
        assert!(words_of(&fsa).is_empty());
        assert_ne!(fsa.match_sequence(b"a").kind, MatchKind::ExactMatch);
    }
}
