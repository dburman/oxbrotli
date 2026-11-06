//! Compressing at quality 4: the match loop, and cutting the input into meta-blocks.
//!
//! This follows the reference's approach at quality 4 (`c/enc/backward_references_inc.h`,
//! `CreateBackwardReferences`, and `c/enc/encode.c`, `EncodeData`):
//!
//! - The input goes through in blocks of 64 KiB. Matches stay within a block, except that a
//!   copy ending at a block's end may run on into the next (`ExtendLastCommand`).
//! - At each position the hasher gives its best match. If there is one, the next position is
//!   tried too, and wins if it scores 175 more ("lazy matching"), up to four times in a row.
//! - After 64 bytes with no match, positions are tried every 2 bytes, and after 256 more
//!   every 4: incompressible data goes faster and fills the table less.
//! - Blocks join one meta-block until the next wouldn't fit in `2^(1 + max(lgwin, 16))`
//!   bytes, or its commands or literals reach an eighth of that.

use crate::bitwriter::BitWriter;
use crate::encode::command::{Command, DistanceCode, Ring};
use crate::encode::hash::{H4, H54, HASH_READ, Hasher, MIN_SCORE, Match, Params};
use crate::encode::{MAX_MLEN, write_empty_last, write_meta_block, write_window};

/// The input block (`lgblock` 16 at quality 4).
pub(crate) const INPUT_BLOCK: usize = 1 << 16;
/// The window's last 16 bytes can't be reached (§9.1).
pub(crate) const WINDOW_GAP: usize = 16;
/// How much better a match one byte later must score to be taken instead.
const LAZY_GAIN: usize = 175;
/// Matches put off in a row, at most.
const MAX_DELAYS: usize = 4;
/// Literals in a row after which positions are skipped.
const SPARSE_AFTER: usize = 64;
/// Streams hinted up to this size get all the room they can need at the start.
const RESERVE_LIMIT: usize = 16 << 10;
/// Inputs this large use the bigger hash table (H54), as the reference does given the size.
const LARGE_INPUT: usize = 1 << 20;

/// The state of one stream being compressed.
pub(crate) struct Encoder {
    hasher: Hasher,
    ring: Ring,
    /// The ring as the last meta-block left it: an uncompressed meta-block goes back to it.
    saved_ring: Ring,
    /// The farthest a copy can reach: `2^lgwin - 16`.
    max_backward: usize,
    /// Where the meta-block being built starts.
    flushed: usize,
    commands: Vec<Command>,
    literals: Vec<u8>,
    /// Literals after the last command, not yet in one.
    pending: usize,
    /// Whether the last command copies a dictionary word (which can't be extended).
    last_is_dictionary: bool,
}

impl Encoder {
    /// An encoder for a stream of about `size_hint` bytes, if known, with window `lgwin`.
    pub(crate) fn new(size_hint: Option<usize>, lgwin: u32) -> Self {
        Self {
            hasher: Hasher::new(Self::params(size_hint), size_hint),
            ring: Ring::START,
            saved_ring: Ring::START,
            max_backward: (1 << lgwin) - WINDOW_GAP,
            flushed: 0,
            commands: Vec::new(),
            literals: Vec::new(),
            pending: 0,
            last_is_dictionary: false,
        }
    }

    /// The hasher for a stream of `size_hint` bytes: H54 for large ones, as the reference
    /// picks given the size.
    fn params(size_hint: Option<usize>) -> Params {
        if size_hint.is_some_and(|n| n >= LARGE_INPUT) {
            H54
        } else {
            H4
        }
    }

    /// Ready for a new stream, as [`new`](Self::new) would make it, keeping its memory.
    pub(crate) fn reset(&mut self, size_hint: Option<usize>, lgwin: u32) {
        self.hasher.reset(Self::params(size_hint), size_hint);
        self.ring = Ring::START;
        self.saved_ring = Ring::START;
        self.max_backward = (1 << lgwin) - WINDOW_GAP;
        self.flushed = 0;
        self.commands.clear();
        self.literals.clear();
        self.pending = 0;
        self.last_is_dictionary = false;
        // Room for a small stream of the size hinted, so that many of them never grow it:
        // each command but the last covers 3 bytes or more (a dictionary word cut to 3 is the
        // shortest copy that beats the minimum score). Larger streams grow it as they need,
        // rather than take the worst case's 5 bytes per byte up front.
        let n = size_hint.unwrap_or(0).min(RESERVE_LIMIT);
        self.literals.reserve(n);
        self.commands.reserve(n / 3 + 1);
    }

    /// Where the meta-block being built starts.
    pub(crate) fn flushed(&self) -> usize {
        self.flushed
    }

