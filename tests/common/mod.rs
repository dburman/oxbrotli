//! Shared test helpers: a deterministic corpus and a runner for the reference `brotli` tool.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "shared test code: a panic is a test failure, and each file uses a subset"
)]

use std::io::Write;
use std::process::{Command, Stdio};

/// A small deterministic generator (xorshift), so the corpus is the same everywhere.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub(crate) fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub(crate) fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const WORDS: &[&str] = &[
    "the",
    "of",
    "and",
    "to",
    "in",
    "is",
    "that",
    "for",
    "it",
    "as",
    "was",
    "with",
    "be",
    "by",
    "on",
    "not",
    "he",
    "this",
    "are",
    "or",
    "his",
    "from",
    "at",
    "which",
    "but",
    "have",
    "an",
    "had",
    "they",
    "you",
    "were",
    "their",
    "one",
    "all",
    "we",
    "can",
    "her",
    "has",
    "there",
    "been",
    "if",
    "more",
    "when",
    "will",
    "would",
    "who",
    "so",
    "no",
    "garden",
    "weather",
    "proxy",
    "compression",
    "window",
    "sequence",
    "literal",
    "offset",
    "frame",
    "block",
];

/// English-like text of `len` bytes.
pub(crate) fn text(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        let w = WORDS[rng.below(WORDS.len() as u64) as usize];
        out.extend_from_slice(w.as_bytes());
        out.push(if rng.below(12) == 0 { b'\n' } else { b' ' });
    }
    out.truncate(len);
    out
}

/// Words of Greek, Russian and Chinese text, with some English and punctuation: letters of two
/// and three bytes in UTF-8, where quality 5 and up code literals by context.
const UTF8_WORDS: &[&str] = &[
    "καλημέρα",
    "κόσμε",
    "λόγος",
    "ήλιος",
    "θάλασσα",
    "ουρανός",
    "άνθρωπος",
    "πόλη",
    "привет",
    "мир",
    "слово",
    "солнце",
    "море",
    "человек",
    "город",
    "время",
    "你好",
    "世界",
    "文字",
    "太阳",
    "大海",
    "时间",
    "the",
    "of",
    "2026",
    "OK",
];

/// Multilingual UTF-8 text of `len` bytes (possibly ending inside a letter).
pub(crate) fn utf8_text(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        let w = UTF8_WORDS[rng.below(UTF8_WORDS.len() as u64) as usize];
        out.extend_from_slice(w.as_bytes());
        out.extend_from_slice(match rng.below(16) {
            0 => ". ".as_bytes(),
            1 => ", ".as_bytes(),
            2 => "\n".as_bytes(),
            3 => "。".as_bytes(),
            _ => " ".as_bytes(),
        });
    }
    out.truncate(len);
    out
}

/// HTML-like markup with repeated structure and varying content.
pub(crate) fn html(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut out = b"<!doctype html><html><head><title>test</title></head><body>\n".to_vec();
    while out.len() < len {
        let id = rng.below(100_000);
        let words = text(20 + rng.below(80) as usize, rng.next());
        out.extend_from_slice(
            format!("<div class=\"item\" id=\"i{id}\"><a href=\"/items/{id}\">").as_bytes(),
        );
        out.extend_from_slice(&words);
        out.extend_from_slice(b"</a></div>\n");
    }
    out.truncate(len);
    out
}

/// Random bytes (incompressible).
pub(crate) fn random(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    (0..len).map(|_| rng.next() as u8).collect()
}

/// CSS-like rules: selectors, properties and values from small fixed sets.
pub(crate) fn css(len: usize, seed: u64) -> Vec<u8> {
    const PROPS: &[&str] = &[
        "color",
        "margin",
        "padding",
        "display",
        "font-size",
        "border",
        "background",
        "line-height",
        "width",
        "max-width",
        "flex",
        "grid-template-columns",
    ];
    const VALUES: &[&str] = &[
        "0",
        "auto",
        "1rem",
        "0.5em",
        "#333",
        "#fafafa",
        "none",
        "flex",
        "block",
        "100%",
        "1px solid #ddd",
        "repeat(3, 1fr)",
        "inherit",
    ];
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(len + 64);
    while out.len() < len {
        let id = rng.below(5000);
        out.extend_from_slice(format!(".c{id} > .item:hover, #n{id} {{\n").as_bytes());
        for _ in 0..=rng.below(6) {
            let p = PROPS[rng.below(PROPS.len() as u64) as usize];
            let v = VALUES[rng.below(VALUES.len() as u64) as usize];
            out.extend_from_slice(format!("  {p}: {v};\n").as_bytes());
        }
        out.extend_from_slice(b"}\n");
    }
    out.truncate(len);
    out
}

/// JavaScript-like code: functions with repeated keywords and varying names.
pub(crate) fn js(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut out = b"\"use strict\";\n".to_vec();
    while out.len() < len {
        let f = rng.below(20_000);
        let a = rng.below(100);
        let b = rng.below(100);
        out.extend_from_slice(
            format!(
                "export function f{f}(a{a}, b{b}) {{\n  const x = a{a} + b{b} * {};\n  \
                 if (x > {}) {{ return document.querySelector(\"#n{f}\"); }}\n  \
                 return null;\n}}\n",
                rng.below(1000),
                rng.below(1000)
            )
            .as_bytes(),
        );
    }
    out.truncate(len);
    out
}

