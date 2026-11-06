//! The static dictionary and its word transforms (RFC 7932 §8, Appendices A and B).
//!
//! `dictionary.bin` is Appendix A's DICT array, extracted by `scripts/extract-rfc.py`: 122,784
//! bytes with the CRC-32 the RFC lists (0x5136cb04) and SHA-256
//! `20e42eb1b511c21806d4d227d07e5dd06877d8ce7b3a817f378f313653f35c70`. It is identical to
//! google/brotli's `c/common/dictionary.bin` (compared 2026-11-05).

use crate::Error;
use crate::dictionary_tables::{NDBITS, TRANSFORMS};

/// The dictionary words, all lengths one after another (Appendix A).
static DICTIONARY: &[u8; 122_784] = include_bytes!("dictionary.bin");

/// The elementary transforms of §8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Identity,
    /// Uppercases the first character, as §8's `FermentFirst`.
    FermentFirst,
    /// Uppercases every character, as §8's `FermentAll`.
    FermentAll,
    /// Drops the first `k` bytes.
    OmitFirst(u8),
    /// Drops the last `k` bytes.
    OmitLast(u8),
}

/// Where the words of each length start in the dictionary (§8's DOFFSET).
const OFFSETS: [usize; 25] = {
    let mut offsets = [0; 25];
    let mut length = 0;
    while length < 24 {
        let words = if length < 4 { 0 } else { 1 << NDBITS[length] };
        offsets[length + 1] = offsets[length] + length * words;
        length += 1;
    }
    offsets
};

/// The shortest and longest dictionary words.
pub(crate) const MIN_WORD: usize = 4;
pub(crate) const MAX_WORD: usize = 24;

/// The number of dictionary words of length `len` (4..=24).
pub(crate) fn word_count(len: usize) -> usize {
    1 << NDBITS[len]
}

/// Dictionary word `index` of length `len` (4..=24), untransformed.
pub(crate) fn word(len: usize, index: usize) -> &'static [u8] {
    let start = OFFSETS[len] + index * len;
    &DICTIONARY[start..start + len]
}

/// Appends dictionary word `word_id` of base length `len`, transformed (§8), if it fits in
/// `room` bytes.
///
/// # Errors
///
/// [`Error::Corrupt`] if `len` is outside 4..=24, the transform doesn't exist, or the word is
/// longer than `room`; nothing is appended then.
pub(crate) fn append_word(
    out: &mut Vec<u8>,
    len: usize,
    word_id: usize,
    room: usize,
) -> Result<(), Error> {
    if !(MIN_WORD..=MAX_WORD).contains(&len) {
        return Err(Error::Corrupt(
            "a dictionary reference with a length outside 4..=24",
        ));
    }
    let bits = NDBITS[len];
    let index = word_id & ((1 << bits) - 1);
    let transform = word_id >> bits;
    let Some(&(prefix, kind, suffix)) = TRANSFORMS.get(transform) else {
        return Err(Error::Corrupt(
            "a dictionary reference with a transform past 120",
        ));
    };
    let body = match kind {
        Kind::OmitFirst(k) | Kind::OmitLast(k) => len.saturating_sub(k as usize),
        _ => len,
    };
    if prefix.len() + body + suffix.len() > room {
        return Err(Error::Corrupt(
            "a dictionary word runs past the end of the meta-block",
        ));
    }
    let word = word(len, index);
    out.extend_from_slice(prefix);
    let base = out.len();
    match kind {
        Kind::Identity => out.extend_from_slice(word),
        Kind::OmitFirst(k) => out.extend_from_slice(&word[(k as usize).min(len)..]),
        Kind::OmitLast(k) => out.extend_from_slice(&word[..len.saturating_sub(k as usize)]),
        Kind::FermentFirst => {
            out.extend_from_slice(word);
            ferment(&mut out[base..], 0);
        }
        Kind::FermentAll => {
            out.extend_from_slice(word);
            ferment_all(&mut out[base..]);
        }
    }
    out.extend_from_slice(suffix);
    Ok(())
}

/// §8's `FermentAll`: [`ferment`] at each character, stepping by the length it returns.
fn ferment_all(word: &mut [u8]) {
    let mut i = 0;
    while i < word.len() {
        i += ferment(word, i);
    }
}

/// §8's `Ferment`: uppercases the character at `pos` of `word` and returns its length (an
/// ASCII letter is flipped to uppercase; for a 2- or 3-byte UTF-8 sequence, a bit of its
/// second or third byte is flipped, as the RFC defines it).
fn ferment(word: &mut [u8], pos: usize) -> usize {
    let len = word.len();
    let c = word[pos];
    if c < 192 {
        if c.is_ascii_lowercase() {
            word[pos] ^= 32;
        }
        1
    } else if c < 224 {
        if pos + 1 < len {
            word[pos + 1] ^= 32;
        }
        2
    } else {
        if pos + 2 < len {
            word[pos + 2] ^= 5;
        }
        3
    }
}

#[cfg(test)]
mod tests {
    use super::{DICTIONARY, Kind, MAX_WORD, OFFSETS, append_word};
    use crate::Error;
    use crate::dictionary_tables::{NDBITS, TRANSFORMS};

    fn word(len: usize, word_id: usize) -> Result<Vec<u8>, Error> {
        let mut out = b"..".to_vec();
        append_word(&mut out, len, word_id, usize::MAX)?;
        Ok(out.split_off(2))
    }

