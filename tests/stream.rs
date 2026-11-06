//! The stream compressor: whatever the chunks and flushes, the output decodes to the input,
//! by our decoder and the reference `brotli -d`; after each flush, the output so far decodes
//! to everything written; reuse keeps memory and changes nothing.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code: a panic is a test failure"
)]

mod common;

use common::{Rng, html, small_corpus, text, unbrotli};
use oxbrotli::{Options, StreamCompressor};

/// The bytes that end a stream on a byte boundary: an empty last meta-block (`ISLAST` 1,
/// `ISLASTEMPTY` 1).
const END: u8 = 0x03;

fn stream_in_chunks(data: &[u8], options: Options, chunk: usize) -> Vec<u8> {
    let mut c = StreamCompressor::new(options, None);
    let mut out = Vec::new();
    for piece in data.chunks(chunk) {
        c.write(piece, &mut out);
    }
    c.finish(&mut out);
    out
}

#[test]
fn any_chunks_decode_to_the_input() {
    let corpus = small_corpus();
    std::thread::scope(|s| {
        for chunk in [1, 7, 4096, 65_536] {
            let corpus = &corpus;
            s.spawn(move || {
                for (name, data) in corpus {
                    // Byte by byte, only the shorter entries: it is the same code, slower.
                    if chunk == 1 && data.len() > 200_000 {
                        continue;
                    }
                    for lgwin in [10, 16, 22] {
                        let out = stream_in_chunks(data, Options::new(4).lgwin(lgwin), chunk);
                        let what = format!("{name} w{lgwin} in chunks of {chunk}");
                        assert!(oxbrotli::decompress(&out).unwrap() == *data, "{what}");
                        assert!(unbrotli(&out).unwrap() == *data, "{what}: brotli -d");
                    }
                }
            });
        }
    });
}

#[test]
fn without_flushes_a_stream_is_the_one_shot_stream() {
    for (name, data) in small_corpus() {
        for lgwin in [10, 19] {
            let options = Options::new(4).lgwin(lgwin);
            let mut c = StreamCompressor::new(options, Some(data.len() as u64));
            let mut out = Vec::new();
            for piece in data.chunks(10_000) {
                c.write(piece, &mut out);
            }
            c.finish(&mut out);
            assert!(
                out == oxbrotli::compress_with(&data, options),
                "{name} w{lgwin}"
            );
        }
    }
}

/// Writes `data` in random pieces with a flush after each, checking that the output so far,
/// ended with an empty last meta-block, decodes to what was written; by the reference too,
/// every `reference_every` flushes.
fn check_flushes(data: &[u8], options: Options, seed: u64, reference_every: usize) {
    let mut rng = Rng::new(seed);
    let mut c = StreamCompressor::new(options, None);
    let mut out = Vec::new();
    let mut written = 0;
    let mut flushes = 0;
    while written < data.len() {
        let n = (1 + rng.below(40_000) as usize).min(data.len() - written);
        c.write(&data[written..written + n], &mut out);
        written += n;
        c.flush(&mut out);
        flushes += 1;
        let mut prefix = out.clone();
        prefix.push(END);
        let what = format!("{} bytes, w{}, flush {flushes}", written, options.lgwin);
        assert!(
            oxbrotli::decompress(&prefix).unwrap() == data[..written],
            "{what}"
        );
        if flushes % reference_every == 0 {
            assert!(
                unbrotli(&prefix).unwrap() == data[..written],
                "{what}: brotli -d"
            );
        }
        // A second flush adds nothing.
        let len = out.len();
        c.flush(&mut out);
        assert_eq!(out.len(), len, "{what}: a second flush");
    }
    c.finish(&mut out);
    assert!(oxbrotli::decompress(&out).unwrap() == data, "finished");
    assert!(unbrotli(&out).unwrap() == data, "finished: brotli -d");
}

