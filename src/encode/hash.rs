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
/// The hash multiplier (the reference's `kHashMul64`).
pub(crate) const HASH_MUL: u64 = 0x1fe3_5a7b_d357_9bd3;
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
pub(crate) fn last_distance_score(len: usize) -> usize {
    SCORE_BASE + LITERAL_BYTE_SCORE * len + 15
}

/// What the match loop needs of a hasher. Generic, not dynamic: quality 4's loop compiles
/// to what it was before there were others.
pub(crate) trait Finder {
    /// Bytes a position's hash reads (the reference's `HashTypeLength` and
    /// `StoreLookahead`): a position this close to the end of the input isn't searched.
    fn hash_len(&self) -> usize;
    /// Whether the search one byte later starts from nothing, as from quality 5, rather than
    /// from the match in hand cut by one (`MIN_QUALITY_FOR_EXTENSIVE_REFERENCE_SEARCH`).
    const FRESH_LAZY: bool;
    /// Files position `pos`.
    fn store(&mut self, data: &[u8], pos: usize);
    /// Files every position in `range`, in order.
    fn store_range(&mut self, data: &[u8], range: std::ops::Range<usize>);
    /// The best match for `data[cur..]`, up to `max_length` bytes and `max_backward` back,
    /// into `best` if it beats it; then files `cur`.
    fn find_longest_match(
        &mut self,
        data: &[u8],
        ring: &crate::encode::command::Ring,
        cur: usize,
        max_length: usize,
        max_backward: usize,
        best: &mut Match,
    );
}

impl Finder for Hasher {
    fn hash_len(&self) -> usize {
        HASH_READ
    }

    const FRESH_LAZY: bool = false;

    #[inline(always)]
    fn store(&mut self, data: &[u8], pos: usize) {
        Hasher::store(self, data, pos);
    }

    #[inline(always)]
    fn store_range(&mut self, data: &[u8], range: std::ops::Range<usize>) {
        Hasher::store_range(self, data, range);
    }

    #[inline(always)]
    fn find_longest_match(
        &mut self,
        data: &[u8],
        ring: &crate::encode::command::Ring,
        cur: usize,
        max_length: usize,
        max_backward: usize,
        best: &mut Match,
    ) {
        Hasher::find_longest_match(
            self,
            data,
            ring.last() as usize,
            cur,
            max_length,
            max_backward,
            best,
        );
    }
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
#[derive(Default)]
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
    #[cfg(test)]
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
            // ceil(log2(n)), without the overflow of next_power_of_two near usize::MAX.
            let log = usize::BITS - (n.max(1) - 1).leading_zeros();
            (log + SLOTS_PER_BYTE_BITS).clamp(MIN_BUCKET_BITS, params.bucket_bits)
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
        self.hash_word(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    /// The hash of the first bytes of `v` (little-endian): only the low `hash_len` count.
    #[inline]
    fn hash_word(&self, v: u64) -> usize {
        ((v << self.shift).wrapping_mul(HASH_MUL) >> self.take) as usize
    }

    /// Files position `pos` (with [`HASH_READ`] bytes after it) under its hash.
    #[inline]
    pub(crate) fn store(&mut self, data: &[u8], pos: usize) {
        let key = self.hash(data, pos) + (pos & SWEEP_MASK);
        self.buckets[key & self.mask] = pos as u32;
    }

    /// Files every position in `range`, in order, as [`store`](Self::store) would one by one.
    /// Four at a time while 16 bytes can be read: their words come from two reads, which on
    /// highly repetitive input (long copies, every position filed) took 40% of the time when
    /// each position read its own.
    pub(crate) fn store_range(&mut self, data: &[u8], range: std::ops::Range<usize>) {
        // The parameters in locals and the table cut to the mask (it is exactly that long):
        // otherwise each store could change them, as far as the compiler knows, and they would
        // be read again and the index checked against the length every time.
        let (shift, take, mask) = (self.shift, self.take, self.mask);
        let Some(buckets) = self.buckets.get_mut(..=mask) else {
            return;
        };
        let hash = |v: u64| ((v << shift).wrapping_mul(HASH_MUL) >> take) as usize;
        let mut pos = range.start;
        while pos + 4 <= range.end
            && let Some(bytes) = data.get(pos..pos + 16)
        {
            let (lo, hi) = bytes.split_at(8);
            let lo = u64::from_le_bytes(lo.try_into().unwrap_or_default());
            let hi = u64::from_le_bytes(hi.try_into().unwrap_or_default());
            for k in 0..4 {
                // The word at pos + k: lo shifted down k bytes, the gap filled from hi.
                let word = if k == 0 {
                    lo
                } else {
                    (lo >> (8 * k)) | (hi << (64 - 8 * k))
                };
                let at = pos + k;
                buckets[(hash(word) + (at & SWEEP_MASK)) & mask] = at as u32;
            }
            pos += 4;
        }
        for at in pos..range.end {
            let b = &data[at..at + HASH_READ];
            let word = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
            buckets[(hash(word) + (at & SWEEP_MASK)) & mask] = at as u32;
        }
    }

    /// The best match for `data[cur..]` up to `max_length` bytes and `max_backward` back,
    /// trying `last` (the last distance) first, into `best` if it beats it; then files `cur`.
    /// With nothing found, the dictionary is tried. Always inlined: out of line it cost 4% on a
    /// 1 MiB script, where most positions are searched.
    #[inline(always)]
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
        // The slots first: on a large table they are the slow reads, and this way they are
        // under way while the last distance is tried.
        let prevs: [u32; SWEEP] =
            std::array::from_fn(|i| self.buckets[(key + (i << 3)) & self.mask]);
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
        for prev in prevs {
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
    fn a_range_is_filed_as_one_position_at_a_time() {
        let mut x = 0x9e37_79b9_7f4a_7c15_u64;
        let data: Vec<u8> = (0..4096)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                // Runs and repeats, so slots collide and the order of filing matters.
                if i % 512 < 200 {
                    b'a' + (x % 3) as u8
                } else {
                    x as u8
                }
            })
            .collect();
        for params in [H4, H54] {
            for (start, end) in [
                (0, 0),
                (0, 3),
                (5, 9),
                (0, 4089),
                (7, 4000),
                (100, 4081),
                (4070, 4089),
            ] {
                let mut batched = Hasher::new(params, Some(256));
                let mut single = Hasher::new(params, Some(256));
                batched.store_range(&data, start..end);
                for pos in start..end {
                    single.store(&data, pos);
                }
                assert!(batched.buckets == single.buckets, "{start}..{end}");
            }
        }
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
        // Either side of a power of two, and hints far past any input.
        assert_eq!(Hasher::new(H4, Some(1025)).table_bytes(), 128 << 10);
        assert_eq!(Hasher::new(H4, Some(1)).table_bytes(), 4 << 10);
        for huge in [usize::MAX, (usize::MAX >> 1) + 2, (usize::MAX >> 1) + 1] {
            assert_eq!(Hasher::new(H4, Some(huge)).table_bytes(), 512 << 10);
            assert_eq!(Hasher::new(H54, Some(huge)).table_bytes(), 4 << 20);
        }
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