    /// Whether the meta-block should be written after the input block ending at `end`
    /// (`max_block` is the longest meta-block): the next block wouldn't fit, or the commands
    /// or literals have reached an eighth of it.
    pub(crate) fn full(&self, end: usize, max_block: usize) -> bool {
        end - self.flushed + INPUT_BLOCK > max_block
            || self.literals.len() >= max_block / 8
            || self.commands.len() >= max_block / 8
    }

    /// The input's positions move `by` bytes down: the caller dropped that much from the
    /// front, all of it before the meta-block being built and out of the window's reach.
    pub(crate) fn rebase(&mut self, by: usize) {
        debug_assert!(by <= self.flushed);
        self.hasher.rebase(by);
        self.flushed -= by;
    }

    /// Bytes held between streams.
    pub(crate) fn capacity_bytes(&self) -> usize {
        self.hasher.capacity_bytes()
            + self.commands.capacity() * size_of::<Command>()
            + self.literals.capacity()
    }

    /// Bytes held: the hash table and the meta-block's commands and literals, and what
    /// writing the meta-block will add (a code per command, and the prefix codes).
    #[cfg(test)]
    pub(crate) fn footprint(&self) -> usize {
        self.hasher.table_bytes()
            + self.commands.capacity() * size_of::<Command>()
            + self.literals.capacity()
            + self.commands.len() * size_of::<crate::encode::command::Coded>()
            + (256 + 704 + 64) * (4 + 1 + 2)
    }

    /// Finds the commands for `src[start..end]`, one input block.
    pub(crate) fn process(&mut self, src: &[u8], start: usize, end: usize) {
        let mut position = start;
        // The last three positions of the block before couldn't be hashed then: their bytes
        // ran into this block.
        if end - start >= HASH_READ - 1 && start >= 3 {
            self.hasher.store_range(src, start - 3..start);
        }
        if let Some(last) = self.commands.last_mut()
            && self.pending == 0
            && !self.last_is_dictionary
        {
            // Run the last copy on while the bytes keep matching.
            let distance = self.ring.last() as usize;
            let copy_start = start - last.copy as usize;
            if distance <= copy_start.min(self.max_backward) {
                while position < end && src[position] == src[position - distance] {
                    position += 1;
                }
                last.copy += (position - start) as u32;
            }
        }
        self.find_commands(src, position, end);
    }

    /// The match loop over `src[position..end]`.
    fn find_commands(&mut self, src: &[u8], mut position: usize, end: usize) {
        let store_end = if end - position >= HASH_READ {
            end - HASH_READ + 1
        } else {
            position
        };
        let mut sparse_from = position + SPARSE_AFTER;
        while position + HASH_READ < end {
            let mut max_length = end - position;
            let mut best = Match::none(0);
            let last = self.ring.last() as usize;
            self.hasher.find_longest_match(
                src,
                last,
                position,
                max_length,
                position.min(self.max_backward),
                &mut best,
            );
            // A dictionary word may tie the minimum; that isn't a match either.
            if best.score <= MIN_SCORE {
                self.pending += 1;
                position += 1;
                if position > sparse_from {
                    // Long without a match: try fewer positions, and file fewer.
                    let (step, reach) = if position > sparse_from + 4 * SPARSE_AFTER {
                        (4, 16)
                    } else {
                        (2, 8)
                    };
                    let jump = (position + reach).min(end - (HASH_READ - 1));
                    while position < jump {
                        self.hasher.store(src, position);
                        position += step;
                        self.pending += step;
                    }
                }
                continue;
            }
            // A match: see whether one starting a byte later is clearly better.
            let mut delays = 0;
            loop {
                max_length -= 1;
                let mut later = Match::none((best.len - 1).min(max_length));
                self.hasher.find_longest_match(
                    src,
                    last,
                    position + 1,
                    max_length,
                    (position + 1).min(self.max_backward),
                    &mut later,
                );
                if later.score < best.score + LAZY_GAIN {
                    break;
                }
                position += 1;
                self.pending += 1;
                best = later;
                delays += 1;
                if delays == MAX_DELAYS || position + HASH_READ >= end {
                    break;
                }
            }
            sparse_from = position + 2 * best.len + SPARSE_AFTER;
            self.push_command(src, position, best);
            // File the positions the copy covers, but not deep into a run: a short distance
            // repeated over a long copy would fill the table with one pattern.
            let mut store_start = position + 2;
            let store_stop = (position + best.len).min(store_end);
            if best.distance < best.len >> 2 {
                store_start = store_start
                    .max(position + best.len - (best.distance << 2))
                    .min(store_stop);
            }
            self.hasher.store_range(src, store_start..store_stop);
            position += best.len;
        }
        self.pending += end - position;
    }