/// JSON-like records, as an API would return them.
pub(crate) fn json(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut out = b"[".to_vec();
    while out.len() < len {
        let id = rng.below(1_000_000);
        let name = text(5 + rng.below(30) as usize, rng.next());
        let name = String::from_utf8(name).unwrap().replace('\n', " ");
        out.extend_from_slice(
            format!(
                "{{\"id\":{id},\"name\":\"{name}\",\"active\":{},\"score\":{}.{}}},\n",
                rng.below(2) == 0,
                rng.below(100),
                rng.below(100)
            )
            .as_bytes(),
        );
    }
    out.truncate(len);
    out
}

/// The test corpus: names and contents, from empty to bigger than the reference's default
/// window (2^22 bytes).
pub(crate) fn corpus() -> Vec<(String, Vec<u8>)> {
    let mut skewed = Rng::new(9);
    let mut out: Vec<(String, Vec<u8>)> = vec![
        ("empty".into(), Vec::new()),
        ("one-byte".into(), b"x".to_vec()),
        ("short-text".into(), text(100, 1)),
        ("ones-1k".into(), vec![b'a'; 1024]),
        ("zeros-300k".into(), vec![0; 300_000]),
        ("text-200k".into(), text(200_000, 2)),
        ("utf8-text-200k".into(), utf8_text(200_000, 10)),
        ("html-500k".into(), html(500_000, 4)),
        ("css-100k".into(), css(100_000, 6)),
        ("js-200k".into(), js(200_000, 7)),
        ("json-150k".into(), json(150_000, 8)),
        ("random-64k".into(), random(65_536, 5)),
        (
            "skewed-300k".into(),
            (0..300_000)
                .map(|_| {
                    let r = skewed.below(100);
                    if r < 60 {
                        b'a'
                    } else if r < 90 {
                        b'b'
                    } else {
                        (r as u8) | 0x40
                    }
                })
                .collect(),
        ),
        ("text-5m".into(), text(5 << 20, 3)),
    ];
    out.extend(bench_data());
    out
}

/// The small entries of the corpus (at most 512 KiB), for the slow matrix tests.
pub(crate) fn small_corpus() -> Vec<(String, Vec<u8>)> {
    corpus()
        .into_iter()
        .filter(|(_, d)| d.len() <= 512 * 1024)
        .collect()
}

/// Files in `bench-data/` (not committed), when there are any.
fn bench_data() -> Vec<(String, Vec<u8>)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bench-data");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let name = format!("bench-data/{}", p.file_name().unwrap().to_string_lossy());
            (name, std::fs::read(&p).unwrap())
        })
        .collect()
}

/// Runs the reference `brotli` with `args`, feeding `input` on stdin; returns its status,
/// stdout and stderr.
fn run_brotli<S: AsRef<std::ffi::OsStr>>(args: &[S], input: &[u8]) -> (bool, Vec<u8>, String) {
    let mut child = Command::new("brotli")
        .args(args)
        .arg("-c")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the reference `brotli` tool must be installed (brew install brotli)");
    let mut stdin = child.stdin.take().unwrap();
    let data = input.to_vec();
    let writer = std::thread::spawn(move || {
        drop(stdin.write_all(&data));
    });
    let out = child.wait_with_output().unwrap();
    writer.join().unwrap();
    (
        out.status.success(),
        out.stdout,
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Compresses `input` with the reference `brotli` and `args` (such as `["-q", "4", "-w",
/// "19"]`).
pub(crate) fn brotli<S: AsRef<std::ffi::OsStr>>(args: &[S], input: &[u8]) -> Vec<u8> {
    let (ok, out, err) = run_brotli(args, input);
    let shown: Vec<String> = args
        .iter()
        .map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect();
    assert!(ok, "brotli {shown:?} failed: {err}");
    out
}

/// Compresses `input` with the reference at quality `q` and window `w`.
pub(crate) fn brotli_qw(input: &[u8], q: u32, w: u32) -> Vec<u8> {
    brotli(&["-q", &q.to_string(), "-w", &w.to_string()], input)
}

/// Decompresses `input` with the reference `brotli -d`; the error is its message.
pub(crate) fn unbrotli(input: &[u8]) -> Result<Vec<u8>, String> {
    match run_brotli(&["-d"], input) {
        (true, out, _) => Ok(out),
        (false, _, err) => Err(err),
    }
}

/// `brotli -q quality -w lgwin` on `input` given as a file, so the reference knows its size
/// (and picks its hasher by it) as a one-shot compressor does.
#[allow(dead_code, reason = "not every test uses every helper")]
pub(crate) fn brotli_file(input: &[u8], quality: u32, lgwin: u32) -> Vec<u8> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "input-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, input).unwrap();
    let out = Command::new("brotli")
        .args(["-q", &quality.to_string(), "-w", &lgwin.to_string(), "-c"])
        .arg(&path)
        .output()
        .expect("the reference `brotli` tool must be installed (brew install brotli)");
    std::fs::remove_file(&path).unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}
