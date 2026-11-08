//! Our size against the reference `brotli`'s, per quality, corpus entry and window. Run by `scripts/gate --ratio`
//! (or `cargo test --release --test ratio -- --ignored --nocapture`): it prints the table, and
//! fails if any entry comes out more than [`LIMIT`] larger than the reference's.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code: a panic is a test failure"
)]

mod common;

use std::fmt::Write as _;

use common::{brotli_file, corpus};

/// How much larger than the reference's an output may be: 0.5%.
const LIMIT: f64 = 1.005;

/// A corpus entry's name and length, and per window our size and the reference's.
type Row = (String, usize, Vec<(usize, usize)>);

/// How much larger than the reference's a web-sized body at quality 5 or 6 may be: 1% (they
/// come out within 0.5%; quality 4's [`SMALL_LIMIT`] is wider for its word choice).
const CHAIN_SMALL_LIMIT: f64 = 1.01;
/// Bytes an output of quality 5 or 6 under 1 KiB may be over the limit: in 100 bytes one dictionary word is 4%,
/// and from quality 5 the reference's tuned choice of short words decides (see B12 in
/// `docs/plan/README.md`).
const SMALL_SLACK: usize = 4;

#[test]
#[ignore = "slow; run by scripts/gate --ratio"]
fn quality_4_is_within_the_limit_of_the_reference() {
    let (report, over) = corpus_within(4, &[10, 15, 19], LIMIT, 0);
    println!("{report}");
    assert!(over.is_empty(), "over the limit: {over:?}");
}

#[test]
#[ignore = "slow; run by scripts/gate --ratio"]
fn quality_5_is_within_the_limit_of_the_reference() {
    let (report, over) = corpus_within(5, &[10, 15, 19], LIMIT, SMALL_SLACK);
    println!("{report}");
    assert!(over.is_empty(), "over the limit: {over:?}");
}

#[test]
#[ignore = "slow; run by scripts/gate --ratio"]
fn quality_6_is_within_the_limit_of_the_reference() {
    let (report, over) = corpus_within(6, &[10, 15, 19], LIMIT, SMALL_SLACK);
    println!("{report}");
    assert!(over.is_empty(), "over the limit: {over:?}");
}

/// The generated corpus's sizes at `quality` and each of `windows`, ours against the
/// reference's, as a table; and the entries more than `limit` times the reference's (plus
/// `slack` bytes, for outputs under 1 KiB).
fn corpus_within(quality: u32, windows: &[u32], limit: f64, slack: usize) -> (String, Vec<String>) {
    let corpus = corpus();
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
                            let ours = oxbrotli::compress_with(
                                data,
                                oxbrotli::Options::new(quality).lgwin(w),
                            );
                            assert!(oxbrotli::decompress(&ours).unwrap() == *data, "{name}");
                            (ours.len(), brotli_file(data, quality, w).len())
                        })
                        .collect();
                    (name.clone(), data.len(), sizes)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut report = format!("q{quality} {:<21} {:>10}", "entry", "bytes");
    for w in windows {
        write!(report, " {:>24}", format!("w{w} ours/ref")).unwrap();
    }
    let mut worst = (0.0f64, String::new());
    let mut over = Vec::new();
    for (name, len, sizes) in &rows {
        write!(report, "\n{name:<24} {len:>10}").unwrap();
        for (&(ours, theirs), w) in sizes.iter().zip(windows) {
            let ratio = ours as f64 / theirs as f64;
            if ratio > worst.0 {
                worst = (ratio, format!("{name} w{w}"));
            }
            let allowed = if theirs < 1024 { slack } else { 0 };
            if ours as f64 > theirs as f64 * limit + allowed as f64 {
                over.push(format!("{name} w{w} {:+.2}%", (ratio - 1.0) * 100.0));
            }
            let cell = format!("{ours}/{theirs} {:+.2}%", (ratio - 1.0) * 100.0);
            write!(report, " {cell:>24}").unwrap();
        }
    }
    let (ours, theirs) = rows
        .iter()
        .flat_map(|r| &r.2)
        .fold((0, 0), |(a, b), &(x, y)| (a + x, b + y));
    write!(
        report,
        "\ntotal: {ours}/{theirs} {:+.2}%; worst {} {:+.2}%",
        (ours as f64 / theirs as f64 - 1.0) * 100.0,
        worst.1,
        (worst.0 - 1.0) * 100.0
    )
    .unwrap();
    (report, over)
}

/// How much larger than the reference's a web-sized body may be, for now: our dictionary
/// lookup keeps a different word in some slots than the reference's tuned table does, which
/// shows most on small bodies (see the B9 notes in `docs/plan/README.md`). To be tightened to
/// [`LIMIT`] once we choose those words as well.
const SMALL_LIMIT: f64 = 1.02;

#[test]
#[ignore = "slow; run by scripts/gate --ratio"]
fn web_sized_bodies_are_within_the_limit_of_the_reference() {
    let (report, worst) = web_sized(4);
    println!("{report}");
    assert!(
        worst.0 <= SMALL_LIMIT,
        "{} is {:+.2}% larger",
        worst.1,
        (worst.0 - 1.0) * 100.0
    );
}

#[test]
#[ignore = "slow; run by scripts/gate --ratio"]
fn web_sized_bodies_at_quality_5_and_6_are_within_the_limit_of_the_reference() {
    for quality in [5, 6] {
        let (report, worst) = web_sized(quality);
        println!("{report}");
        assert!(
            worst.0 <= CHAIN_SMALL_LIMIT,
            "quality {quality}: {} is {:+.2}% larger",
            worst.1,
            (worst.0 - 1.0) * 100.0
        );
    }
}

