//! The match finder's hash table: the reference's "quickly" hasher (`c/enc/hash.h`, H4 and
//! H54, and `c/enc/hash_longest_match_quickly_inc.h`).
//!
//! Each position is filed under a hash of its next 5 (H4) or 7 (H54) bytes, in one of four
//! slots eight apart, the slot chosen by the position. A lookup tries the last distance and
//! the four slots, and keeps the best-scoring match of at least 4 bytes; the score weighs
//! length against distance (`BackwardReferenceScore`).

use crate::encode::static_dict;

/// Hash table parameters: H4 for most inputs, H54 for inputs of 1 MiB or more.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Params {
    /// Bits of the table size, at most.
    bucket_bits: u32,
    /// Bytes hashed.
    hash_len: u32,
    /// Whether to look in the static dictionary when no copy is found.
    dictionary: bool,
}

pub(crate) const H4: Params = Params {
    bucket_bits: 17,
    hash_len: 5,
    dictionary: true,
};

pub(crate) const H54: Params = Params {
    bucket_bits: 20,
    hash_len: 7,
    dictionary: false,
};

/// Slots per hash, eight apart.
const SWEEP: usize = 4;
const SWEEP_MASK: usize = (SWEEP - 1) << 3;
/// Bytes a hash reads.
pub(crate) const HASH_READ: usize = 8;
/// A hinted table has `2^SLOTS_PER_BYTE_BITS` slots per byte of the hint (up to the
/// reference's size).
const SLOTS_PER_BYTE_BITS: u32 = 4;
/// The smallest table, in bits: big enough for the four slots of every hash.
const MIN_BUCKET_BITS: u32 = 10;

/// Score units: a byte copied instead of written as a literal, and a bit of distance.
const LITERAL_BYTE_SCORE: usize = 135;
const DISTANCE_BIT_PENALTY: usize = 30;
/// Keeps every score positive after the largest distance penalty.
const SCORE_BASE: usize = DISTANCE_BIT_PENALTY * 8 * 8;
/// A match must score more than this to be used.
pub(crate) const MIN_SCORE: usize = SCORE_BASE + 100;

/// The score of a copy of `len` bytes from `distance` back.
pub(crate) fn score(len: usize, distance: usize) -> usize {
    SCORE_BASE + LITERAL_BYTE_SCORE * len - DISTANCE_BIT_PENALTY * distance.ilog2() as usize
}

/// The score of a copy of `len` bytes from the last distance: cheaper than any other.
fn last_distance_score(len: usize) -> usize {
    SCORE_BASE + LITERAL_BYTE_SCORE * len + 15
}

/// A match: `len` bytes from `distance` back, its length written as `len_code` (more than
/// `len` for a dictionary word cut short), and its score.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Match {
    pub(crate) len: usize,
    pub(crate) len_code: usize,
    pub(crate) distance: usize,
    pub(crate) score: usize,
}

impl Match {
    /// No match yet, at length `len`: a match found must be longer, or as long and closer.
    pub(crate) fn none(len: usize) -> Self {
        Self {
            len,
            len_code: len,
            distance: 0,
            score: MIN_SCORE,
        }
    }
}

/// How many bytes `a` and `b` have in common at the start, at most `limit`; both must be at
/// least `limit` long.
pub(crate) fn match_length(a: &[u8], b: &[u8], limit: usize) -> usize {
    let (a, b) = (&a[..limit], &b[..limit]);
    let mut n = 0;
    for (x, y) in a.as_chunks::<8>().0.iter().zip(b.as_chunks::<8>().0) {
        let diff = u64::from_le_bytes(*x) ^ u64::from_le_bytes(*y);
        if diff != 0 {
            return n + (diff.trailing_zeros() / 8) as usize;
        }
        n += 8;
    }
    n + a[n..]
        .iter()
        .zip(&b[n..])
        .take_while(|(x, y)| x == y)
        .count()
}