#[test]
fn after_each_flush_the_output_so_far_decodes() {
    let data = html(600_000, 11);
    for (lgwin, seed) in [(10, 1), (16, 2), (22, 3)] {
        check_flushes(&data, Options::new(4).lgwin(lgwin), seed, 5);
    }
    // Noise, stored uncompressed and so already on byte boundaries.
    check_flushes(&common::random(200_000, 4), Options::new(4), 5, 3);
}

#[test]
fn matches_reach_back_across_a_flush() {
    // The second half repeats the first: after a flush it is still a copy, not new literals.
    let half = text(20_000, 6);
    let mut c = StreamCompressor::new(Options::new(4), None);
    let mut out = Vec::new();
    c.write(&half, &mut out);
    c.flush(&mut out);
    let first = out.len();
    c.write(&half, &mut out);
    c.finish(&mut out);
    assert!(
        out.len() - first < 100,
        "{} bytes for the repeat",
        out.len() - first
    );
    assert_eq!(
        oxbrotli::decompress(&out).unwrap(),
        [&half[..], &half].concat()
    );
}

#[test]
fn finishing_without_writing_is_an_empty_stream() {
    for lgwin in 10..=24 {
        let options = Options::new(4).lgwin(lgwin);
        let mut c = StreamCompressor::new(options, None);
        let mut out = Vec::new();
        c.finish(&mut out);
        assert_eq!(out, oxbrotli::compress_with(b"", options), "w{lgwin}");
        assert_eq!(unbrotli(&out).unwrap(), b"", "w{lgwin}: brotli -d");
        // Flushed first: still empty.
        let mut c = StreamCompressor::new(options, None);
        let mut out = Vec::new();
        c.flush(&mut out);
        c.finish(&mut out);
        assert_eq!(
            oxbrotli::decompress(&out).unwrap(),
            b"",
            "w{lgwin}, flushed"
        );
        assert_eq!(unbrotli(&out).unwrap(), b"", "w{lgwin}, flushed: brotli -d");
    }
}

/// One stream through `c`: written in pieces, flushed once in the middle.
fn one_stream(c: &mut StreamCompressor, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let (a, b) = data.split_at(data.len() / 2);
    c.write(a, &mut out);
    c.flush(&mut out);
    c.write(b, &mut out);
    c.finish(&mut out);
    out
}

#[test]
fn a_reused_compressor_writes_what_a_fresh_one_does() {
    let options = Options::new(4).lgwin(18);
    let first = html(300_000, 7);
    let second = text(150_000, 8);
    let mut fresh = StreamCompressor::new(options, None);
    let expected = one_stream(&mut fresh, &second);
    // After a finished stream.
    let mut c = StreamCompressor::new(options, None);
    one_stream(&mut c, &first);
    assert!(one_stream(&mut c, &second) == expected, "after finish");
    // After a stream dropped half way, with output pending.
    let mut out = Vec::new();
    c.write(&first[..100_000], &mut out);
    c.flush(&mut out);
    c.write(&first[100_000..100_100], &mut out);
    c.reset();
    assert!(one_stream(&mut c, &second) == expected, "after reset");
    // A size hint for the next stream is the same as one given to a fresh compressor.
    c.set_size_hint(Some(second.len() as u64));
    let mut hinted = StreamCompressor::new(options, Some(second.len() as u64));
    assert!(
        one_stream(&mut c, &second) == one_stream(&mut hinted, &second),
        "hinted"
    );
}

