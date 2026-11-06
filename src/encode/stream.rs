//! Compressing a stream that arrives in pieces, with flushes in between.
//!
//! The input goes into a buffer that holds the window and the meta-block being built. It is
//! cut into the same 64 KiB blocks as one-shot compression, and a block is processed once the
//! byte after it arrives: by then it is known not to be the last. So a stream with no flushes
//! and the same size hint compresses exactly as [`compress`](super::matcher::compress) does.
//!
//! A flush processes the partial block, ends the meta-block and, if the stream isn't on a byte
//! boundary, adds an empty metadata meta-block (`ISLAST = 0`, `MNIBBLES` = metadata, reserved
//! 0, `MSKIPBYTES = 0`), after which the decoder skips to a byte: the reference's flush.
//! Copies may still reach back across it.
//!
//! Once a meta-block is written, the bytes before its window are dropped, when they are at
//! least a window's worth (so each byte is moved once or twice, not once per flush). The
//! buffer holds at most two windows, the longest meta-block and an input block:
//! `2^(lgwin + 1) + 2^(1 + max(lgwin, 16)) + 2^16` bytes, 16 MiB at the default window.

use crate::bitwriter::{BitWriter, Pending};
use crate::encode::matcher::{Encoder, INPUT_BLOCK, WINDOW_GAP, max_meta_block};
use crate::encode::{write_empty_last, write_window};

/// An empty metadata meta-block: `ISLAST` 0, `MNIBBLES` 0 (`11`), reserved 0,
/// `MSKIPBYTES` 0; then the decoder skips to a byte boundary.
const EMPTY_METADATA: (u32, u32) = (6, 0b0000_0110);

/// Positions move down by multiples of this when bytes are dropped.
const POSITION_ALIGN: usize = 32;

/// The state of a stream compressor.
pub(crate) struct Stream {
    /// The window of the stream under way.
    lgwin: u32,
    /// The window for the next stream.
    next_lgwin: u32,
    size_hint: Option<usize>,
    encoder: Encoder,
    /// The window and the input not yet written: `buffer[processed..]` is the start of the
    /// next input block.
    buffer: Vec<u8>,
    processed: usize,
    /// Bits of the stream written but not yet a byte.
    pending: Pending,
    /// Whether a stream has begun: the encoder is set up at the first call of each stream.
    running: bool,
}

impl Stream {
    pub(crate) fn new(lgwin: u32, size_hint: Option<usize>) -> Self {
        Self {
            lgwin,
            next_lgwin: lgwin,
            size_hint,
            // The smallest table: the stream's own is made at its first call.
            encoder: Encoder::new(Some(0), lgwin),
            buffer: Vec::new(),
            processed: 0,
            pending: Pending::default(),
            running: false,
        }
    }

    /// The size hint for the next stream.
    pub(crate) fn set_size_hint(&mut self, size_hint: Option<usize>) {
        self.size_hint = size_hint;
    }

    /// The window for the next stream.
    pub(crate) fn set_lgwin(&mut self, lgwin: u32) {
        self.next_lgwin = lgwin;
    }

    /// Drops the stream under way, if any.
    pub(crate) fn reset(&mut self) {
        self.running = false;
    }

    /// Sets up for a new stream if one isn't under way: the window header is its first bits.
    fn begin(&mut self) {
        if self.running {
            return;
        }
        self.lgwin = self.next_lgwin;
        self.encoder.reset(self.size_hint, self.lgwin);
        self.buffer.clear();
        // Room for the stream hinted, up to what the buffer ever holds.
        let most = (2 << self.lgwin) + max_meta_block(self.lgwin) + INPUT_BLOCK + 1;
        self.buffer.reserve(self.size_hint.unwrap_or(0).min(most));
        self.processed = 0;
        let mut none = Vec::new();
        let mut w = BitWriter::new(&mut none);
        write_window(&mut w, self.lgwin);
        self.pending = w.suspend();
        debug_assert!(none.is_empty(), "the window header is under a byte");
        self.running = true;
    }

    pub(crate) fn write(&mut self, mut data: &[u8], out: &mut Vec<u8>) {
        self.begin();
        let mut w = BitWriter::resume(out, self.pending);
        let max_block = max_meta_block(self.lgwin);
        while !data.is_empty() {
            // Up to one byte past the next input block.
            let room = self.processed + INPUT_BLOCK + 1 - self.buffer.len();
            let (now, later) = data.split_at(room.min(data.len()));
            self.buffer.extend_from_slice(now);
            data = later;
            if self.buffer.len() > self.processed + INPUT_BLOCK {
                let end = self.processed + INPUT_BLOCK;
                self.encoder.process(&self.buffer, self.processed, end);
                self.processed = end;
                if self.encoder.full(end, max_block) {
                    self.encoder.flush(&mut w, &self.buffer, end, false);
                    self.trim();
                }
            }
        }
        self.pending = w.suspend();
    }

    /// Processes the input not yet processed, the end of a block.
    fn process_rest(&mut self) {
        let end = self.buffer.len();
        if end > self.processed {
            self.encoder.process(&self.buffer, self.processed, end);
            self.processed = end;
        }
    }

    pub(crate) fn flush(&mut self, out: &mut Vec<u8>) {
        self.begin();
        let mut w = BitWriter::resume(out, self.pending);
        self.process_rest();
        if self.encoder.flushed() < self.buffer.len() {
            self.encoder
                .flush(&mut w, &self.buffer, self.buffer.len(), false);
            self.trim();
        }
        if w.bit_offset() != 0 {
            w.put(EMPTY_METADATA.0, EMPTY_METADATA.1);
            w.align();
        }
        self.pending = w.suspend();
    }

