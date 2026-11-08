//! The match finder of qualities 5 and 6 for windows of 16 or less: the reference's H40
//! (`c/enc/hash_forgetful_chain_inc.h`).
//!
//! Each hash has a chain of the positions filed under it, newest first. A chain is a list of
//! nodes in one ring of 2^16 shared by all of them: a node holds how far back the next
//! position is (`delta`) and where the next node is (`next`). The ring is reused oldest
//! first, so old chains lose their tails to new nodes, or share them ("forgetful"). A lookup
//! tries the four last distances, the second to fourth only if a one-byte hash of the
//! position (`tiny`) agrees, then walks the chain for at most `max_hops` nodes.

use std::ops::Range;

use crate::encode::chain::last_distance_penalty;
use crate::encode::command::Ring;
use crate::encode::hash::{Finder, Match, last_distance_score, match_length, score};
use crate::encode::static_dict;

/// Bits of the number of chains.
const BUCKET_BITS: u32 = 15;
/// Nodes in the ring.
const NODES: usize = 1 << 16;
/// Positions whose low 16 bits index the tiny hashes.
const TINY: usize = 1 << 16;
/// A chain's newest position before anything is filed: far beyond any position, so the
/// distance to it is beyond any window (and the first node's delta is capped).
const NONE: u32 = 0xcccc_cccc;
/// The longest stream whose chains are cleared one by one when it ends, rather than all at
/// the next reset.
const CLEAN_LIMIT: usize = 1 << 12;
/// The largest delta a node holds: farther than any window here (2^16 - 16 at most), so a
/// walk reaching it stops.
const MAX_DELTA: usize = 0xffff;

/// The forgetful chain hasher, and the dictionary search that goes with it.
#[derive(Default)]
pub(crate) struct Forgetful {
    /// The newest position filed per hash.
    addr: Vec<u32>,
    /// The node of the newest position per hash.
    head: Vec<u16>,
    /// Per node: the distance from its position to the next's, and the next's node.
    delta: Vec<u16>,
    next: Vec<u16>,
    /// The low byte of each position's hash, by the position's low 16 bits.
    tiny: Vec<u8>,
    /// The node the next position takes.
    free: u16,
    max_hops: usize,
    dictionary: Option<static_dict::Search>,
    /// Whether the last stream cleared what it filed when it ended (see [`finish`]).
    ///
    /// [`finish`]: Self::finish
    clean: bool,
    /// Whether positions moved since the reset: the input no longer starts at 0.
    rebased: bool,
}

impl Forgetful {
    /// Empties the table for a new stream at `quality` (5 or 6), keeping its memory.
    ///
    /// The reference clears the newest positions and the tiny hashes for every stream (or,
    /// for a small input it has whole, the chains that input hashes to): 256 KiB, several
    /// microseconds, more than a 1 KiB body takes to match. Ours skips that when the last
    /// stream cleared up after itself (see [`finish`](Self::finish)). The heads and the nodes
    /// needn't be cleared: a chain's first node always holds the capped delta, so no walk goes
    /// past it to what came before.
    pub(crate) fn reset(&mut self, quality: u32) {
        self.max_hops = 8 << (quality - 4);
        if !self.clean || self.addr.len() != 1 << BUCKET_BITS {
            self.addr.clear();
            self.addr.resize(1 << BUCKET_BITS, NONE);
            self.tiny.clear();
            self.tiny.resize(TINY, 0);
        }
        self.clean = false;
        self.rebased = false;
        self.head.resize(1 << BUCKET_BITS, 0);
        self.delta.resize(NODES, 0);
        self.next.resize(NODES, 0);
        self.free = 0;
        self.dictionary = Some(static_dict::Search::default());
    }

    /// Positions move `by` bytes down: the data before them was dropped. A newest position in
    /// it becomes 0, out of reach as it was (the caller keeps more than a window); the tiny
    /// hashes move with their positions.
    pub(crate) fn rebase(&mut self, by: usize) {
        let down = by as u32;
        for a in &mut self.addr {
            if *a != NONE {
                *a = a.saturating_sub(down);
            }
        }
        self.rebased = true;
        self.clean = false;
        if !self.tiny.is_empty() {
            let n = self.tiny.len();
            self.tiny.rotate_left(by % n);
        }
    }

    /// Bytes the table holds on to.
    pub(crate) fn capacity_bytes(&self) -> usize {
        self.addr.capacity() * 4
            + (self.head.capacity() + self.delta.capacity() + self.next.capacity()) * 2
            + self.tiny.capacity()
    }

    /// The stream ended, and `data` was all of it. If it was short and its positions never
    /// moved, clears the chains and tiny hashes its positions could have filed, so the next
    /// reset needn't clear everything: as the reference prepares a small input, at the end.
    pub(crate) fn finish(&mut self, data: &[u8]) {
        if self.rebased || data.len() > CLEAN_LIMIT || self.addr.len() != 1 << BUCKET_BITS {
            return;
        }
        // Only positions with 4 bytes after them are filed.
        for pos in 0..data.len().saturating_sub(3) {
            self.addr[Self::hash(data, pos)] = NONE;
        }
        self.tiny[..data.len()].fill(0);
        self.clean = true;
    }

