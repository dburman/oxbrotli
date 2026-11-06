//! Matching the static dictionary (RFC 7932 §8) while compressing.
//!
//! At quality 4 the reference looks up one word per position: the word of 8 bytes or more
//! filed under the hash of the next 4 bytes. A word matches if all of it, or all but its last 1
//! to 9 bytes, is what comes next; the copy then names the word with the transform that cuts
//! those bytes (`c/enc/hash.h`, `SearchInStaticDictionary` and `TestStaticDictionaryItem`).
//! Which word gets each hash slot is ours: the shortest, then the most common (the lowest
//! index, the dictionary's order). The reference picks with a tuned table
//! (`c/enc/dictionary_hash.c`); ours is built from the dictionary alone.

use std::sync::OnceLock;

use crate::dictionary::{self, Kind, MAX_WORD};
use crate::dictionary_tables::{NDBITS, TRANSFORMS};
use crate::encode::command::MAX_DISTANCE;
use crate::encode::hash::{Match, match_length, score};

/// Bits of the hash of a word's first 4 bytes.
const HASH_BITS: u32 = 14;
/// The shortest word looked up.
const MIN_LOOKUP: usize = 8;
/// How many bytes may be cut off the end of a word: transforms `OmitLast1` to `OmitLast9`.
const MAX_CUT: usize = 9;

/// The lookup table: per hash, a word (its length, 0 for none, and index), and per number of
/// bytes cut, the transform that cuts them.
struct Lookup {
    slots: Box<[(u8, u16)]>,
    cuts: [u16; MAX_CUT + 1],
}

/// The hash of 4 bytes, as the reference's `Hash14` (`c/enc/hash_base.h`).
fn hash14(bytes: &[u8]) -> usize {
    let v = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    (v.wrapping_mul(0x1e35_a7bd) >> (32 - HASH_BITS)) as usize
}

fn lookup() -> &'static Lookup {
    static LOOKUP: OnceLock<Lookup> = OnceLock::new();
    LOOKUP.get_or_init(build)
}

fn build() -> Lookup {
    let mut slots = vec![(0u8, 0u16); 1 << HASH_BITS].into_boxed_slice();
    // Longest first and highest index first, so the shortest and most common word of each
    // hash is written last and stays.
    for len in (MIN_LOOKUP..=MAX_WORD).rev() {
        for index in (0..dictionary::word_count(len)).rev() {
            slots[hash14(dictionary::word(len, index))] = (len as u8, index as u16);
        }
    }
    let mut cuts = [0u16; MAX_CUT + 1];
    for (cut, transform) in cuts.iter_mut().enumerate() {
        let kind = if cut == 0 {
            Kind::Identity
        } else {
            Kind::OmitLast(cut as u8)
        };
        // Every cut has a transform with nothing before or after the word (Appendix B).
        *transform = TRANSFORMS
            .iter()
            .position(|t| t.0.is_empty() && t.1 == kind && t.2.is_empty())
            .unwrap_or(0) as u16;
    }
    Lookup { slots, cuts }
}

/// Builds the lookup table now rather than at the first dictionary search.
#[cfg(test)]
pub(crate) fn prepare() {
    lookup();
}

/// Dictionary searches for one stream: they stop being made once fewer than one in 128 finds
/// a word, as the reference's do.
pub(crate) struct Search {
    lookups: usize,
    matches: usize,
    /// Fetched once for the stream: a search then calls nothing and needs no stack frame. Had
    /// each search fetched it, the call that may build it cost 2% on a 200 KiB script.
    table: &'static Lookup,
}

impl Default for Search {
    fn default() -> Self {
        Self {
            lookups: 0,
            matches: 0,
            table: lookup(),
        }
    }
}