    pub(crate) fn finish(&mut self, out: &mut Vec<u8>) {
        self.begin();
        let mut w = BitWriter::resume(out, self.pending);
        self.process_rest();
        if self.encoder.flushed() < self.buffer.len() {
            self.encoder
                .flush(&mut w, &self.buffer, self.buffer.len(), true);
        } else {
            write_empty_last(&mut w);
        }
        w.finish();
        self.pending = Pending::default();
        self.running = false;
    }

    /// Drops the bytes no copy can reach any more, once there is a window's worth of them.
    /// At least a byte more than the window is kept: a table entry pointing into the dropped
    /// bytes becomes 0, and must stay out of reach, as it was. And positions move by a
    /// multiple of 32, so each still goes into the table slot it went into before (the slot is
    /// picked by the position's bits 3 and 4).
    fn trim(&mut self) {
        let keep = (1 << self.lgwin) - WINDOW_GAP + 1;
        let flushed = self.encoder.flushed();
        if flushed >= 2 * keep {
            let by = (flushed - keep) & !(POSITION_ALIGN - 1);
            self.buffer.drain(..by);
            self.processed -= by;
            self.encoder.rebase(by);
        }
    }

    /// Bytes held between streams.
    pub(crate) fn footprint(&self) -> usize {
        self.encoder.capacity_bytes() + self.buffer.capacity()
    }

    /// Bytes of input held now.
    #[cfg(test)]
    fn buffered(&self) -> usize {
        self.buffer.len()
    }
}

#[cfg(test)]
mod tests {
    use super::Stream;
    use crate::encode::matcher::compress;

    fn text(len: usize) -> Vec<u8> {
        let words = [
            "stream ", "window ", "block ", "copy ", "literal ", "flush ", "brotli ",
        ];
        let mut x = 7u32;
        let mut out = Vec::new();
        while out.len() < len {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            out.extend_from_slice(words[(x >> 16) as usize % words.len()].as_bytes());
            out.extend_from_slice(&x.to_le_bytes()[..(x >> 29) as usize % 3]);
        }
        out.truncate(len);
        out
    }

    #[test]
    fn without_flushes_a_stream_is_compressed_as_a_whole() {
        // Past several meta-blocks and trims at window 16, and short ones at 22.
        for (len, lgwin, chunk) in [
            (700_000, 16, 4096),
            (300_000, 16, 65_536),
            (200_000, 22, 1000),
            (65_536, 22, 65_536),
            (65_537, 22, 7),
            (100, 18, 1),
        ] {
            let src = text(len);
            let mut whole = Vec::new();
            compress(&src, lgwin, &mut whole);
            let mut s = Stream::new(lgwin, Some(len));
            let mut out = Vec::new();
            for piece in src.chunks(chunk) {
                s.write(piece, &mut out);
            }
            s.finish(&mut out);
            assert!(out == whole, "{len} bytes at window {lgwin}");
        }
    }

    #[test]
    fn the_buffer_stays_within_its_bound() {
        let lgwin = 16;
        let bound = (2 << lgwin) + super::max_meta_block(lgwin) + super::INPUT_BLOCK + 1;
        let src = text(3 << 20);
        let mut s = Stream::new(lgwin, None);
        let mut out = Vec::new();
        let mut most = 0;
        for (i, piece) in src.chunks(10_000).enumerate() {
            s.write(piece, &mut out);
            if i % 3 == 0 {
                s.flush(&mut out);
            }
            most = most.max(s.buffered());
        }
        s.finish(&mut out);
        assert!(most <= bound, "{most} bytes held, bound {bound}");
        assert!(most > 1 << lgwin, "{most} bytes held");
        assert!(crate::decompress(&out).unwrap() == src, "round trip");
    }

    #[test]
    fn a_trim_keeps_dropped_positions_out_of_reach() {
        // After a trim, the meta-block starts past the window's reach from position 0, where
        // the table's dropped entries now point. Flushes 16 bytes past a multiple of 32 put
        // the trim exactly at the edge if the margin were missing.
        let lgwin = 16;
        let max_backward = (1 << lgwin) - super::WINDOW_GAP;
        let src = text(1 << 20);
        let mut s = Stream::new(lgwin, None);
        let mut out = Vec::new();
        s.write(&src[..16], &mut out);
        let mut written = 16;
        let mut trims = 0;
        for piece in src[16..].chunks(8192) {
            s.write(piece, &mut out);
            s.flush(&mut out);
            written += piece.len();
            if s.buffered() < written {
                trims += 1;
                assert!(s.encoder.flushed() > max_backward, "at {written}");
            }
        }
        assert!(trims > 0);
        s.finish(&mut out);
        assert!(crate::decompress(&out).unwrap() == src, "round trip");
    }

    #[test]
    fn two_flushes_add_nothing() {
        let mut s = Stream::new(22, None);
        let mut out = Vec::new();
        s.flush(&mut out);
        // The window header and an empty metadata block, padded.
        let first = out.len();
        assert!(first > 0);
        s.flush(&mut out);
        assert_eq!(out.len(), first);
        s.write(b"some bytes", &mut out);
        s.flush(&mut out);
        let second = out.len();
        s.flush(&mut out);
        assert_eq!(out.len(), second);
        s.finish(&mut out);
        assert_eq!(crate::decompress(&out).unwrap(), b"some bytes");
    }
}
