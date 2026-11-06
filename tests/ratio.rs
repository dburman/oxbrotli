//! Our size against `brotli -q 4`'s, per corpus entry and window. Run by `scripts/gate --ratio`
//! (or `cargo test --release --test ratio -- --ignored --nocapture`): it prints the table, and
//! fails if any entry comes out more than [`LIMIT`] larger than the reference's.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code: a panic is a test failure"
)]

mod common;

use common::{brotli_file, corpus};

/// How much larger than the reference's an output may be: 0.5%.
const LIMIT: f64 = 1.005;

/// A corpus entry's name and length, and per window our size and the reference's.
type Row = (String, usize, Vec<(usize, usize)>);

#[test]
#[ignore = "slow; run by scripts/gate --ratio"]
fn quality_4_is_within_the_limit_of_the_reference() {
    let corpus = corpus();
    let windows = [10, 15, 19];
    let rows: Vec<Row> = std::thread::scope(|s| {
        let handles: Vec<_> = corpus
            .iter()
            // Real responses are checked as served, below.
            .filter(|(name, data)| !data.is_empty() && !name.starts_with("bench-data/"))
            .map(|(name, data)| {
                s.spawn(move || {
                    let sizes = windows
                        .iter()
                        .map(|&w| {
                            let ours =
                                oxbrotli::compress_with(data, oxbrotli::Options::new(4).lgwin(w));
                            assert!(oxbrotli::decompress(&ours).unwrap() == *data, "{name}");
                            (ours.len(), brotli_file(data, 4, w).len())
                        })
                        .collect();
                    (name.clone(), data.len(), sizes)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    println!(
        "{:<24} {:>10} {:>24} {:>24} {:>24}",
        "entry", "bytes", "w10 ours/ref", "w15 ours/ref", "w19 ours/ref"
    );
    let mut worst = (0.0f64, String::new());
    for (name, len, sizes) in &rows {
        let cells: Vec<String> = sizes
            .iter()
            .zip(windows)
            .map(|(&(ours, theirs), w)| {
                let ratio = ours as f64 / theirs as f64;
                if ratio > worst.0 {
                    worst = (ratio, format!("{name} w{w}"));
                }
                format!("{ours}/{theirs} {:+.2}%", (ratio - 1.0) * 100.0)
            })
            .collect();
        println!(
            "{name:<24} {len:>10} {:>24} {:>24} {:>24}",
            cells[0], cells[1], cells[2]
        );
    }
    let (ours, theirs) = rows
        .iter()
        .flat_map(|r| &r.2)
        .fold((0, 0), |(a, b), &(x, y)| (a + x, b + y));
    println!(
        "total: {ours}/{theirs} {:+.2}%; worst {} {:+.2}%",
        (ours as f64 / theirs as f64 - 1.0) * 100.0,
        worst.1,
        (worst.0 - 1.0) * 100.0
    );
    assert!(
        worst.0 <= LIMIT,
        "{} is {:+.2}% larger",
        worst.1,
        (worst.0 - 1.0) * 100.0
    );
}

/// How much larger than the reference's a web-sized body may be, for now: our dictionary
/// lookup keeps a different word in some slots than the reference's tuned table does, which
/// shows most on small bodies (see the B9 notes in `docs/plan/README.md`). To be tightened to
/// [`LIMIT`] once we choose those words as well.
const SMALL_LIMIT: f64 = 1.02;

#[test]
#[ignore = "slow; run by scripts/gate --ratio"]
fn web_sized_bodies_are_within_the_limit_of_the_reference() {
    let mut worst = (0.0f64, String::new());
    let (mut ours_total, mut theirs_total) = (0, 0);
    println!(
        "{:<12} {:>4} {:>8} {:>8} {:>8}",
        "body", "w", "ours", "ref", "size"
    );
    for len in [1 << 10, 4 << 10, 16 << 10, 64 << 10] {
        for (kind, make) in [
            ("html", common::html as fn(usize, u64) -> Vec<u8>),
            ("css", common::css),
            ("js", common::js),
            ("json", common::json),
        ] {
            let data = make(len, 31);
            let options = oxbrotli::Options::for_size(4, Some(len as u64), 22);
            let ours = oxbrotli::compress_with(&data, options);
            assert!(oxbrotli::decompress(&ours).unwrap() == data, "{kind} {len}");
            let theirs = brotli_file(&data, 4, options.lgwin).len();
            let ratio = ours.len() as f64 / theirs as f64;
            let name = format!("{kind}-{}k", len >> 10);
            if ratio > worst.0 {
                worst = (ratio, name.clone());
            }
            ours_total += ours.len();
            theirs_total += theirs;
            println!(
                "{name:<12} {:>4} {:>8} {theirs:>8} {:>+7.2}%",
                options.lgwin,
                ours.len(),
                (ratio - 1.0) * 100.0
            );
        }
    }
    println!(
        "total: {ours_total}/{theirs_total} {:+.2}%; worst {} {:+.2}%",
        (ours_total as f64 / theirs_total as f64 - 1.0) * 100.0,
        worst.1,
        (worst.0 - 1.0) * 100.0
    );
    assert!(
        worst.0 <= SMALL_LIMIT,
        "{} is {:+.2}% larger",
        worst.1,
        (worst.0 - 1.0) * 100.0
    );
}

/// How much larger than the reference's a real response may be, compressed as served.
const REAL_LIMIT: f64 = 1.01;

/// Files in `bench-data/` (real responses, kept locally), each at the window
/// `Options::for_size` picks for it, as a server compresses them. They need more room than the
/// generated corpus: small real bodies lean on the dictionary, where the reference's tuned
/// choice of words gains up to 0.7% (see the B9 notes in `docs/plan/README.md`).
#[test]
#[ignore = "slow; run by scripts/gate --ratio"]
fn real_responses_as_served_are_within_the_limit_of_the_reference() {
    let real: Vec<(String, Vec<u8>)> = corpus()
        .into_iter()
        .filter(|(name, data)| name.starts_with("bench-data/") && !data.is_empty())
        .collect();
    if real.is_empty() {
        println!("no files in bench-data/");
        return;
    }
    let mut worst = (0.0f64, String::new());
    let (mut ours_total, mut theirs_total) = (0, 0);
    for (name, data) in &real {
        let options = oxbrotli::Options::for_size(4, Some(data.len() as u64), 22);
        let ours = oxbrotli::compress_with(data, options);
        assert!(oxbrotli::decompress(&ours).unwrap() == *data, "{name}");
        let theirs = brotli_file(data, 4, options.lgwin).len();
        let ratio = ours.len() as f64 / theirs as f64;
        if ratio > worst.0 {
            worst = (ratio, name.clone());
        }
        ours_total += ours.len();
        theirs_total += theirs;
        println!(
            "{name:<48} {:>9} w{:<2} {:>8} {theirs:>8} {:>+7.2}%",
            data.len(),
            options.lgwin,
            ours.len(),
            (ratio - 1.0) * 100.0
        );
    }
    println!(
        "total: {ours_total}/{theirs_total} {:+.2}%; worst {} {:+.2}%",
        (ours_total as f64 / theirs_total as f64 - 1.0) * 100.0,
        worst.1,
        (worst.0 - 1.0) * 100.0
    );
    assert!(
        worst.0 <= REAL_LIMIT,
        "{} is {:+.2}% larger",
        worst.1,
        (worst.0 - 1.0) * 100.0
    );
}
