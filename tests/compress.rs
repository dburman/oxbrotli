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