    /// Whether the next reset can skip clearing the table.
    #[cfg(test)]
    pub(crate) fn is_clean(&self) -> bool {
        self.clean
    }

    /// The chain of the bytes at `data[pos..]`.
    #[inline]
    fn hash(data: &[u8], pos: usize) -> usize {
        let b = &data[pos..pos + 4];
        let v = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        (v.wrapping_mul(0x1e35_a7bd) >> (32 - BUCKET_BITS)) as usize
    }

    /// Files `pos`, whose hash is `key`.
    #[inline]
    fn file(&mut self, key: usize, pos: usize) {
        let node = usize::from(self.free);
        self.free = self.free.wrapping_add(1);
        self.tiny[pos & (TINY - 1)] = key as u8;
        let delta = pos.wrapping_sub(self.addr[key] as usize).min(MAX_DELTA);
        self.delta[node] = delta as u16;
        self.next[node] = self.head[key];
        self.addr[key] = pos as u32;
        self.head[key] = node as u16;
    }
}

impl Finder for Forgetful {
    fn hash_len(&self) -> usize {
        4
    }

    const FRESH_LAZY: bool = true;

    #[inline]
    fn store(&mut self, data: &[u8], pos: usize) {
        self.file(Self::hash(data, pos), pos);
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
        let key = Self::hash(data, cur);
        // The last distances: a copy of 2 bytes or more is worth it from any of them. The
        // second to fourth are tried only where the tiny hash agrees.
        for (i, &distance) in ring.0.iter().enumerate() {
            let distance = distance as usize;
            let prev = cur.wrapping_sub(distance);
            if i > 0 && self.tiny[prev & (TINY - 1)] != key as u8 {
                continue;
            }
            if distance == 0 || distance > cur || distance > max_backward {
                continue;
            }
            let len = match_length(&data[prev..], here, max_length);
            if len >= 2 {
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
        let mut distance = 0usize;
        let mut delta = cur.wrapping_sub(self.addr[key] as usize);
        let mut node = usize::from(self.head[key]);
        for _ in 0..self.max_hops {
            distance = distance.wrapping_add(delta);
            if distance > max_backward {
                break;
            }
            let prev = cur - distance;
            delta = usize::from(self.delta[node]);
            node = usize::from(self.next[node]);
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
        self.file(key, cur);
        if best.score == start_score
            && let Some(search) = &mut self.dictionary
        {
            search.search_deep(here, max_length, max_backward, best);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Forgetful;
    use crate::encode::chain::last_distance_penalty;
    use crate::encode::command::Ring;
    use crate::encode::hash::{Finder, MIN_SCORE, Match, last_distance_score};

    const RING: Ring = Ring([1, 2, 3, 4]);

    fn forgetful(quality: u32) -> Forgetful {
        let mut f = Forgetful::default();
        f.reset(quality);
        f.dictionary = None;
        f
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

    fn find(f: &mut Forgetful, data: &[u8], ring: &Ring, cur: usize, max_backward: usize) -> Match {
        let mut best = Match::none(0);
        let max_length = (data.len() - cur).min(64);
        f.find_longest_match(data, ring, cur, max_length, max_backward, &mut best);
        best
    }

    #[test]
    fn the_hops_follow_the_quality() {
        assert_eq!((forgetful(5).max_hops, forgetful(6).max_hops), (16, 32));
    }

    #[test]
    fn a_planted_repeat_is_found() {
        let mut data = noise(5000, 3);
        data.copy_within(100..140, 3000);
        let mut f = forgetful(5);
        f.store_range(&data, 0..3000);
        let best = find(&mut f, &data, &RING, 3000, 4000);
        assert_eq!((best.len, best.distance), (40, 2900));
    }

    #[test]
    fn a_chain_longer_than_max_hops_is_cut() {
        // Copies 64 bytes apart that share their first 4 bytes, so one chain, and differ
        // after. The current position repeats copy 0 for 16 bytes. At quality 5 a walk takes
        // 16 hops: with copy 0 and 15 more filed, copy 0 is found whole; one more and it is
        // past the walk, and only 4 bytes match.
        let mut data = noise(4000, 5);
        for k in 0..40 {
            data[k * 64..k * 64 + 4].copy_from_slice(b"abcd");
        }
        let cur = 40 * 64;
        data.copy_within(0..16, cur);
        data[cur + 16] = !data[16];
        for (later, len) in [(15, 16), (16, 4)] {
            let mut f = forgetful(5);
            f.store(&data, 0);
            for k in 40 - later..40 {
                f.store(&data, k * 64);
            }
            let best = find(&mut f, &data, &RING, cur, 4000);
            assert_eq!(best.len, len, "{later} filed after copy 0");
        }
        // At quality 6 the walk is 32 hops.
        let mut f = forgetful(6);
        f.store(&data, 0);
        for k in 40 - 31..40 {
            f.store(&data, k * 64);
        }
        assert_eq!(find(&mut f, &data, &RING, cur, 4000).len, 16);
    }

    #[test]
    fn a_capped_delta_ends_the_walk() {
        // Position 70,000 is filed after position 0 with the same 16 bytes: its node's delta,
        // 70,000, is capped at 65,535, past a window of 16, so the walk stops there. Cut to
        // 16 bits instead it would lead to 65,536, which holds a longer copy (not filed).
        let mut data = noise(80_000, 7);
        let cur = 70_100;
        data.copy_within(cur..cur + 40, 65_536);
        data.copy_within(cur..cur + 16, 0);
        data.copy_within(cur..cur + 16, 70_000);
        let mut f = forgetful(6);
        f.store(&data, 0);
        f.store(&data, 70_000);
        let best = find(&mut f, &data, &RING, cur, (1 << 16) - 16);
        assert_eq!((best.len, best.distance), (16, 100));
    }

    #[test]
    fn the_tiny_hash_rejects_a_wrong_last_distance_candidate() {
        // 8 bytes at 200 repeat those at 100. The second last distance is tried only where
        // the tiny hash of 100 is the current position's: once 100 is filed. The first is
        // always tried.
        let mut data = noise(300, 11);
        data.copy_within(100..108, 200);
        assert_ne!(
            Forgetful::hash(&data, 200) as u8,
            0,
            "the test needs a nonzero tiny hash"
        );
        let mut f = forgetful(5);
        assert_eq!(
            find(&mut f, &data, &Ring([7, 100, 8, 9]), 200, 250).score,
            MIN_SCORE
        );
        let mut f = forgetful(5);
        let best = find(&mut f, &data, &Ring([100, 7, 8, 9]), 200, 250);
        assert_eq!((best.len, best.score), (8, last_distance_score(8)));
        let mut f = forgetful(5);
        f.tiny[100] = Forgetful::hash(&data, 200) as u8;
        let best = find(&mut f, &data, &Ring([7, 100, 8, 9]), 200, 250);
        let s = last_distance_score(8) - last_distance_penalty(1);
        assert_eq!((best.len, best.distance, best.score), (8, 100, s));
    }

    #[test]
    fn a_rebase_keeps_the_tiny_hashes_with_their_positions() {
        // Position 1,000 filed, then everything moves 96 down: the second last distance to
        // it still passes the tiny hash, and scores as a last distance.
        let mut data = noise(3000, 13);
        data.copy_within(1000..1016, 2000);
        let mut f = forgetful(5);
        f.store(&data, 1000);
        f.rebase(96);
        let best = find(&mut f, &data[96..], &Ring([7, 1000, 8, 9]), 2000 - 96, 1500);
        let s = last_distance_score(16) - last_distance_penalty(1);
        assert_eq!((best.len, best.distance, best.score), (16, 1000, s));
        // The newest position of a chain, dropped, is out of reach.
        let mut f = forgetful(5);
        f.store(&data, 50);
        f.rebase(96);
        assert_eq!(f.addr[Forgetful::hash(&data, 50)], 0);
    }

    /// Whether the table is as a full reset leaves it.
    fn as_new(f: &Forgetful) -> bool {
        f.addr.iter().all(|&a| a == super::NONE) && f.tiny.iter().all(|&b| b == 0)
    }

    /// Files every position of `data` as the match loop can, searching at each.
    fn fill(f: &mut Forgetful, data: &[u8]) {
        for cur in 0..data.len() - 3 {
            find(f, data, &RING, cur, 1000);
        }
    }

    #[test]
    fn a_short_stream_leaves_the_table_as_new_when_it_ends() {
        let data = noise(super::CLEAN_LIMIT, 17);
        let mut f = forgetful(5);
        fill(&mut f, &data);
        assert!(!as_new(&f));
        f.finish(&data);
        assert!(f.clean && as_new(&f));
        f.reset(6);
        assert!(!f.clean && as_new(&f));
    }

    #[test]
    fn a_long_or_moved_stream_is_cleared_at_the_next_reset() {
        let long = noise(super::CLEAN_LIMIT + 1, 19);
        let short = noise(3000, 23);
        for case in 0..3 {
            let mut f = forgetful(5);
            match case {
                0 => {
                    fill(&mut f, &long);
                    f.finish(&long);
                }
                1 => {
                    fill(&mut f, &short);
                    f.rebase(32);
                    f.finish(&short[32..]);
                }
                _ => {
                    fill(&mut f, &short);
                    f.finish(&short);
                    f.rebase(32);
                }
            }
            assert!(!f.clean, "case {case}");
            f.reset(5);
            assert!(as_new(&f), "case {case}");
        }
    }
}