impl Search {
    /// Looks for a dictionary word at the start of `data` (at least `max_length` bytes) and
    /// puts it in `best` if it scores at least as well. Dictionary distances start past
    /// `max_backward`, the farthest a copy could reach here.
    pub(crate) fn search(
        &mut self,
        data: &[u8],
        max_length: usize,
        max_backward: usize,
        best: &mut Match,
    ) {
        if self.matches < self.lookups >> 7 {
            return;
        }
        self.lookups += 1;
        let table = self.table;
        let (len, index) = table.slots[hash14(data)];
        let (len, index) = (len as usize, index as usize);
        if len == 0 || len > max_length {
            return;
        }
        let matched = match_length(data, dictionary::word(len, index), len);
        if matched == 0 || matched + MAX_CUT < len {
            return;
        }
        let transform = usize::from(table.cuts[len - matched]);
        let distance = max_backward + 1 + index + (transform << NDBITS[len]);
        if distance > MAX_DISTANCE as usize {
            return;
        }
        let s = score(matched, distance);
        if s < best.score {
            return;
        }
        *best = Match {
            len: matched,
            len_code: len,
            distance,
            score: s,
        };
        self.matches += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::{MIN_LOOKUP, Search, build, hash14};
    use crate::dictionary;
    use crate::encode::hash::{MIN_SCORE, Match};

    /// Not a check: how long building the table takes (run with --nocapture, in release).
    #[test]
    #[ignore = "a measurement"]
    fn build_time() {
        let start = std::time::Instant::now();
        let runs = 100;
        for _ in 0..runs {
            std::hint::black_box(build());
        }
        println!(
            "building the dictionary lookup: {:?}",
            start.elapsed() / runs
        );
        super::prepare();
    }

    #[test]
    fn the_cut_transforms_are_the_rfcs() {
        // Appendix B: identity, then OmitLast1 to OmitLast9 with no prefix or suffix.
        assert_eq!(build().cuts, [0, 12, 27, 23, 42, 63, 56, 48, 59, 64]);
    }

    #[test]
    fn each_slot_holds_its_shortest_most_common_word() {
        let table = build();
        for len in MIN_LOOKUP..=dictionary::MAX_WORD {
            for index in 0..dictionary::word_count(len) {
                let (l, i) = table.slots[hash14(dictionary::word(len, index))];
                assert!(
                    (l as usize, i as usize) <= (len, index),
                    "length {len} word {index} lost to length {l} word {i}"
                );
            }
        }
        // 7,616 words of 8 bytes or more, with 2,376 different first 4 bytes.
        let used = table.slots.iter().filter(|s| s.0 != 0).count();
        assert!(used > 2000, "{used} slots used");
    }

    fn fresh() -> Match {
        Match {
            len: 0,
            len_code: 0,
            distance: 0,
            score: MIN_SCORE,
        }
    }

    #[test]
    fn a_whole_word_and_a_cut_word_are_found() {
        let table = build();
        // Some word filed in its slot, then the same with its last 3 bytes changed.
        let (len, index) = table
            .slots
            .iter()
            .copied()
            .find(|&(l, _)| l == 10)
            .map(|(l, i)| (l as usize, i as usize))
            .unwrap();
        let word = dictionary::word(len, index);
        let mut data = word.to_vec();
        data.extend_from_slice(b"..........");
        let mut best = fresh();
        Search::default().search(&data, data.len(), 1000, &mut best);
        assert_eq!((best.len, best.len_code), (len, len));
        assert_eq!(best.distance, 1000 + 1 + index);
        for b in &mut data[len - 3..len] {
            *b = b'#';
        }
        let mut best = fresh();
        Search::default().search(&data, data.len(), 1000, &mut best);
        assert_eq!((best.len, best.len_code), (len - 3, len));
        assert_eq!(
            best.distance,
            1000 + 1 + index + (23 << crate::dictionary_tables::NDBITS[len])
        );
        // Not when the word runs past the data that is left.
        let mut best = fresh();
        Search::default().search(&data, len - 1, 1000, &mut best);
        assert_eq!(best, fresh());
    }

    #[test]
    fn searches_stop_when_they_rarely_find_anything() {
        let mut search = Search::default();
        let data = [0xffu8; 32];
        for _ in 0..1000 {
            search.search(&data, 32, 1000, &mut fresh());
        }
        assert!(search.lookups < 200, "{} lookups", search.lookups);
    }
}