    /// Ends the pending literals with the copy `m` at `position`.
    fn push_command(&mut self, src: &[u8], position: usize, m: Match) {
        let distance = if m.distance <= position.min(self.max_backward) {
            self.last_is_dictionary = false;
            self.ring.encode(m.distance as u32)
        } else {
            // A dictionary word: it doesn't go on the ring.
            self.last_is_dictionary = true;
            DistanceCode::explicit(m.distance as u32)
        };
        self.literals
            .extend_from_slice(&src[position - self.pending..position]);
        self.commands.push(Command {
            insert: self.pending as u32,
            copy: m.len_code as u32,
            distance,
        });
        self.pending = 0;
    }

    /// Writes the meta-block of `src[self.flushed..end]`.
    pub(crate) fn flush(&mut self, w: &mut BitWriter<'_>, src: &[u8], end: usize, is_last: bool) {
        if self.pending > 0 {
            self.literals
                .extend_from_slice(&src[end - self.pending..end]);
            self.commands.push(Command {
                insert: self.pending as u32,
                copy: 0,
                distance: DistanceCode::LAST,
            });
            self.pending = 0;
        }
        let compressed = write_meta_block(
            w,
            &src[self.flushed..end],
            &self.literals,
            &self.commands,
            is_last,
        );
        if !compressed {
            // The decoder's ring didn't move: neither does ours.
            self.ring = self.saved_ring;
        }
        self.saved_ring = self.ring;
        self.flushed = end;
        self.commands.clear();
        self.literals.clear();
    }
}

/// The longest meta-block for window `lgwin`: `2^(1 + max(lgwin, 16))`, at most 16 MiB.
pub(crate) fn max_meta_block(lgwin: u32) -> usize {
    (1 << (1 + lgwin.max(16))).min(MAX_MLEN)
}

/// Compresses `src` into `out` at quality 4 with window `lgwin`.
pub(crate) fn compress(src: &[u8], lgwin: u32, out: &mut Vec<u8>) {
    let mut w = BitWriter::new(out);
    write_window(&mut w, lgwin);
    if src.is_empty() {
        write_empty_last(&mut w);
        w.finish();
        return;
    }
    let mut e = Encoder::new(Some(src.len()), lgwin);
    let max_block = max_meta_block(lgwin);
    let mut start = 0;
    while start < src.len() {
        let end = (start + INPUT_BLOCK).min(src.len());
        e.process(src, start, end);
        let is_last = end == src.len();
        if is_last || e.full(end, max_block) {
            e.flush(&mut w, src, end, is_last);
        }
        start = end;
    }
    w.finish();
}

#[cfg(test)]
mod tests {
    use super::{Encoder, compress};
    use crate::bitwriter::BitWriter;
    use crate::encode::command::{DistanceCode, Ring};
    use crate::encode::write_window;

    fn noise(len: usize, seed: u32) -> Vec<u8> {
        let mut x = seed;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    }

    fn round_trip(src: &[u8], lgwin: u32) -> Vec<u8> {
        let mut out = Vec::new();
        compress(src, lgwin, &mut out);
        assert!(
            crate::decompress(&out).unwrap() == src,
            "{} bytes, lgwin {lgwin}",
            src.len()
        );
        out
    }

    /// The distances the encoder's commands copy from, with a 64-byte repeat of noise
    /// planted `distance` bytes after its first copy.
    fn planted(distance: usize) -> Vec<DistanceCode> {
        let mut src = noise(3000, 9);
        src.copy_within(200..264, 200 + distance);
        let mut e = Encoder::new(Some(src.len()), 10);
        e.process(&src, 0, src.len());
        round_trip(&src, 10);
        e.commands.iter().map(|c| c.distance).collect()
    }

    #[test]
    fn a_repeat_at_the_edge_of_the_window_is_used_and_one_past_it_is_not() {
        // Window 10: copies reach 2^10 - 16 = 1008 bytes back.
        assert!(planted(1008).contains(&DistanceCode::explicit(1008)));
        let past = planted(1009);
        assert!(
            past.iter().all(|&d| d != DistanceCode::explicit(1009)),
            "{past:?}"
        );
    }

    #[test]
    fn text_shrinks_and_round_trips_at_every_window() {
        let text = b"It was the best of times, it was the worst of times, it was the age of \
            wisdom, it was the age of foolishness, it was the epoch of belief. "
            .repeat(2000);
        for lgwin in 10..=24 {
            let out = round_trip(&text, lgwin);
            assert!(
                out.len() < text.len() / 50,
                "lgwin {lgwin}: {} bytes",
                out.len()
            );
        }
    }

    #[test]
    fn copies_run_on_across_input_blocks() {
        // One long run: the copy that ends at the first input block's end goes on.
        let src = vec![b'a'; 300_000];
        let mut e = Encoder::new(Some(src.len()), 22);
        e.process(&src, 0, 1 << 16);
        let before = e.commands.len();
        e.process(&src, 1 << 16, 2 << 16);
        assert_eq!(e.commands.len(), before, "no new command");
        let out = round_trip(&src, 22);
        assert!(out.len() < 100, "{} bytes", out.len());
    }

