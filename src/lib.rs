//! Brotli (RFC 7932) compression and decompression in safe Rust, with no dependencies.
//!
//! Brotli is the compression format behind `Content-Encoding: br` on the web. This crate
//! reads and writes it without `unsafe` and without C: anything the reference `brotli` tool
//! writes decompresses here, and anything this crate writes decompresses with the reference.
//!
//! **Status:** 0.2.1. Decompression works. Compression works at qualities 4, 5 and 6, as the
//! reference compresses at each: on our test corpus the output is the same size, give or take
//! 0.5% per entry (a few bytes on the smallest), in one go or as a stream
//! ([`StreamCompressor`]), at about the reference's speed. Quality 4 is the fastest; 5
//! searches further for matches and codes literals by context, for output 1% to 14% smaller at
//! 56% to 72% of the speed; 6 searches further still, another 1% to 6% smaller at 45% to 69%
//! of 4's speed. Qualities 0 to 3 compress as 4 does, and
//! 7 to 11 as 6 does.
//!
//! ```
//! let text = b"Brotli in safe Rust. ".repeat(100);
//! let stream = oxbrotli::compress(&text, 4);
//! assert_eq!(oxbrotli::decompress(&stream).unwrap(), text);
//!
//! // "hello" as one uncompressed meta-block, written by hand.
//! let stream = [0x0b, 0x02, 0x80, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x03];
//! assert_eq!(oxbrotli::decompress(&stream).unwrap(), b"hello");
//! ```

#![forbid(unsafe_code)]

mod bits;
mod bitwriter;
mod context;
mod context_lut;
mod decode;
mod dictionary;
mod dictionary_tables;
mod encode;
mod prefix;
mod tables;

use encode::Level;

/// How to compress: see [`compress_with`].
///
/// ```
/// let options = oxbrotli::Options::new(4).lgwin(18);
/// let stream = oxbrotli::compress_with(b"some bytes", options);
/// assert_eq!(oxbrotli::decompress(&stream).unwrap(), b"some bytes");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Options {
    /// The quality, 0 to 11 as in the reference `brotli`: higher is smaller and slower.
    /// 4, 5 and 6 compress as the reference's do; 0 to 3 compress as 4, and 7 to 11 as 6.
    pub quality: u32,
    /// The window: back-references reach up to `2^lgwin - 16` bytes, and a decoder keeps that
    /// much of the output. 10 to 24; values outside are clamped. The default is 22, as the
    /// reference's.
    pub lgwin: u32,
}

impl Options {
    /// Options for `quality`, with the default window.
    #[must_use]
    pub fn new(quality: u32) -> Self {
        Self { quality, lgwin: 22 }
    }

    /// Options for `quality` with the smallest window that holds `size_hint` bytes, between
    /// 10 and `max_lgwin` (clamped to 10..=24), or `max_lgwin` when the size isn't known. A
    /// decoder needs no more memory than the window, so a small body is cheap to decode.
    ///
    /// ```
    /// use oxbrotli::Options;
    /// assert_eq!(Options::for_size(4, Some(500), 19).lgwin, 10);
    /// assert_eq!(Options::for_size(4, Some(5000), 19).lgwin, 13);
    /// assert_eq!(Options::for_size(4, Some(1 << 30), 19).lgwin, 19);
    /// assert_eq!(Options::for_size(4, None, 19).lgwin, 19);
    /// ```
    #[must_use]
    pub fn for_size(quality: u32, size_hint: Option<u64>, max_lgwin: u32) -> Self {
        let max = max_lgwin.clamp(encode::MIN_LGWIN, encode::MAX_LGWIN);
        let lgwin = size_hint.map_or(max, |n| {
            // Copies reach 2^w - 16 back: a body of n bytes needs no more than that.
            (encode::MIN_LGWIN..max)
                .find(|&w| (1u64 << w) - 16 >= n)
                .unwrap_or(max)
        });
        Self::new(quality).lgwin(lgwin)
    }

    /// These options with window `lgwin`, clamped to 10..=24.
    #[must_use]
    pub fn lgwin(self, lgwin: u32) -> Self {
        Self {
            lgwin: lgwin.clamp(encode::MIN_LGWIN, encode::MAX_LGWIN),
            ..self
        }
    }
}

impl Default for Options {
    /// Quality 4, the window 22.
    fn default() -> Self {
        Self::new(4)
    }
}

/// Compresses `src` at `quality` (0 to 11; 0 to 3 compress as 4, and 7 to 11 as 6) with the
/// default window.
#[must_use]
pub fn compress(src: &[u8], quality: u32) -> Vec<u8> {
    compress_with(src, Options::new(quality))
}

