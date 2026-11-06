//! Matching the static dictionary (RFC 7932 §8) while compressing.
//!
//! At quality 4 the reference looks up one word per position: the word of 8 bytes or more
//! filed under the hash of the next 4 bytes. A word matches if all of it, or all but its last 1
//! to 9 bytes, is what comes next; the copy then names the word with the transform that cuts
//! those bytes (`c/enc/hash.h`, `SearchInStaticDictionary` and `TestStaticDictionaryItem`).
//! Which word gets each hash slot is ours: the shortest, then the most common (the lowest
//! index, the dictionary's order). The reference picks with a tuned table
//! (`c/enc/dictionary_hash.c`); ours is built from the dictionary alone.
//!
//! From quality 5 the reference looks up a second word per hash too, one of 4 to 7 bytes, as
//! its table has a slot for each (`c/enc/encoder_dict.c`, `BuildDictionaryHashTable`, files
//! words that way). Ours is again the shortest, then the most common, of those. Among the
//! rules tried for it, this one is the smallest by far (see B12 in `docs/plan/README.md`).

use std::sync::OnceLock;

use crate::dictionary::{self, Kind, MAX_WORD, MIN_WORD};
use crate::dictionary_tables::{NDBITS, TRANSFORMS};
use crate::encode::command::MAX_DISTANCE;
use crate::encode::hash::{Match, match_length, score};

/// Bits of the hash of a word's first 4 bytes.
const HASH_BITS: u32 = 14;
/// The shortest word in the first slot; the second holds the shorter ones.
const MIN_LOOKUP: usize = 8;
/// How many bytes may be cut off the end of a word: transforms `OmitLast1` to `OmitLast9`.
const MAX_CUT: usize = 9;

/// The lookup table: per hash, a word of 8 bytes or more (its length, 0 for none, and index)
/// and one of 4 to 7, and per number of bytes cut, the transform that cuts them.
struct Lookup {
    slots: Box<[(u8, u16)]>,
    /// The short word per hash, for the searches of quality 5 and up.
    second: Box<[(u8, u16)]>,
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
    let slots = fill(MIN_LOOKUP..=MAX_WORD);
    let second = fill(MIN_WORD..=MIN_LOOKUP - 1);
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
    Lookup {
        slots,
        second,
        cuts,
    }
}

/// Per hash, the shortest and then most common word of the lengths `lens`.
fn fill(lens: std::ops::RangeInclusive<usize>) -> Box<[(u8, u16)]> {
    let mut slots = vec![(0u8, 0u16); 1 << HASH_BITS].into_boxed_slice();
    // Longest first and highest index first, so the shortest and most common word of each
    // hash is written last and stays.
    for len in lens.rev() {
        for index in (0..dictionary::word_count(len)).rev() {
            slots[hash14(dictionary::word(len, index))] = (len as u8, index as u16);
        }
    }
    slots
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
        let slot = self.table.slots[hash14(data)];
        self.try_word(slot, data, max_length, max_backward, best);
    }

    /// [`search`](Self::search) with two words per hash, as from quality 5 (the reference's
    /// `SearchInStaticDictionary` with `shallow` false): the long word, then the short one,
    /// each a lookup of its own. The short one wins a tie.
    pub(crate) fn search_deep(
        &mut self,
        data: &[u8],
        max_length: usize,
        max_backward: usize,
        best: &mut Match,
    ) {
        if self.matches < self.lookups >> 7 {
            return;
        }
        let key = hash14(data);
        let (first, second) = (self.table.slots[key], self.table.second[key]);
        self.try_word(first, data, max_length, max_backward, best);
        self.try_word(second, data, max_length, max_backward, best);
    }

    /// One lookup: the word in `slot` (length 0 for none), whole or cut, into `best` if it
    /// scores at least as well.
    #[inline(always)]
    fn try_word(
        &mut self,
        (len, index): (u8, u16),
        data: &[u8],
        max_length: usize,
        max_backward: usize,
        best: &mut Match,
    ) {
        self.lookups += 1;
        let (len, index) = (len as usize, index as usize);
        if len == 0 || len > max_length {
            return;
        }
        let matched = match_length(data, dictionary::word(len, index), len);
        if matched == 0 || matched + MAX_CUT < len {
            return;
        }
        let transform = usize::from(self.table.cuts[len - matched]);
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
        for (slots, lens) in [
            (&table.slots, MIN_LOOKUP..=dictionary::MAX_WORD),
            (&table.second, dictionary::MIN_WORD..=MIN_LOOKUP - 1),
        ] {
            for len in lens.clone() {
                for index in 0..dictionary::word_count(len) {
                    let (l, i) = slots[hash14(dictionary::word(len, index))];
                    assert!(
                        lens.contains(&(l as usize)) && (l as usize, i as usize) <= (len, index),
                        "length {len} word {index} lost to length {l} word {i}"
                    );
                }
            }
        }
        // 7,616 words of 8 bytes or more, with 2,376 different first 4 bytes.
        let used = table.slots.iter().filter(|s| s.0 != 0).count();
        assert!(used > 2000, "{used} slots used");
        let used = table.second.iter().filter(|s| s.0 != 0).count();
        assert!(used > 2000, "{used} short slots used");
    }

    #[test]
    fn the_deep_search_finds_short_words_too() {
        let table = build();
        // A short word whose hash has no long word, then a long word's hash with a short one.
        let only_short = (0..table.second.len())
            .find(|&k| table.second[k].0 != 0 && table.slots[k].0 == 0)
            .unwrap();
        let (len, index) = table.second[only_short];
        let mut data = dictionary::word(usize::from(len), usize::from(index)).to_vec();
        data.extend_from_slice(b"##########");
        let mut best = fresh();
        let mut search = Search::default();
        search.search(&data, data.len(), 1000, &mut best);
        assert_eq!(best, fresh(), "quality 4 doesn't look");
        assert_eq!(search.lookups, 1);
        search.search_deep(&data, data.len(), 1000, &mut best);
        assert_eq!(best.len, usize::from(len));
        assert_eq!(best.distance, 1000 + 1 + usize::from(index));
        assert_eq!(search.lookups, 3, "a lookup per word");
        // Where both match, the longer copy wins.
        let both = (0..table.second.len())
            .find(|&k| {
                let (l, i) = table.slots[k];
                let (sl, si) = table.second[k];
                l != 0
                    && sl != 0
                    && dictionary::word(usize::from(l), usize::from(i))
                        .starts_with(dictionary::word(usize::from(sl), usize::from(si)))
            })
            .unwrap();
        let (len, index) = table.slots[both];
        let mut data = dictionary::word(usize::from(len), usize::from(index)).to_vec();
        data.extend_from_slice(b"##########");
        let mut best = fresh();
        Search::default().search_deep(&data, data.len(), 1000, &mut best);
        assert_eq!(best.len, usize::from(len));
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