/// Generated HTML, CSS, JS and JSON bodies of 1 to 64 KiB at `quality`, each at the window
/// `Options::for_size` picks: the table, and the worst ratio to the reference's and its body.
fn web_sized(quality: u32) -> (String, (f64, String)) {
    let mut worst = (0.0f64, String::new());
    let (mut ours_total, mut theirs_total) = (0, 0);
    let mut report = format!(
        "q{quality} {:<9} {:>4} {:>8} {:>8} {:>8}",
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
            let options = oxbrotli::Options::for_size(quality, Some(len as u64), 22);
            let ours = oxbrotli::compress_with(&data, options);
            assert!(oxbrotli::decompress(&ours).unwrap() == data, "{kind} {len}");
            let theirs = brotli_file(&data, quality, options.lgwin).len();
            let ratio = ours.len() as f64 / theirs as f64;
            let name = format!("{kind}-{}k", len >> 10);
            if ratio > worst.0 {
                worst = (ratio, name.clone());
            }
            ours_total += ours.len();
            theirs_total += theirs;
            write!(
                report,
                "\n{name:<12} {:>4} {:>8} {theirs:>8} {:>+7.2}%",
                options.lgwin,
                ours.len(),
                (ratio - 1.0) * 100.0
            )
            .unwrap();
        }
    }
    write!(
        report,
        "\ntotal: {ours_total}/{theirs_total} {:+.2}%; worst {} {:+.2}%",
        (ours_total as f64 / theirs_total as f64 - 1.0) * 100.0,
        worst.1,
        (worst.0 - 1.0) * 100.0
    )
    .unwrap();
    (report, worst)
}

/// How much larger than the reference's a real response may be, compressed as served.
const REAL_LIMIT: f64 = 1.01;
/// The same at qualities 5 and 6 (owner's choice, 2026-11-07, kept after B14 on 2026-11-08):
/// the reference's tuned choice of dictionary words weighs more when two are looked up per
/// position, up to 1.2% on dictionary-heavy pages at windows 14 and 15, which both code with
/// one literal context (see B13 and B14 in `docs/plan/README.md`).
const CHAIN_REAL_LIMIT: f64 = 1.015;

/// Files in `bench-data/` (real responses, kept locally), each at the window
/// `Options::for_size` picks for it, as a server compresses them. They need more room than the
/// generated corpus: small real bodies lean on the dictionary, where the reference's tuned
/// choice of words gains up to 0.7% (see the B9 notes in `docs/plan/README.md`).
#[test]
#[ignore = "slow; run by scripts/gate --ratio"]
fn real_responses_as_served_are_within_the_limit_of_the_reference() {
    for (quality, limit) in [
        (4, REAL_LIMIT),
        (5, CHAIN_REAL_LIMIT),
        (6, CHAIN_REAL_LIMIT),
    ] {
        let Some((report, over)) = real_responses(quality, limit) else {
            println!("no files in bench-data/");
            return;
        };
        println!("{report}");
        assert!(
            over.is_empty(),
            "quality {quality}: over the limit: {over:?}"
        );
    }
}

/// The files of `bench-data/` at `quality`, each at the window `Options::for_size` picks: the
/// table, and the files more than `limit` times the reference's (and, from quality 5, more
/// than [`SMALL_SLACK`] bytes over, for outputs under 1 KiB); none if there are no files.
fn real_responses(quality: u32, limit: f64) -> Option<(String, Vec<String>)> {
    let real: Vec<(String, Vec<u8>)> = corpus()
        .into_iter()
        .filter(|(name, data)| name.starts_with("bench-data/") && !data.is_empty())
        .collect();
    if real.is_empty() {
        return None;
    }
    let mut worst = (0.0f64, String::new());
    let mut over = Vec::new();
    let slack = if quality > 4 { SMALL_SLACK } else { 0 };
    let (mut ours_total, mut theirs_total) = (0, 0);
    let mut report = format!("q{quality}");
    for (name, data) in &real {
        let options = oxbrotli::Options::for_size(quality, Some(data.len() as u64), 22);
        let ours = oxbrotli::compress_with(data, options);
        assert!(oxbrotli::decompress(&ours).unwrap() == *data, "{name}");
        let theirs = brotli_file(data, quality, options.lgwin).len();
        let ratio = ours.len() as f64 / theirs as f64;
        if ratio > worst.0 {
            worst = (ratio, name.clone());
        }
        let allowed = if theirs < 1024 { slack } else { 0 };
        if ours.len() as f64 > theirs as f64 * limit + allowed as f64 {
            over.push(format!("{name} {:+.2}%", (ratio - 1.0) * 100.0));
        }
        ours_total += ours.len();
        theirs_total += theirs;
        write!(
            report,
            "\n{name:<48} {:>9} w{:<2} {:>8} {theirs:>8} {:>+7.2}%",
            data.len(),
            options.lgwin,
            ours.len(),
            (ratio - 1.0) * 100.0
        )
        .unwrap();
    }
    write!(
        report,
        "\ntotal: {ours_total}/{theirs_total} {:+.2}%; worst {} {:+.2}%",
        (ours_total as f64 / theirs_total as f64 - 1.0) * 100.0,
        worst.1,
        (worst.0 - 1.0) * 100.0
    )
    .unwrap();
    Some((report, over))
}
