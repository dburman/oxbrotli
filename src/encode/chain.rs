//! The match finder of qualities 5 and 6 for windows over 16: the reference's H5 and H6
//! (`c/enc/hash_longest_match_inc.h`, `c/enc/hash_longest_match64_inc.h`).
//!
//! Each hash has a bucket of the last `2^block_bits` positions filed under it, newest last,
//! in a ring: `num` counts the positions ever filed, and the newest is at
//! `(num - 1) & block_mask`. A lookup tries the four last distances first, then the bucket from
//! newest to oldest until a position is out of reach, keeping the best-scoring match.

use std::ops::Range;

use crate::encode::command::Ring;
use crate::encode::hash::{Finder, HASH_MUL, Match, last_distance_score, match_length, score};
use crate::encode::static_dict;

/// The fewest buckets a hinted table has, in bits.
const MIN_BUCKET_BITS: u32 = 8;

/// Hash table parameters: H5 for most inputs, H6 for large inputs at large windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Params {
    /// Bits of the number of buckets.
    pub(crate) bucket_bits: u32,
    /// Bits of the positions kept per bucket.
    pub(crate) block_bits: u32,
    /// Whether the hash reads 5 bytes of an 8-byte word (H6) instead of 4 (H5).
    pub(crate) wide: bool,
}

/// What the reference takes off a last distance's score for being the second to fourth
/// (`BackwardReferencePenaltyUsingLastDistance`): 39 plus 0 to 14, by a packed table.
pub(crate) fn last_distance_penalty(i: usize) -> usize {
    39 + ((0x1_ca10 >> (i & 0xe)) & 0xe)
}

/// The chain hasher, and the dictionary search that goes with it.
#[derive(Default)]
pub(crate) struct Chain {
    /// Positions filed per bucket, ever: a `u16` that wraps, as the reference's does.
    num: Vec<u16>,
    buckets: Vec<u32>,
    bucket_bits: u32,
    block_bits: u32,
    wide: bool,
    dictionary: Option<static_dict::Search>,
}

impl Chain {
    /// Empties the table for a new stream of about `size_hint` bytes with `params`, keeping
    /// its memory. The reference's table always has `2^bucket_bits` buckets; ours has one per
    /// byte of the size hint (rounded up to a power of two), up to that, and keeps the same
    /// hash, its top bits. That loses nothing measurable on bodies of 1 to 64 KiB (a quarter
    /// per byte lost up to 0.5%), and a 1 KiB body's table at quality 5 is 64 KiB, not 1 MiB.
    pub(crate) fn reset(&mut self, params: Params, size_hint: Option<usize>) {
        let bits = size_hint.map_or(params.bucket_bits, |n| {
            // ceil(log2(n)), without the overflow of next_power_of_two near usize::MAX.
            let log = usize::BITS - (n.max(1) - 1).leading_zeros();
            log.clamp(MIN_BUCKET_BITS, params.bucket_bits)
        });
        let params = Params {
            bucket_bits: bits,
            ..params
        };
        self.bucket_bits = params.bucket_bits;
        self.block_bits = params.block_bits;
        self.wide = params.wide;
        self.num.clear();
        self.num.resize(1 << params.bucket_bits, 0);
        // The bucket contents are never read past `num`, so they needn't be cleared.
        self.buckets
            .resize(1 << (params.bucket_bits + params.block_bits), 0);
        self.dictionary = Some(static_dict::Search::default());
    }

    /// Positions move `by` bytes down: the data before them was dropped. Positions that were
    /// in it become 0, out of reach as they were.
    pub(crate) fn rebase(&mut self, by: usize) {
        let by = by as u32;
        for b in &mut self.buckets {
            *b = b.saturating_sub(by);
        }
    }

    /// Bytes the table holds on to.
    pub(crate) fn capacity_bytes(&self) -> usize {
        self.num.capacity() * 2 + self.buckets.capacity() * 4
    }

    /// The bucket of the bytes at `data[pos..]`.
    #[inline]
    fn hash(&self, data: &[u8], pos: usize) -> usize {
        if self.wide {
            let b = &data[pos..pos + 8];
            let v = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
            ((v << 24).wrapping_mul(HASH_MUL) >> (64 - self.bucket_bits)) as usize
        } else {
            let b = &data[pos..pos + 4];
            let v = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            (v.wrapping_mul(0x1e35_a7bd) >> (32 - self.bucket_bits)) as usize
        }
    }
}

impl Finder for Chain {
    fn hash_len(&self) -> usize {
        if self.wide { 8 } else { 4 }
    }

    const FRESH_LAZY: bool = true;

    #[inline]
    fn store(&mut self, data: &[u8], pos: usize) {
        let key = self.hash(data, pos);
        let n = self.num[key];
        let mask = (1 << self.block_bits) - 1;
        self.buckets[(key << self.block_bits) + (usize::from(n) & mask)] = pos as u32;
        self.num[key] = n.wrapping_add(1);
    }

