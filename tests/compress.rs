//! What oxbrotli compresses, the reference `brotli -d` and our own decoder both decompress.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code: a panic is a test failure"
)]

mod common;

use common::{Rng, corpus, random, unbrotli};

/// Qualities that compress differently: 4, 5 and 6.
const QUALITIES: [u32; 3] = [4, 5, 6];

#[test]
fn the_corpus_round_trips_at_every_quality_and_window() {
    let corpus = corpus();
    std::thread::scope(|s| {
        for quality in QUALITIES {
            for lgwin in 10..=24 {
                let corpus = &corpus;
                s.spawn(move || {
                    for (name, data) in corpus {
                        let options = oxbrotli::Options::new(quality).lgwin(lgwin);
                        let stream = oxbrotli::compress_with(data, options);
                        assert!(
                            oxbrotli::decompress(&stream).unwrap() == *data,
                            "{name} q{quality} w{lgwin}: oxbrotli"
                        );
                        assert!(
                            unbrotli(&stream).unwrap() == *data,
                            "{name} q{quality} w{lgwin}: brotli -d"
                        );
                    }
                });
            }
        }
    });
}

#[test]
fn incompressible_input_is_stored() {
    for (len, seed) in [(1, 1), (100, 2), (65_536, 3), (300_000, 4), (1 << 20, 5)] {
        let data = random(len, seed);
        let stream = oxbrotli::compress(&data, 4);
        assert!(
            stream.len() <= len + len / 16 + 16,
            "{len} random bytes became {}",
            stream.len()
        );
        assert_eq!(unbrotli(&stream).unwrap(), data, "{len} bytes");
    }
}

#[test]
fn the_empty_input_is_the_shortest_stream() {
    let stream = oxbrotli::compress(b"", 4);
    assert_eq!(stream.len(), 1);
    assert_eq!(unbrotli(&stream).unwrap(), b"");
    assert_eq!(oxbrotli::decompress(&stream).unwrap(), b"");
}

/// Literal counts that grow like Fibonacci numbers: a Huffman code for 17 of them is 16 deep,
/// one past the limit, and for 22 of them 21 deep. The length limit is what makes these
/// streams valid.
#[test]
fn skewed_literals_need_the_length_limit() {
    for symbols in [17, 22] {
        let (mut a, mut b) = (1usize, 1usize);
        let mut data = Vec::new();
        for symbol in 0..symbols {
            data.extend(std::iter::repeat_n(b'A' + symbol, a));
            (a, b) = (b, a + b);
        }
        let mut rng = Rng::new(5);
        for i in (1..data.len()).rev() {
            data.swap(i, rng.below(i as u64 + 1) as usize);
        }
        assert!(data.len() < 1 << 16, "one meta-block: {}", data.len());
        let stream = oxbrotli::compress(&data, 4);
        assert!(stream.len() < data.len() / 2, "{} bytes", stream.len());
        assert_eq!(unbrotli(&stream).unwrap(), data, "{symbols} symbols");
        assert_eq!(oxbrotli::decompress(&stream).unwrap(), data);
    }
}

#[test]
fn every_quality_gives_a_valid_stream() {
    let data = common::text(20_000, 3);
    for quality in [0, 1, 4, 5, 6, 9, 11, 12, u32::MAX] {
        let stream = oxbrotli::compress(&data, quality);
        assert_eq!(unbrotli(&stream).unwrap(), data, "quality {quality}");
    }
}

/// FNV-1a: a fingerprint of a stream.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[test]
fn quality_4_output_is_pinned() {
    // Qualities 5 and up change the encoder around quality 4's, which must write what it
    // always has (checked byte for byte against the encoder before B14 on the corpus and
    // `bench-data/`): fingerprints of its streams for text, markup and UTF-8.
    // Name, input, window, and the stream's length and fingerprint.
    let cases: [(&str, Vec<u8>, u32, Pin); 3] = [
        ("text", common::text(20_000, 1), 22, PIN_TEXT),
        ("html", common::html(5_000, 2), 16, PIN_HTML),
        ("utf8", common::utf8_text(30_000, 3), 10, PIN_UTF8),
    ];
    for (name, data, lgwin, pin) in cases {
        let out = oxbrotli::compress_with(&data, oxbrotli::Options::new(4).lgwin(lgwin));
        assert_eq!((out.len(), fnv(&out)), pin, "{name}");
    }
}

/// A stream's length and fingerprint.
type Pin = (usize, u64);

const PIN_TEXT: Pin = (6445, 8_494_504_788_070_808_472);
const PIN_HTML: Pin = (1272, 16_233_511_495_893_429_143);
const PIN_UTF8: Pin = (6467, 9_297_582_356_251_961_978);