    #[test]
    fn noise_is_stored_and_mixed_input_round_trips() {
        let src = noise(200_000, 3);
        let out = round_trip(&src, 22);
        assert!(out.len() <= src.len() + src.len() / 16 + 16);
        let mut mixed = noise(70_000, 4);
        mixed.extend(b"abcabcabd".repeat(20_000));
        mixed.extend(noise(70_000, 5));
        mixed.extend_from_within(1000..90_000);
        for lgwin in [10, 16, 22] {
            round_trip(&mixed, lgwin);
        }
    }

    #[test]
    fn a_stored_meta_block_leaves_the_ring_as_it_was() {
        // The first meta-block holds every byte value equally often, shuffled, so no prefix
        // code beats storing it; say its commands put 777 on the encoder's ring. The second
        // copies from 777 back: the decoder never saw 777 go on its ring, so the encoder
        // must not use short code 0 for it.
        let mut stored: Vec<u8> = (0..1u32 << 16).map(|i| i as u8).collect();
        let order = noise(stored.len(), 11);
        for i in (1..stored.len()).rev() {
            let r = usize::from(order[i]) << 8 | usize::from(order[i - 1]);
            stored.swap(i, r % (i + 1));
        }
        let mut src = stored.clone();
        let mut text = noise(800, 12);
        for _ in 0..40 {
            text.extend_from_within(text.len() - 777..text.len() - 777 + 60);
        }
        src.extend_from_slice(&text);

        let mut out = Vec::new();
        let mut w = BitWriter::new(&mut out);
        write_window(&mut w, 16);
        let mut e = Encoder::new(Some(src.len()), 16);
        e.process(&src, 0, stored.len());
        e.ring.push(777);
        e.flush(&mut w, &src, stored.len(), false);
        assert_eq!(e.ring, Ring::START, "the stored meta-block left the ring");
        e.process(&src, stored.len(), src.len());
        e.flush(&mut w, &src, src.len(), true);
        w.finish();
        assert!(
            out.windows(64).any(|w| w == &stored[1000..1064]),
            "the first meta-block is stored"
        );
        assert!(crate::decompress(&out).unwrap() == src, "round trip");
    }

    #[test]
    fn short_inputs_round_trip() {
        let text = b"hello hello hello hello, the world of the dictionary; information";
        for len in 0..text.len() {
            round_trip(&text[..len], 22);
        }
    }

    /// Not a check: the encoder's bytes for the sizes and windows oxgate's `brotli_pool.rs`
    /// measures (run with --nocapture).
    #[test]
    #[ignore = "a measurement"]
    fn footprint() {
        let words: Vec<&[u8]> =
            b"the of and to in is that for it as was with be by on not he this are or \
            his from at which but have an they you were her she there would their we him been"
                .split(|&b| b == b' ')
                .filter(|w| !w.is_empty())
                .collect();
        let pick = noise(1 << 20, 21);
        for (len, lgwin, kind) in [
            (1024usize, 10u32, "pool"),
            (32 << 10, 15, "pool"),
            (1 << 20, 19, "pool"),
            (1024, 10, "text"),
            (32 << 10, 15, "text"),
            (1 << 20, 19, "text"),
        ] {
            let src: Vec<u8> = if kind == "pool" {
                (0..len).map(|i| b"abcdefghij <p>"[i % 14]).collect()
            } else {
                let mut t = Vec::new();
                let mut i = 0;
                while t.len() < len {
                    t.extend_from_slice(words[pick[i] as usize % words.len()]);
                    t.push(if pick[i + 1].is_multiple_of(13) {
                        b'.'
                    } else {
                        b' '
                    });
                    i += 2;
                }
                t.truncate(len);
                t
            };
            let mut e = Encoder::new(Some(src.len()), lgwin);
            let mut peak = 0;
            let mut start = 0;
            while start < src.len() {
                let end = (start + super::INPUT_BLOCK).min(src.len());
                e.process(&src, start, end);
                peak = peak.max(e.footprint());
                start = end;
            }
            let mut out = Vec::new();
            compress(&src, lgwin, &mut out);
            println!(
                "{kind} {len} bytes, lgwin {lgwin}: {} KiB (table {} KiB), output {} bytes",
                (peak + out.capacity()) / 1024,
                e.hasher.table_bytes() / 1024,
                out.len()
            );
        }
    }

    #[test]
    fn dictionary_words_are_used() {
        // Words of 8 or more letters with nothing earlier to copy from.
        let src = b"information international environment ";
        let mut e = Encoder::new(Some(src.len()), 22);
        e.process(src, 0, src.len());
        assert!(e.last_is_dictionary || e.commands.iter().any(|c| c.distance.code >= 16));
        round_trip(src, 22);
    }
}