    fn store_range(&mut self, data: &[u8], range: Range<usize>) {
        for pos in range {
            self.store(data, pos);
        }
    }

    fn find_longest_match(
        &mut self,
        data: &[u8],
        ring: &Ring,
        cur: usize,
        max_length: usize,
        max_backward: usize,
        best: &mut Match,
    ) {
        let start_score = best.score;
        let here = &data[cur..cur + max_length];
        let mut best_len = best.len;
        // The last distances: a copy of 2 or 3 bytes is worth it from the first two.
        for (i, &distance) in ring.0.iter().enumerate() {
            let distance = distance as usize;
            if distance == 0 || distance > cur || distance > max_backward {
                continue;
            }
            let prev = cur - distance;
            if best_len >= max_length || data[prev + best_len] != here[best_len] {
                continue;
            }
            let len = match_length(&data[prev..], here, max_length);
            if len >= 3 || (len == 2 && i < 2) {
                let mut s = last_distance_score(len);
                if best.score < s {
                    if i != 0 {
                        s -= last_distance_penalty(i);
                    }
                    if best.score < s {
                        best_len = len;
                        *best = Match {
                            len,
                            len_code: len,
                            distance,
                            score: s,
                        };
                    }
                }
            }
        }
        // From here a match needs 4 bytes: compare the 4 ending one past the best length.
        best_len = best_len.max(3);
        let key = self.hash(data, cur);
        let n = self.num[key];
        let mask = (1 << self.block_bits) - 1;
        let bucket = key << self.block_bits;
        let oldest = usize::from(n).saturating_sub(1 << self.block_bits);
        for i in (oldest..usize::from(n)).rev() {
            let prev = self.buckets[bucket + (i & mask)] as usize;
            let distance = cur.wrapping_sub(prev);
            if distance > max_backward {
                break;
            }
            // The 4 bytes ending one past the best length.
            let at = best_len - 3;
            if best_len >= max_length || data[prev + at..prev + at + 4] != here[at..at + 4] {
                continue;
            }
            let len = match_length(&data[prev..], here, max_length);
            if len >= 4 {
                let s = score(len, distance);
                if best.score < s {
                    best_len = len;
                    *best = Match {
                        len,
                        len_code: len,
                        distance,
                        score: s,
                    };
                }
            }
        }
        self.buckets[bucket + (usize::from(n) & mask)] = cur as u32;
        self.num[key] = n.wrapping_add(1);
        if best.score == start_score
            && let Some(search) = &mut self.dictionary
        {
            search.search_deep(here, max_length, max_backward, best);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Chain, Params, last_distance_penalty};
    use crate::dictionary;
    use crate::encode::command::Ring;
    use crate::encode::hash::{Finder, MIN_SCORE, Match, last_distance_score};

    const H5_Q5: Params = Params {
        bucket_bits: 14,
        block_bits: 4,
        wide: false,
    };

    fn chain(params: Params) -> Chain {
        let mut c = Chain::default();
        c.reset(params, None);
        c
    }

    fn noise(len: usize, seed: u64) -> Vec<u8> {
        let mut x = seed;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    }

    #[test]
    fn the_penalties_are_the_references() {
        // 39 plus 0, 4 and 4: the bits of 0x1ca10 at 0 and 2, masked to 0xe.
        assert_eq!(
            (1..4).map(last_distance_penalty).collect::<Vec<_>>(),
            [39, 43, 43]
        );
    }

    #[test]
    fn a_planted_repeat_is_found() {
        for params in [
            H5_Q5,
            Params {
                bucket_bits: 15,
                block_bits: 5,
                wide: true,
            },
        ] {
            let mut data = noise(5000, 3);
            data.copy_within(100..140, 3000);
            let mut c = chain(params);
            c.store_range(&data, 0..3000);
            let mut best = Match::none(0);
            c.find_longest_match(&data, &Ring([1, 2, 3, 4]), 3000, 2000, 4000, &mut best);
            assert_eq!((best.len, best.distance), (40, 2900), "{params:?}");
        }
    }

    #[test]
    fn a_bucket_keeps_only_its_newest_positions() {
        // Copies 64 bytes apart that share their first 4 bytes, so one bucket, and differ
        // after. The current position repeats copy 0 for 16 bytes. At quality 5 a bucket
        // keeps 16 positions: with copy 0 and 15 more filed, copy 0 is found whole; one more
        // and it is forgotten, and only 4 bytes match.
        let mut data = noise(4000, 5);
        for k in 0..40 {
            data[k * 64..k * 64 + 4].copy_from_slice(b"abcd");
        }
        let cur = 40 * 64;
        data.copy_within(0..16, cur);
        data[cur + 16] = !data[16];
        for (later, len) in [(15, 16), (16, 4)] {
            let mut c = chain(H5_Q5);
            c.dictionary = None;
            c.store(&data, 0);
            for k in 40 - later..40 {
                c.store(&data, k * 64);
            }
            let mut best = Match::none(0);
            c.find_longest_match(&data, &Ring([1, 2, 3, 4]), cur, 64, 4000, &mut best);
            assert_eq!(best.len, len, "{later} filed after copy 0");
            if len == 16 {
                assert_eq!(best.distance, cur);
            }
        }
        // Nothing past the window.
        let mut c = chain(H5_Q5);
        c.store(&data, 0);
        let mut best = Match::none(0);
        c.find_longest_match(&data, &Ring([1, 2, 3, 4]), cur, 64, cur - 1, &mut best);
        assert_eq!(best.score, MIN_SCORE, "position 0 is out of reach");
    }

    #[test]
    fn a_last_distance_wins_short_copies() {
        // 2 bytes from the last distance score over the minimum; from the third, not.
        let mut data = noise(300, 7);
        data[200..202].copy_from_slice(&[0xaa, 0xbb]);
        data[100..102].copy_from_slice(&[0xaa, 0xbb]);
        data[202] = 0x11;
        data[102] = 0x22;
        // The first scores in full; the second less its penalty.
        for (ring, score) in [
            (Ring([100, 7, 8, 9]), Some(last_distance_score(2))),
            (Ring([7, 100, 8, 9]), Some(last_distance_score(2) - 39)),
            (Ring([7, 8, 100, 9]), None),
        ] {
            let mut c = chain(H5_Q5);
            let mut best = Match::none(0);
            c.dictionary = None;
            c.find_longest_match(&data, &ring, 200, 90, 200, &mut best);
            assert_eq!(best.score > MIN_SCORE, score.is_some(), "{ring:?}");
            if let Some(score) = score {
                assert_eq!((best.len, best.distance, best.score), (2, 100, score));
            }
        }
    }

    #[test]
    fn a_search_files_its_position() {
        let mut data = noise(3000, 13);
        data.copy_within(1000..1032, 2000);
        let mut c = chain(H5_Q5);
        let mut best = Match::none(0);
        c.find_longest_match(&data, &Ring([1, 2, 3, 4]), 1000, 32, 4000, &mut best);
        let mut best = Match::none(0);
        c.find_longest_match(&data, &Ring([1, 2, 3, 4]), 2000, 32, 4000, &mut best);
        assert_eq!((best.len, best.distance), (32, 1000));
    }

    #[test]
    fn the_hashes_read_4_and_5_bytes() {
        let h6 = Params {
            bucket_bits: 15,
            block_bits: 5,
            wide: true,
        };
        for (params, read) in [(H5_Q5, 4), (h6, 5)] {
            let c = chain(params);
            let data = noise(64, 17);
            let key = c.hash(&data, 0);
            assert!(key < 1 << params.bucket_bits);
            // A byte past those read changes nothing; one of them, something (for these bytes).
            let mut past = data.clone();
            past[read] ^= 0x5a;
            assert_eq!(c.hash(&past, 0), key, "{params:?}");
            let mut inside = data.clone();
            inside[read - 1] ^= 0x5a;
            assert_ne!(c.hash(&inside, 0), key, "{params:?}");
        }
    }

    #[test]
    fn a_short_dictionary_word_is_found() {
        // The most common 4-byte word always has its hash's short slot. Quality 4 doesn't
        // look up short words; 5 and 6 do, when the table has nothing.
        let mut data = dictionary::word(4, 0).to_vec();
        data.extend_from_slice(b"#####");
        let mut c = chain(H5_Q5);
        let mut best = Match::none(0);
        c.find_longest_match(&data, &Ring([1, 2, 3, 4]), 0, data.len(), 1000, &mut best);
        assert_eq!((best.len, best.len_code, best.distance), (4, 4, 1001));
    }

    #[test]
    fn a_hinted_table_has_a_bucket_per_byte() {
        let h6 = Params {
            bucket_bits: 15,
            block_bits: 5,
            wide: true,
        };
        let buckets = |params: Params, hint| {
            let mut c = Chain::default();
            c.reset(params, hint);
            (c.num.len(), c.buckets.len() >> params.block_bits)
        };
        assert_eq!(buckets(H5_Q5, None), (1 << 14, 1 << 14));
        assert_eq!(buckets(H5_Q5, Some(1000)), (1024, 1024));
        assert_eq!(buckets(H5_Q5, Some(1024)), (1024, 1024));
        assert_eq!(buckets(H5_Q5, Some(1025)), (2048, 2048));
        assert_eq!(buckets(H5_Q5, Some(0)), (256, 256));
        assert_eq!(buckets(H5_Q5, Some(usize::MAX)), (1 << 14, 1 << 14));
        assert_eq!(buckets(h6, Some(8 << 20)), (1 << 15, 1 << 15));
        // A repeat is still found in a small table.
        let mut data = noise(1000, 29);
        data.copy_within(100..132, 900);
        let mut c = Chain::default();
        c.reset(H5_Q5, Some(data.len()));
        c.store_range(&data, 0..900);
        let mut best = Match::none(0);
        c.find_longest_match(&data, &Ring([1, 2, 3, 4]), 900, 100, 1000, &mut best);
        assert_eq!((best.len, best.distance), (32, 800));
    }
}