/// Compresses `src` with `options`.
#[must_use]
pub fn compress_with(src: &[u8], options: Options) -> Vec<u8> {
    let lgwin = options.lgwin.clamp(encode::MIN_LGWIN, encode::MAX_LGWIN);
    let mut out = Vec::new();
    encode::matcher::compress(src, lgwin, Level::from_quality(options.quality), &mut out);
    out
}

/// A compressor for a stream that arrives in pieces, such as a response body.
///
/// [`write`](Self::write) takes input and appends to `out` the meta-blocks it completes;
/// [`flush`](Self::flush) makes everything taken so far decodable; [`finish`](Self::finish)
/// ends the stream, after which the compressor starts a new one, keeping its memory. With no
/// flushes, a stream compresses exactly as [`compress_with`] would compress it whole, given the
/// same size hint.
///
/// It holds the hash table and at most `2^(lgwin + 1) + 2^(1 + max(lgwin, 16)) + 64 KiB` bytes
/// of input: two windows, the longest meta-block and an input block (16 MiB at the default
/// window; 1.1 MiB at window 18). The hash table is sized to the size hint, up to: at quality
/// 4, 512 KiB, or 4 MiB with a hint of 1 MiB or more; at 5, 1 MiB, or 2 MiB with such a hint at
/// windows from 19; at 6, 2 MiB, or 4 MiB with such a hint. At 5 and 6 and windows of 16 or
/// less it is a fixed 512 KiB instead, as the reference's, whatever the hint.
///
/// ```
/// let mut c = oxbrotli::StreamCompressor::new(oxbrotli::Options::new(4), None);
/// let mut out = Vec::new();
/// c.write(b"hello ", &mut out);
/// c.flush(&mut out);
/// c.write(b"world", &mut out);
/// c.finish(&mut out);
/// assert_eq!(oxbrotli::decompress(&out).unwrap(), b"hello world");
/// ```
pub struct StreamCompressor {
    stream: encode::stream::Stream,
}

impl StreamCompressor {
    /// A stream compressor; `size_hint` is the stream's expected size, if known, to fit the
    /// hash table to it. The window is the options' (see [`Options::for_size`]).
    #[must_use]
    pub fn new(options: Options, size_hint: Option<u64>) -> Self {
        let lgwin = options.lgwin.clamp(encode::MIN_LGWIN, encode::MAX_LGWIN);
        Self {
            stream: encode::stream::Stream::new(
                lgwin,
                Level::from_quality(options.quality),
                size_hint.map(hint),
            ),
        }
    }

    /// The next stream's expected size, if known (after [`finish`](Self::finish) or
    /// [`reset`](Self::reset), or before the first write).
    pub fn set_size_hint(&mut self, size_hint: Option<u64>) {
        self.stream.set_size_hint(size_hint.map(hint));
    }

    /// The next stream's options (after [`finish`](Self::finish) or [`reset`](Self::reset),
    /// or before the first write), so that one compressor can take each stream at its own
    /// window, such as [`Options::for_size`] gives it. The stream under way keeps its window.
    pub fn set_options(&mut self, options: Options) {
        self.stream.set_options(
            options.lgwin.clamp(encode::MIN_LGWIN, encode::MAX_LGWIN),
            Level::from_quality(options.quality),
        );
    }

    /// Drops the stream under way, if any, without ending it: ready for a new one, with its
    /// memory kept.
    pub fn reset(&mut self) {
        self.stream.reset();
    }

    /// Takes `data`, appending to `out` whatever it completes.
    pub fn write(&mut self, data: &[u8], out: &mut Vec<u8>) {
        self.stream.write(data, out);
    }

    /// Appends to `out` everything taken so far, so a decoder can produce all of it. Copies
    /// still reach back across a flush. Two flushes in a row add nothing.
    pub fn flush(&mut self, out: &mut Vec<u8>) {
        self.stream.flush(out);
    }

    /// Ends the stream, appending the rest to `out`. The compressor is then ready for a new
    /// stream.
    pub fn finish(&mut self, out: &mut Vec<u8>) {
        self.stream.finish(out);
    }

    /// Bytes held, for tests of memory reuse.
    #[doc(hidden)]
    #[must_use]
    pub fn footprint(&self) -> usize {
        self.stream.footprint()
    }
}

impl std::fmt::Debug for StreamCompressor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamCompressor").finish_non_exhaustive()
    }
}

