//! Damaged input never makes the decoder panic, loop or take memory past its limit: it returns
//! an error, or (for damage the format can't see) some output. Brotli has no checksum, so a
//! flip that still decodes to the right length can't be detected; that's the format.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test code: a panic is a test failure"
)]

mod common;

use common::{Rng, brotli_qw, css, html, js, text};

/// Reference streams with every kind of meta-block and code: small and large inputs, fast and
/// slow qualities (block switches, context maps and dictionary words come with the slow
/// ones), small and large windows.
fn streams() -> Vec<(String, Vec<u8>)> {
    let mut zeros_then_text = vec![0; 5_000];
    zeros_then_text.extend(text(5_000, 14));
    let mut rng = Rng::new(16);
    let skewed: Vec<u8> = (0..50_000)
        .map(|_| b"eeeeeeettttaaoinshrdlu"[rng.below(22) as usize])
        .collect();
    let sources = [
        ("text-3k", text(3_000, 11)),
        ("html-40k", html(40_000, 12)),
        ("css-20k", css(20_000, 13)),
        ("js-30k", js(30_000, 15)),
        ("zeros-then-text", zeros_then_text),
        ("skewed-50k", skewed),
    ];
    let mut out = Vec::new();
    for (name, data) in &sources {
        for q in [0, 1, 4, 11] {
            for w in [10, 22] {
                out.push((format!("{name} q{q} w{w}"), brotli_qw(data, q, w)));
            }
        }
    }
    out
}

/// The output limit for damaged streams: generous for the corpus, small enough that damage
/// claiming a huge size stays cheap.
const LIMIT: usize = 1 << 20;

/// Decompresses `stream` fresh and with a decompressor every damaged stream goes through:
/// they have to agree, or the reused one kept something from an earlier stream. Returns the
/// fresh result.
fn try_decompress(stream: &[u8]) -> Result<Vec<u8>, oxbrotli::Error> {
    thread_local! {
        static REUSED: std::cell::RefCell<oxbrotli::Decompressor> =
            std::cell::RefCell::new(oxbrotli::Decompressor::new());
    }
    let fresh = oxbrotli::decompress_with_limit(stream, LIMIT);
    let mut out = Vec::new();
    let reused = REUSED.with(|d| d.borrow_mut().decompress_into(stream, &mut out, LIMIT));
    assert!(out.capacity() <= LIMIT, "{} bytes taken", out.capacity());
    match (&fresh, reused) {
        (Ok(a), Ok(())) => assert!(*a == out, "reused decompressor gave other output"),
        (Err(a), Err(b)) => assert_eq!(*a, b),
        (a, b) => panic!("fresh {:?}, reused {:?}", a.as_ref().map(Vec::len), b),
    }
    fresh
}

#[test]
fn every_stream_decodes_undamaged() {
    for (name, stream) in streams() {
        assert!(try_decompress(&stream).is_ok(), "{name}");
    }
}

#[test]
fn truncated_streams_are_truncation() {
    let mut rng = Rng::new(31);
    for (name, stream) in streams() {
        // Every length for short streams; for longer ones, every length near both ends and
        // a sample in between.
        let lengths: Vec<usize> = if stream.len() <= 2048 {
            (0..stream.len()).collect()
        } else {
            (0..512)
                .chain(stream.len() - 512..stream.len())
                .chain((0..300).map(|_| rng.below(stream.len() as u64) as usize))
                .collect()
        };
        for len in lengths {
            assert_eq!(
                try_decompress(&stream[..len]),
                Err(oxbrotli::Error::Truncated),
                "{name} cut to {len} of {} bytes",
                stream.len()
            );
        }
    }
}

#[test]
fn every_bit_flip_in_the_headers_is_survived() {
    // The first 512 bytes hold the stream header, the first meta-block header and its codes:
    // every single bit flipped there.
    for (_, stream) in streams().into_iter().step_by(3) {
        for at in 0..stream.len().min(512) {
            for bit in 0..8 {
                let mut bad = stream.clone();
                bad[at] ^= 1 << bit;
                drop(try_decompress(&bad));
            }
        }
    }
}

#[test]
fn random_flips_are_survived() {
    let mut rng = Rng::new(77);
    for (_, stream) in streams() {
        for _ in 0..400 {
            let mut bad = stream.clone();
            for _ in 0..=rng.below(3) {
                let at = rng.below(bad.len() as u64) as usize;
                bad[at] ^= 1 << rng.below(8);
            }
            drop(try_decompress(&bad));
        }
    }
}

#[test]
fn overwritten_runs_are_survived() {
    let mut rng = Rng::new(78);
    for (_, stream) in streams() {
        for _ in 0..200 {
            let mut bad = stream.clone();
            let at = rng.below(bad.len() as u64) as usize;
            let len = (1 + rng.below(16) as usize).min(bad.len() - at);
            for b in &mut bad[at..at + len] {
                *b = rng.next() as u8;
            }
            drop(try_decompress(&bad));
        }
    }
}

#[test]
fn random_bytes_are_survived() {
    let mut rng = Rng::new(79);
    for _ in 0..100_000 {
        let len = rng.below(513) as usize;
        let junk: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        drop(try_decompress(&junk));
    }
}

#[test]
fn a_claimed_size_past_the_limit_takes_no_memory() {
    // A meta-block claiming 16 MiB, then nothing: bit 0 WBITS 16, bit 1 ISLAST 0, bits 2-3
    // MNIBBLES 6, bits 4-27 MLEN - 1 = 0xffffff. TooLarge, before reading or reserving more.
    let bomb = [0xf8, 0xff, 0xff, 0x0f];
    let mut out = Vec::new();
    assert_eq!(
        oxbrotli::Decompressor::new().decompress_into(&bomb, &mut out, LIMIT),
        Err(oxbrotli::Error::TooLarge)
    );
    assert_eq!(out.capacity(), 0);
}

#[test]
fn a_real_bomb_stops_at_the_limit() {
    // 64 MiB of zeros compresses to a few hundred bytes; with a 1 MiB limit the decoder
    // stops with TooLarge and its buffer stays within the limit.
    let bomb = brotli_qw(&vec![0; 64 << 20], 5, 24);
    assert!(bomb.len() < 4096, "{} bytes", bomb.len());
    let mut out = Vec::new();
    assert_eq!(
        oxbrotli::Decompressor::new().decompress_into(&bomb, &mut out, LIMIT),
        Err(oxbrotli::Error::TooLarge)
    );
    assert!(out.capacity() <= LIMIT, "{} bytes taken", out.capacity());
    assert_eq!(oxbrotli::decompress(&bomb).unwrap().len(), 64 << 20);
}

#[test]
fn trailing_bytes_are_reported() {
    let (_, stream) = streams().swap_remove(0);
    let mut longer = stream.clone();
    longer.push(0);
    assert_eq!(
        oxbrotli::decompress(&longer),
        Err(oxbrotli::Error::TrailingData)
    );
}
