//! The corpus and the reference runner work, so the other tests can rely on them.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code: a panic is a test failure"
)]

mod common;

use common::{brotli_qw, corpus, unbrotli};

#[test]
fn the_corpus_is_the_same_every_time() {
    let a = corpus();
    let b = corpus();
    assert_eq!(a.len(), b.len());
    for ((name, x), (_, y)) in a.iter().zip(&b) {
        assert!(x == y, "{name} differs between calls");
    }
}

#[test]
fn only_the_empty_entry_is_empty() {
    for (name, data) in corpus() {
        assert_eq!(
            data.is_empty(),
            name == "empty",
            "{name}: {} bytes",
            data.len()
        );
    }
}

/// Window sizes up to the reference's default (2^22, about 4 MiB) are exercised in full by
/// one entry bigger than that; the smaller windows by every entry bigger than them.
#[test]
fn one_entry_is_bigger_than_the_default_window() {
    let largest = corpus().iter().map(|(_, d)| d.len()).max().unwrap();
    assert!(largest > (1 << 22) - 16, "largest entry {largest} bytes");
}

#[test]
fn the_reference_round_trips() {
    let (_, data) = corpus()
        .into_iter()
        .find(|(n, _)| n == "html-500k")
        .unwrap();
    let packed = brotli_qw(&data, 4, 19);
    assert!(packed.len() < data.len() / 2, "{} bytes", packed.len());
    assert!(
        unbrotli(&packed).unwrap() == data,
        "wrong output (not printed: 500 KB)"
    );
}

#[test]
fn the_reference_rejects_garbage() {
    let err = unbrotli(b"\xff\xff\xff\xff not brotli").unwrap_err();
    assert!(!err.is_empty(), "brotli -d gave no message");
}