/// A size hint in memory terms: anything past `usize` is as large as can be.
fn hint(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// Why data couldn't be decompressed.
///
/// More reasons may be added, so a `match` needs a `_` arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The input ended in the middle of the stream.
    Truncated,
    /// The input isn't valid Brotli; the text says which rule it broke.
    Corrupt(&'static str),
    /// The output would exceed the limit given.
    TooLarge,
    /// A complete stream is followed by more bytes.
    TrailingData,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated Brotli data"),
            Self::Corrupt(what) => write!(f, "corrupt Brotli data: {what}"),
            Self::TooLarge => f.write_str("decompressed data exceeds the limit"),
            Self::TrailingData => f.write_str("trailing bytes after the Brotli data"),
        }
    }
}

impl std::error::Error for Error {}

/// Decompresses a Brotli stream, with no limit on the output size.
///
/// A small malicious stream can expand to gigabytes: use [`decompress_with_limit`] for input
/// from outside.
///
/// # Errors
///
/// [`Error::Truncated`] or [`Error::Corrupt`] for invalid input, [`Error::TrailingData`] if
/// bytes follow the stream.
pub fn decompress(src: &[u8]) -> Result<Vec<u8>, Error> {
    decompress_with_limit(src, usize::MAX)
}

/// Decompresses a Brotli stream of at most `limit` bytes of output.
///
/// The limit is checked before each meta-block's data is decoded, so a stream that claims more
/// fails with [`Error::TooLarge`] before the memory is taken. Memory use is the output buffer,
/// which never grows past `limit` bytes, plus about 1 MiB of tables at most for the prefix
/// codes of a meta-block.
///
/// ```
/// let stream = [0x0b, 0x02, 0x80, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x03]; // "hello"
/// assert_eq!(oxbrotli::decompress_with_limit(&stream, 5).unwrap(), b"hello");
/// assert_eq!(
///     oxbrotli::decompress_with_limit(&stream, 4),
///     Err(oxbrotli::Error::TooLarge)
/// );
/// ```
///
/// # Errors
///
/// [`Error::TooLarge`] past the limit, otherwise as [`decompress`].
pub fn decompress_with_limit(src: &[u8], limit: usize) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    Decompressor::new().decompress_into(src, &mut out, limit)?;
    Ok(out)
}

/// A decompressor that keeps its tables between streams, for decoding many.
///
/// ```
/// let stream = [0x0b, 0x02, 0x80, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x03]; // "hello"
/// let mut d = oxbrotli::Decompressor::new();
/// let mut out = Vec::new();
/// d.decompress_into(&stream, &mut out, 1 << 20).unwrap();
/// d.decompress_into(&stream, &mut out, 1 << 20).unwrap();
/// assert_eq!(out, b"hellohello");
/// ```
#[derive(Default)]
pub struct Decompressor {
    decoder: decode::Decoder,
}

impl Decompressor {
    /// A decompressor with no tables yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Decompresses the Brotli stream `src`, appending its output to `out`: at most `limit`
    /// bytes, as [`decompress_with_limit`] does.
    ///
    /// # Errors
    ///
    /// As [`decompress_with_limit`]. On an error, `out` may hold part of the output.
    pub fn decompress_into(
        &mut self,
        src: &[u8],
        out: &mut Vec<u8>,
        limit: usize,
    ) -> Result<(), Error> {
        let used = self.decoder.decode(src, out, limit)?;
        if used != src.len() {
            return Err(Error::TrailingData);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, Options};

    #[test]
    fn windows_are_clamped() {
        assert_eq!(Options::new(4).lgwin(5).lgwin, 10);
        assert_eq!(Options::new(4).lgwin(30).lgwin, 24);
        assert_eq!(Options::new(4).lgwin(19).lgwin, 19);
        assert_eq!(Options::default(), Options::new(4));
        // Set directly past the limit: still a valid stream.
        let mut options = Options::new(4);
        options.lgwin = 99;
        let stream = super::compress_with(b"x", options);
        assert_eq!(super::decompress(&stream).unwrap(), b"x");
    }

    #[test]
    fn errors_say_what_went_wrong() {
        for (e, text) in [
            (Error::Truncated, "truncated Brotli data"),
            (
                Error::Corrupt("simple prefix code repeats a symbol"),
                "corrupt Brotli data: simple prefix code repeats a symbol",
            ),
            (Error::TooLarge, "decompressed data exceeds the limit"),
            (Error::TrailingData, "trailing bytes after the Brotli data"),
        ] {
            assert_eq!(e.to_string(), text, "{e:?}");
        }
    }
}
