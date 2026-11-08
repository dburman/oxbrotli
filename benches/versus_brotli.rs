//! Ratio and speed against the reference `brotli` tool.
//!
//! `cargo bench --bench versus_brotli -- [--rounds=N] [qualities] [names]`: qualities like
//! `4` or `0-11` (default 4-6, the qualities with encoders of their own), names filter the corpus. Files in `bench-data/` are added to the
//! corpus. Each input is compressed with the window `Options::for_size` gives it (at most 22),
//! by both.
//!
//! oxbrotli is timed in memory, the best of several runs, with its compressor and decompressor
//! reused from run to run: one-shot (`compress_with`) and as a stream (`StreamCompressor`, one
//! write and a finish). The reference has no benchmark mode, but takes many files in one
//! run: it is timed on 40 copies of the input and on 20, and the difference over 20 is its
//! time per input, with the process start cancelled out (the median of three). The files are
//! in `OXBROTLI_BENCH_DIR` if set, else `/dev/shm` if there is one (RAM-backed), else the
//! system temp directory (on macOS, in the page cache). Writing each output file costs it some
//! tens of microseconds, so its speeds are given only from 256 KiB. Each measurement takes `N`
//! rounds (default 3), oxbrotli and the reference in turn, and reports the median of each.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::cast_precision_loss,
    reason = "a benchmark: its output is its report, and a panic is a broken benchmark"
)]

#[path = "../tests/common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Files of the reference's per measurement: it runs on twice this many, then this many.
const TOOL_RUNS: usize = 20;
/// Inputs at least this large get the reference's speeds.
const TOOL_MIN: usize = 256 << 10;

fn qualities(arg: Option<&str>) -> Vec<u32> {
    let Some(arg) = arg else {
        return vec![4, 5, 6];
    };
    let mut out = Vec::new();
    for part in arg.split(',') {
        if let Some((a, b)) = part.split_once('-') {
            let (a, b): (u32, u32) = (a.parse().unwrap(), b.parse().unwrap());
            out.extend(a..=b);
        } else {
            out.push(part.parse().unwrap());
        }
    }
    out
}

/// Best time of `f` over runs filling about `budget`.
fn best<T>(budget: Duration, mut f: impl FnMut() -> T) -> (Duration, T) {
    let start = Instant::now();
    let mut best = Duration::MAX;
    let mut last;
    loop {
        let t = Instant::now();
        last = f();
        best = best.min(t.elapsed());
        if start.elapsed() >= budget {
            break;
        }
    }
    (best, last)
}

/// The median of `v` (not empty).
fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        f64::midpoint(v[n / 2 - 1], v[n / 2])
    }
}

/// The time, in seconds, of one run of `brotli args files...`.
fn tool(args: &[&str], files: &[PathBuf]) -> f64 {
    let t = Instant::now();
    let status = Command::new("brotli")
        .args(args)
        .args(files)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .expect("the reference `brotli` tool must be installed (brew install brotli)");
    let elapsed = t.elapsed().as_secs_f64();
    assert!(status.success(), "brotli {args:?} failed");
    elapsed
}

/// The time, in seconds, the reference takes per file for `brotli args` over `files`
/// (`2 * TOOL_RUNS` of them): a run over all of them less a run over half, so that starting
/// the process cancels out; the median of three.
fn per_file(args: &[&str], files: &[PathBuf]) -> f64 {
    let mut v: Vec<f64> = (0..3)
        .map(|_| {
            let half = tool(args, &files[..TOOL_RUNS]);
            let all = tool(args, files);
            (all - half) / TOOL_RUNS as f64
        })
        .collect();
    median(&mut v)
}

/// Where the reference's files go: RAM-backed if possible.
fn bench_dir() -> PathBuf {
    let base = std::env::var_os("OXBROTLI_BENCH_DIR")
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from("/dev/shm")).filter(|p| p.is_dir()))
        .unwrap_or_else(std::env::temp_dir);
    base.join(format!("oxbrotli-bench-{}", std::process::id()))
}

