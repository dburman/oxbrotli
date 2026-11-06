//! Interoperability with the reference `brotli` program (an outside program: nothing from it
//! is linked). Everything `brotli` writes must decompress here.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code: a panic is a test failure"
)]

mod common;

use common::{brotli_qw, corpus, small_corpus};

/// The reference's stream for `data` at quality `q` and window `w` decodes to `data`.
fn check(name: &str, data: &[u8], q: u32, w: u32) {
    let packed = brotli_qw(data, q, w);
    match oxbrotli::decompress(&packed) {
        Ok(out) => assert!(out == data, "{name} at q{q} w{w}: wrong output"),
        Err(e) => panic!("{name} at q{q} w{w}: {e}"),
    }
}

/// Every quality and every window on the corpus entries up to 512 KiB. Each quality runs on
/// its own thread: the reference's high qualities are slow.
#[test]
fn every_quality_and_window_of_the_reference_decodes() {
    let corpus = small_corpus();
    std::thread::scope(|scope| {
        for q in 0..=11 {
            let corpus = &corpus;
            scope.spawn(move || {
                for w in 10..=24 {
                    for (name, data) in corpus {
                        check(name, data, q, w);
                    }
                }
            });
        }
    });
}

/// The largest entry, bigger than the reference's default window, at a few settings.
#[test]
fn the_largest_entry_decodes() {
    let (name, data) = corpus().into_iter().max_by_key(|(_, d)| d.len()).unwrap();
    std::thread::scope(|scope| {
        for (q, w) in [(1, 24), (3, 16), (5, 22), (9, 24), (11, 24)] {
            let (name, data) = (&name, &data);
            scope.spawn(move || check(name, data, q, w));
        }
    });
}

/// The reference's default settings (quality 11, window 22), as `brotli` with no options.
#[test]
fn the_reference_defaults_decode() {
    for (name, data) in small_corpus() {
        let packed = common::brotli::<&str>(&[], &data);
        assert!(oxbrotli::decompress(&packed).unwrap() == data, "{name}");
    }
}