/// The hash table, and the dictionary search that goes with it.
pub(crate) struct Hasher {
    buckets: Vec<u32>,
    mask: usize,
    /// `64 - 8 * hash_len`: shifts the hashed bytes to the top of a 64-bit word.
    shift: u32,
    /// `64 - bucket_bits`: takes the hash from the top of the product.
    take: u32,
    dictionary: Option<static_dict::Search>,
}

impl Hasher {
    /// A table for compressing a stream of about `size_hint` bytes with `params`. The
    /// reference's table always has `2^bucket_bits` slots; ours has 16 per byte of the size
    /// hint (rounded up to a power of two), up to that, and keeps the same hash, its top bits.
    /// A 1 KiB input gets 64 KiB, not 512 KiB, which is cheaper to clear. Fewer slots lose
    /// matches: at 2 per byte, a 4 KiB HTML body came out 0.5% larger, a 16 KiB one 0.6%.
    /// With no hint, the reference's size.
    pub(crate) fn new(params: Params, size_hint: Option<usize>) -> Self {
        let mut h = Self {
            buckets: Vec::new(),
            mask: 0,
            shift: 0,
            take: 0,
            dictionary: None,
        };
        h.reset(params, size_hint);
        h
    }

    /// Empties the table for a new stream, as [`new`](Self::new) would make it, keeping its
    /// memory.
    pub(crate) fn reset(&mut self, params: Params, size_hint: Option<usize>) {
        let bits = size_hint.map_or(params.bucket_bits, |n| {
            let wanted = n.max(1).next_power_of_two().ilog2() + SLOTS_PER_BYTE_BITS;
            wanted.clamp(MIN_BUCKET_BITS, params.bucket_bits)
        });
        self.buckets.clear();
        self.buckets.resize(1 << bits, 0);
        self.mask = (1 << bits) - 1;
        self.shift = 64 - 8 * params.hash_len;
        self.take = 64 - bits;
        self.dictionary = params.dictionary.then(static_dict::Search::default);
    }

    /// Positions move `by` bytes down: the data before them was dropped. Positions that
    /// were in it become 0 (any position is safe: matches are checked byte by byte).
    pub(crate) fn rebase(&mut self, by: usize) {
        let by = by as u32;
        for b in &mut self.buckets {
            *b = b.saturating_sub(by);
        }
    }

    /// Bytes the table holds on to.
    pub(crate) fn capacity_bytes(&self) -> usize {
        self.buckets.capacity() * 4
    }

    /// Bytes the table takes.
    #[cfg(test)]
    pub(crate) fn table_bytes(&self) -> usize {
        self.buckets.len() * 4
    }

    /// The hash of the bytes at `data[pos..]` (at least [`HASH_READ`] of them).
    #[inline]
    fn hash(&self, data: &[u8], pos: usize) -> usize {
        let b = &data[pos..pos + HASH_READ];
        let v = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        ((v << self.shift).wrapping_mul(0x1fe3_5a7b_d357_9bd3) >> self.take) as usize
    }

    /// Files position `pos` (with [`HASH_READ`] bytes after it) under its hash.
    #[inline]
    pub(crate) fn store(&mut self, data: &[u8], pos: usize) {
        let key = self.hash(data, pos) + (pos & SWEEP_MASK);
        self.buckets[key & self.mask] = pos as u32;
    }

    pub(crate) fn store_range(&mut self, data: &[u8], range: std::ops::Range<usize>) {
        for pos in range {
            self.store(data, pos);
        }
    }