#[test]
fn a_reused_compressor_takes_each_stream_at_its_own_window() {
    let small = text(3000, 9);
    let large = html(200_000, 10);
    let fresh =
        |options: Options, data: &[u8]| one_stream(&mut StreamCompressor::new(options, None), data);
    let (w12, w18) = (Options::new(4).lgwin(12), Options::new(4).lgwin(18));
    let mut c = StreamCompressor::new(w18, None);
    one_stream(&mut c, &large);
    // Down to a small window, then back up: each as a fresh compressor writes it.
    c.set_options(w12);
    assert!(
        one_stream(&mut c, &small) == fresh(w12, &small),
        "w18 to w12"
    );
    assert!(
        one_stream(&mut c, &large) == fresh(w12, &large),
        "kept at w12"
    );
    c.set_options(w18);
    let out = one_stream(&mut c, &large);
    assert!(out == fresh(w18, &large), "w12 to w18");
    assert_eq!(out[0] & 0x0f, 0x03, "the w18 header (0011)");
    // Set during a stream, it waits for the next one.
    let mut out = Vec::new();
    c.write(&large[..100_000], &mut out);
    c.set_options(w12);
    c.write(&large[100_000..], &mut out);
    c.finish(&mut out);
    let mut expected = Vec::new();
    let mut f = StreamCompressor::new(w18, None);
    f.write(&large[..100_000], &mut expected);
    f.write(&large[100_000..], &mut expected);
    f.finish(&mut expected);
    assert!(out == expected, "the stream under way keeps w18");
    assert!(one_stream(&mut c, &large) == fresh(w12, &large), "then w12");
    // Out of range, as for new: clamped.
    c.set_options(Options::new(4).lgwin(30));
    assert!(
        one_stream(&mut c, &small) == fresh(Options::new(4).lgwin(24), &small),
        "clamped to 24"
    );
}

#[test]
fn a_thousand_small_streams_take_no_more_memory_than_one() {
    let options = Options::for_size(4, Some(1024), 19);
    let mut c = StreamCompressor::new(options, Some(1024));
    let mut after_first = 0;
    for i in 0..1000u64 {
        let data = text(1024, i);
        let out = one_stream(&mut c, &data);
        if i % 97 == 0 {
            assert!(oxbrotli::decompress(&out).unwrap() == data, "stream {i}");
        }
        if i == 0 {
            after_first = c.footprint();
        }
        assert_eq!(c.footprint(), after_first, "stream {i}");
    }
    // A table fit to 1 KiB (64 KiB of it), and the input it held.
    assert!(after_first < 96 << 10, "{after_first} bytes");
}

#[test]
fn any_size_hint_is_taken() {
    // A server passes on whatever length a response declares, up to u64::MAX.
    let data = text(5000, 11);
    for hint in [u64::MAX, 1 << 63, (1 << 63) + 1, u64::from(u32::MAX) + 1, 0] {
        let options = Options::for_size(4, Some(hint), 19);
        let mut c = StreamCompressor::new(options, Some(hint));
        let out = one_stream(&mut c, &data);
        assert!(oxbrotli::decompress(&out).unwrap() == data, "new, {hint}");
        c.set_size_hint(Some(hint));
        let out = one_stream(&mut c, &data);
        assert!(
            oxbrotli::decompress(&out).unwrap() == data,
            "reused, {hint}"
        );
    }
}

#[test]
fn windows_for_sizes() {
    for (size, lgwin) in [
        (0, 10),
        (1008, 10),
        (1009, 11),
        ((1 << 16) - 16, 16),
        ((1 << 16) - 15, 17),
        (1 << 30, 19),
    ] {
        assert_eq!(Options::for_size(4, Some(size), 19).lgwin, lgwin, "{size}");
    }
    assert_eq!(Options::for_size(4, None, 19).lgwin, 19);
    assert_eq!(Options::for_size(4, None, 30).lgwin, 24);
    assert_eq!(Options::for_size(4, Some(5), 3).lgwin, 10);
    assert_eq!(Options::for_size(4, Some(u64::MAX), 24).lgwin, 24);
    // The window chosen holds the body: every copy can reach the start.
    let data = text(3000, 9);
    let options = Options::for_size(4, Some(3000), 19);
    assert_eq!(options.lgwin, 12);
    let mut c = StreamCompressor::new(options, Some(3000));
    assert!(
        oxbrotli::decompress(&one_stream(&mut c, &data)).unwrap() == data,
        "window 12"
    );
}