    #[test]
    fn a_word_longer_than_the_room_appends_nothing() {
        // Transform 73: " the " + "time" + " of the " is 17 bytes.
        let id = 73 << NDBITS[4];
        let mut out = Vec::new();
        assert_eq!(
            append_word(&mut out, 4, id, 16),
            Err(Error::Corrupt(
                "a dictionary word runs past the end of the meta-block"
            ))
        );
        assert_eq!(out, b"");
        append_word(&mut out, 4, id, 17).unwrap();
        assert_eq!(out, b" the time of the ");
        // OmitFirst9 and OmitLast9 of a 4-byte word: nothing, which always fits.
        for transform in [54, 64] {
            let mut out = Vec::new();
            append_word(&mut out, 4, transform << NDBITS[4], 0).unwrap();
            assert_eq!(out, b"", "transform {transform}");
        }
    }

    /// The id of `transform` applied to word `index` of length `len`.
    fn id(len: usize, index: usize, transform: usize) -> usize {
        (transform << NDBITS[len]) | index
    }

    #[test]
    fn the_word_offsets_cover_the_dictionary() {
        let end = OFFSETS[MAX_WORD] + MAX_WORD * (1 << NDBITS[MAX_WORD]);
        assert_eq!(end, DICTIONARY.len());
        assert_eq!(OFFSETS[4], 0, "no words shorter than 4");
    }

    #[test]
    fn the_first_words() {
        // Appendix A starts "timedownlifeleftbackcodedatashowonlysitecity...": the first
        // four-letter words, then the first five-letter word after the 1024 four-letter ones.
        assert_eq!(word(4, 0).unwrap(), b"time");
        assert_eq!(word(4, 1).unwrap(), b"down");
        assert_eq!(word(4, 2).unwrap(), b"life");
        assert_eq!(
            &DICTIONARY[OFFSETS[5]..OFFSETS[5] + 5],
            &word(5, 0).unwrap()[..]
        );
    }

    #[test]
    fn each_kind_of_transform() {
        // (transform ID, its expected output on "time")
        let cases: &[(usize, &[u8])] = &[
            (0, b"time"),               // identity
            (1, b"time "),              // suffix
            (2, b" time "),             // prefix and suffix
            (3, b"ime"),                // OmitFirst1
            (9, b"Time"),               // FermentFirst
            (12, b"tim"),               // OmitLast1
            (44, b"TIME"),              // FermentAll
            (49, b"timing "),           // OmitLast1 + "ing "
            (64, b""),                  // OmitLast9 on 4 bytes: empty
            (54, b""),                  // OmitFirst9
            (73, b" the time of the "), // prefix and suffix
            (102, b"\xc2\xa0time"),     // a two-byte prefix
        ];
        for &(transform, expected) in cases {
            assert_eq!(
                word(4, id(4, 0, transform)).unwrap(),
                expected,
                "transform {transform}: {:?}",
                TRANSFORMS[transform]
            );
        }
    }

    #[test]
    fn ferment_follows_the_rfc_rule_for_utf8() {
        let mut two = *b"\xc3\xa9a"; // "éa": the second byte's 0x20 bit flips
        super::ferment(&mut two, 0);
        assert_eq!(&two, b"\xc3\x89a");
        let mut three = *b"\xe3\x81\x82"; // a 3-byte character: the third byte's 0x05 bits flip
        super::ferment(&mut three, 0);
        assert_eq!(&three, b"\xe3\x81\x87");
        let mut cut = *b"\xe3\x81"; // too short: nothing to flip, still counts 3
        assert_eq!(super::ferment(&mut cut, 0), 3);
        assert_eq!(&cut, b"\xe3\x81");
        let mut not_letter = *b"1a";
        super::ferment(&mut not_letter, 0);
        assert_eq!(&not_letter, b"1a");
    }

    #[test]
    fn ferment_all_steps_over_whole_characters() {
        // A 3-byte lead covers the next two bytes even when they are ASCII: only the third
        // byte changes (^ 5), and the two letters are not uppercased.
        let mut word = *b"\xe3ab";
        super::ferment_all(&mut word);
        assert_eq!(&word, b"\xe3ag");
        let mut word = *b"ab\xc3\xa9cd";
        super::ferment_all(&mut word);
        assert_eq!(&word, b"AB\xc3\x89CD");
    }

    #[test]
    fn transform_ids_past_120_are_corrupt() {
        assert!(word(4, id(4, 1023, 120)).is_ok());
        assert_eq!(
            word(4, id(4, 0, 121)),
            Err(Error::Corrupt(
                "a dictionary reference with a transform past 120"
            ))
        );
        assert_eq!(
            word(24, usize::MAX >> 8),
            Err(Error::Corrupt(
                "a dictionary reference with a transform past 120"
            ))
        );
    }

    #[test]
    fn lengths_outside_4_to_24_are_corrupt() {
        for len in [0, 2, 3, 25, 100] {
            assert_eq!(
                word(len, 0),
                Err(Error::Corrupt(
                    "a dictionary reference with a length outside 4..=24"
                )),
                "length {len}"
            );
        }
        for len in 4..=24 {
            assert_eq!(word(len, 0).unwrap().len(), len, "length {len}");
        }
    }

    #[test]
    fn every_transform_kind_appears_and_omit_first_8_does_not() {
        // §8: OmitFirst8 is the one elementary transform the list doesn't use.
        let kinds: Vec<Kind> = TRANSFORMS.iter().map(|t| t.1).collect();
        assert!(!kinds.contains(&Kind::OmitFirst(8)));
        for k in [Kind::Identity, Kind::FermentFirst, Kind::FermentAll] {
            assert!(kinds.contains(&k), "{k:?}");
        }
        for n in (1..=9).filter(|&n| n != 8) {
            assert!(kinds.contains(&Kind::OmitFirst(n)), "OmitFirst{n}");
        }
        for n in 1..=9 {
            assert!(kinds.contains(&Kind::OmitLast(n)), "OmitLast{n}");
        }
    }
}