    /// The best match for `data[cur..]` up to `max_length` bytes and `max_backward` back,
    /// trying `last` (the last distance) first, into `best` if it beats it; then files `cur`.
    /// With nothing found, the dictionary is tried.
    pub(crate) fn find_longest_match(
        &mut self,
        data: &[u8],
        last: usize,
        cur: usize,
        max_length: usize,
        max_backward: usize,
        best: &mut Match,
    ) {
        let start_score = best.score;
        let key = self.hash(data, cur);
        let here = &data[cur..cur + max_length];
        // A candidate must agree with `here` at `best.len` to be longer than the best match.
        let agrees = |data: &[u8], prev: usize, len: usize| {
            len < max_length && data[prev + len] == here[len]
        };
        if last <= cur && agrees(data, cur - last, best.len) {
            let len = match_length(&data[cur - last..], here, max_length);
            if len >= 4 {
                let s = last_distance_score(len);
                if best.score < s {
                    *best = Match {
                        len,
                        len_code: len,
                        distance: last,
                        score: s,
                    };
                }
            }
        }
        for i in 0..SWEEP {
            let prev = self.buckets[(key + (i << 3)) & self.mask];
            // Positions are kept in 32 bits; the bytes are compared, so a wrapped one only
            // misses.
            let distance = (cur as u32).wrapping_sub(prev) as usize;
            if distance == 0 || distance > max_backward || !agrees(data, cur - distance, best.len) {
                continue;
            }
            let len = match_length(&data[cur - distance..], here, max_length);
            if len >= 4 {
                let s = score(len, distance);
                if best.score < s {
                    *best = Match {
                        len,
                        len_code: len,
                        distance,
                        score: s,
                    };
                }
            }
        }
        if best.score == start_score
            && let Some(search) = &mut self.dictionary
        {
            search.search(here, max_length, max_backward, best);
        }
        self.buckets[(key + (cur & SWEEP_MASK)) & self.mask] = cur as u32;
    }
}

#[cfg(test)]
mod tests {
    use super::{H4, H54, Hasher, MIN_SCORE, Match, match_length, score};

    #[test]
    fn match_lengths() {
        let a = b"abcdefghijklmnopqrstuvwxyz";
        let mut b = *a;
        assert_eq!(match_length(a, &b, 26), 26);
        assert_eq!(match_length(a, &b, 11), 11);
        for at in 0..26 {
            b = *a;
            b[at] = b'#';
            assert_eq!(match_length(a, &b, 26), at, "differs at {at}");
        }
    }

    #[test]
    fn scores_prefer_long_and_near() {
        assert!(score(10, 100) > score(10, 1000));
        assert!(score(11, 1000) > score(10, 100));
        assert!(score(4, 1) > MIN_SCORE);
        // A 4-byte copy from far away isn't worth it.
        assert!(score(4, 1 << 20) < MIN_SCORE);
    }

    #[test]
    fn tables_follow_the_input_size() {
        assert_eq!(Hasher::new(H4, Some(1024)).table_bytes(), 64 << 10);
        assert_eq!(Hasher::new(H4, Some(1000)).table_bytes(), 64 << 10);
        assert_eq!(Hasher::new(H4, Some(0)).table_bytes(), 4 << 10);
        assert_eq!(Hasher::new(H4, Some(4 << 10)).table_bytes(), 256 << 10);
        assert_eq!(Hasher::new(H4, Some(32 << 10)).table_bytes(), 512 << 10);
        assert_eq!(Hasher::new(H4, Some(1 << 20)).table_bytes(), 512 << 10);
        assert_eq!(Hasher::new(H54, Some(8 << 20)).table_bytes(), 4 << 20);
        assert_eq!(Hasher::new(H4, None).table_bytes(), 512 << 10);
        // Reset smaller keeps the memory.
        let mut h = Hasher::new(H4, None);
        h.reset(H4, Some(100));
        assert_eq!(h.table_bytes(), 8 << 10);
        assert_eq!(h.capacity_bytes(), 512 << 10);
    }

    #[test]
    fn a_repeat_is_found_and_the_window_is_kept() {
        let mut data: Vec<u8> = (0..2000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        data.copy_within(100..132, 1500);
        let mut h = Hasher::new(H54, Some(data.len()));
        h.store_range(&data, 90..110);
        let mut best = Match::none(0);
        h.find_longest_match(&data, 4, 1500, 64, 1400, &mut best);
        assert_eq!((best.len, best.distance), (32, 1400));
        // One byte less of window: not found.
        let mut h = Hasher::new(H54, Some(data.len()));
        h.store_range(&data, 90..110);
        let mut best = Match::none(0);
        h.find_longest_match(&data, 4, 1500, 64, 1399, &mut best);
        assert_eq!(best, Match::none(0));
    }
}