/// A speed in MB/s to three significant digits (whole numbers from 100).
fn speed(v: f64) -> String {
    if v >= 100.0 {
        format!("{v:.0}")
    } else if v >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

/// The reference's size and speeds (MB/s, if measured) on `data`, with its files in `dir`.
fn reference(data: &[u8], dir: &Path, quality: u32, lgwin: u32) -> (usize, Option<(f64, f64)>) {
    let (q, w) = (quality.to_string(), lgwin.to_string());
    let args = ["-q", &q, "-w", &w, "-f", "-k"];
    let (plain, packed) = (dir.join("plain"), dir.join("packed"));
    for d in [&plain, &packed] {
        drop(std::fs::remove_dir_all(d));
        std::fs::create_dir_all(d).unwrap();
    }
    let runs = if data.len() < TOOL_MIN {
        1
    } else {
        2 * TOOL_RUNS
    };
    let inputs: Vec<PathBuf> = (0..runs).map(|i| plain.join(format!("in{i}"))).collect();
    for f in &inputs {
        std::fs::write(f, data).unwrap();
    }
    if runs == 1 {
        tool(&args, &inputs);
        let size = std::fs::metadata(plain.join("in0.br")).unwrap().len();
        return (size as usize, None);
    }
    let ct = per_file(&args, &inputs);
    let out = std::fs::read(plain.join("in0.br")).unwrap();
    let streams: Vec<PathBuf> = (0..runs)
        .map(|i| packed.join(format!("in{i}.br")))
        .collect();
    for f in &streams {
        std::fs::write(f, &out).unwrap();
    }
    let dt = per_file(&["-d", "-f", "-k"], &streams);
    assert!(
        std::fs::read(packed.join("in0")).unwrap() == data,
        "the reference doesn't round-trip"
    );
    let mb = data.len() as f64 / 1e6;
    (out.len(), Some((mb / ct.max(1e-9), mb / dt.max(1e-9))))
}

/// Measures oxbrotli and the reference on `data` (also at `path`) at `quality`, and prints a
/// line of the report: sizes, then speeds in MB/s and oxbrotli's as a percentage of the
/// reference's.
fn measure(name: &str, data: &[u8], dir: &Path, quality: u32, rounds: usize) {
    let mb = data.len() as f64 / 1e6;
    let options = oxbrotli::Options::for_size(quality, Some(data.len() as u64), 22);
    let budget = Duration::from_millis(400);
    let mut stream = oxbrotli::StreamCompressor::new(options, Some(data.len() as u64));
    let mut decompressor = oxbrotli::Decompressor::new();
    let (mut packed, mut streamed, mut buf) = (Vec::new(), Vec::new(), Vec::new());
    let mut size = 0;
    let [mut oc, mut os, mut od, mut rc, mut rd] = [(); 5].map(|()| Vec::new());
    for _ in 0..rounds {
        let (ct, p) = best(budget, || oxbrotli::compress_with(data, options));
        packed = p;
        let (st, ()) = best(budget, || {
            streamed.clear();
            stream.write(data, &mut streamed);
            stream.finish(&mut streamed);
        });
        let (dt, ()) = best(budget, || {
            buf.clear();
            decompressor
                .decompress_into(&packed, &mut buf, usize::MAX)
                .unwrap();
        });
        assert!(
            buf == data,
            "{name} at quality {quality} doesn't round-trip"
        );
        oc.push(mb / ct.as_secs_f64());
        os.push(mb / st.as_secs_f64());
        od.push(mb / dt.as_secs_f64());
        let (s, speeds) = reference(data, dir, quality, options.lgwin);
        size = s;
        if let Some((c, d)) = speeds {
            rc.push(c);
            rd.push(d);
        }
    }
    let [oc, os, od] = [oc, os, od].map(|mut v| median(&mut v));
    let (rc, rd, pc, pd) = if rc.is_empty() {
        (
            "-".to_owned(),
            "-".to_owned(),
            "-".to_owned(),
            "-".to_owned(),
        )
    } else {
        let (c, d) = (median(&mut rc), median(&mut rd));
        (
            speed(c),
            speed(d),
            format!("{:.0}%", oc / c * 100.0),
            format!("{:.0}%", od / d * 100.0),
        )
    };
    println!(
        "{:<16} {:>2} {:>2} {:>9} {:>9} {:>+6.2}% {:>7} {:>7} {:>7} {:>5} {:>7} {:>7} {:>5}",
        name,
        quality,
        options.lgwin,
        packed.len(),
        size,
        (packed.len() as f64 / size as f64 - 1.0) * 100.0,
        speed(oc),
        speed(os),
        rc,
        pc,
        speed(od),
        rd,
        pd,
    );
}

fn main() {
    let rounds: usize = std::env::args()
        .find_map(|a| a.strip_prefix("--rounds=").map(|n| n.parse().unwrap()))
        .unwrap_or(3)
        .max(1);
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("--"))
        .collect();
    let qualities = qualities(args.first().map(String::as_str));
    let filters: Vec<&String> = args.iter().skip(1).collect();
    let dir = bench_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mut corpus: Vec<(String, Vec<u8>)> = common::corpus()
        .into_iter()
        .filter(|(n, d)| d.len() >= 100_000 && !n.starts_with("bench-data/"))
        .collect();
    // Web-sized bodies, where setting up (sizing and clearing tables) counts as much as
    // compressing.
    for (name, len) in [
        ("html-1k", 1 << 10),
        ("html-4k", 4 << 10),
        ("html-16k", 16 << 10),
        ("html-64k", 64 << 10),
        ("html-256k", 256 << 10),
        ("html-1m", 1 << 20),
    ] {
        corpus.push((name.to_owned(), common::html(len, 21)));
    }
    if let Ok(entries) = std::fs::read_dir("bench-data") {
        let mut files: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        for p in files {
            if let Ok(data) = std::fs::read(&p) {
                corpus.push((p.file_name().unwrap().to_string_lossy().into_owned(), data));
            }
        }
    }
    println!(
        "reference: per file, a run of the tool on {} copies less a run on {}, files in {}; \
         its speeds only from {} KiB",
        2 * TOOL_RUNS,
        TOOL_RUNS,
        dir.display(),
        TOOL_MIN >> 10
    );
    println!(
        "{:<16} {:>2} {:>2} {:>9} {:>9} {:>7} {:>7} {:>7} {:>7} {:>5} {:>7} {:>7} {:>5}",
        "input",
        "q",
        "w",
        "oxbrotli",
        "brotli",
        "size",
        "ox comp",
        "ox strm",
        "br comp",
        "comp",
        "ox dec",
        "br dec",
        "dec"
    );
    for (name, data) in &corpus {
        if !filters.is_empty() && !filters.iter().any(|f| name.contains(f.as_str())) {
            continue;
        }
        for &quality in &qualities {
            measure(name, data, &dir, quality, rounds);
        }
    }
    drop(std::fs::remove_dir_all(&dir));
}
