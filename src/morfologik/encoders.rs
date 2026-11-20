//! Stem decoding: how the lemma of an entry is rebuilt from its word form.
//!
//! Port of the `decode` side of `morfologik.stemming.TrimSuffixEncoder`,
//! `TrimPrefixAndSuffixEncoder`, `TrimInfixAndSuffixEncoder` and `NoEncoder`
//! (Morfologik 2.1.9, BSD-3-Clause). The `encode` side is only needed to
//! build dictionaries and is not ported.
//!
//! A dictionary stores `word SEP encoded-stem SEP tag`. The encoded stem
//! starts with one to three control bytes (`'A' + n`) telling how many bytes
//! of the word to drop, followed by the bytes to append.

use super::metadata::EncoderType;

/// The control value meaning "drop the whole word".
const REMOVE_EVERYTHING: usize = 255;

/// `ISequenceEncoder.prefixBytes`: the number of control bytes that start an
/// encoded stem (a separator byte among them is not a separator).
#[inline]
pub fn prefix_bytes(encoder: EncoderType) -> usize {
    match encoder {
        EncoderType::Suffix => 1,
        EncoderType::Prefix => 2,
        EncoderType::Infix => 3,
        EncoderType::None => 0,
    }
}

#[inline]
fn control(b: u8) -> usize {
    usize::from(b.wrapping_sub(b'A'))
}

/// `ISequenceEncoder.decode`: appends to `out` the stem obtained from the
/// word `source` and the `encoded` stem. `out` is cleared first.
///
/// A malformed entry (control bytes missing or larger than the word) yields a
/// truncated stem instead of the exception Java would throw.
#[inline]
pub fn decode(encoder: EncoderType, source: &[u8], encoded: &[u8], out: &mut Vec<u8>) {
    out.clear();
    match encoder {
        EncoderType::Suffix => {
            let Some(&code) = encoded.first() else { return };
            let mut truncate = control(code);
            if truncate == REMOVE_EVERYTHING {
                truncate = source.len();
            }
            let len1 = source.len().saturating_sub(truncate);
            out.extend_from_slice(&source[..len1]);
            out.extend_from_slice(&encoded[1..]);
        }
        EncoderType::Prefix => {
            if encoded.len() < 2 {
                return;
            }
            let mut truncate_prefix = control(encoded[0]);
            let mut truncate_suffix = control(encoded[1]);
            if truncate_prefix == REMOVE_EVERYTHING || truncate_suffix == REMOVE_EVERYTHING {
                truncate_prefix = source.len();
                truncate_suffix = 0;
            }
            let start = truncate_prefix.min(source.len());
            let end = source.len().saturating_sub(truncate_suffix).max(start);
            out.extend_from_slice(&source[start..end]);
            out.extend_from_slice(&encoded[2..]);
        }
        EncoderType::Infix => {
            if encoded.len() < 3 {
                return;
            }
            let mut infix_index = control(encoded[0]);
            let mut infix_length = control(encoded[1]);
            let mut truncate_suffix = control(encoded[2]);
            if infix_length == REMOVE_EVERYTHING || truncate_suffix == REMOVE_EVERYTHING {
                infix_index = 0;
                infix_length = source.len();
                truncate_suffix = 0;
            }
            let head = infix_index.min(source.len());
            let tail_start = (infix_index + infix_length).min(source.len());
            let tail_end = source.len().saturating_sub(truncate_suffix).max(tail_start);
            out.extend_from_slice(&source[..head]);
            out.extend_from_slice(&source[tail_start..tail_end]);
            out.extend_from_slice(&encoded[3..]);
        }
        EncoderType::None => out.extend_from_slice(encoded),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(encoder: EncoderType, source: &str, encoded: &[u8]) -> String {
        let mut out = Vec::new();
        decode(encoder, source.as_bytes(), encoded, &mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn suffix() {
        // "chevaux" → drop 2 bytes, append "l".
        assert_eq!(dec(EncoderType::Suffix, "chevaux", b"Cl"), "cheval");
        assert_eq!(dec(EncoderType::Suffix, "maison", b"A"), "maison");
        assert_eq!(dec(EncoderType::Suffix, "suis", b"E\xc3\xaatre"), "être");
        // 'A' + 255 wraps to '@': remove everything.
        assert_eq!(dec(EncoderType::Suffix, "suis", b"@aller"), "aller");
    }

    #[test]
    fn prefix_and_infix() {
        // Drop 2 bytes in front ("ge") and 1 at the end ("t"), append "en".
        assert_eq!(dec(EncoderType::Prefix, "gemacht", b"CBen"), "machen");
        assert_eq!(dec(EncoderType::Prefix, "abc", b"@Axyz"), "xyz");
        // Remove the infix of length 2 at index 2, cut 1 byte, append "en".
        assert_eq!(dec(EncoderType::Infix, "abgemacht", b"CCBen"), "abmachen");
        assert_eq!(dec(EncoderType::Infix, "abc", b"A@Axyz"), "xyz");
        assert_eq!(dec(EncoderType::None, "abc", b"xyz"), "xyz");
    }
}
