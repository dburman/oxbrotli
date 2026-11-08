<h1 align="center">
  <img src="docs/assets/logo.png" alt="oxbrotli" width="220">
</h1>

**oxbrotli is a Rust library that compresses and decompresses data in the Brotli format**,
written entirely in safe Rust, with no dependencies and no C code.

[Brotli](https://www.rfc-editor.org/rfc/rfc7932) (RFC 7932) is the compression format
behind `Content-Encoding: br` on the web: browsers have accepted it for years, and it
compresses text, HTML, CSS and JavaScript smaller than gzip. The usual Rust crates for it
are bindings to Google's C library or a line-by-line translation of it. oxbrotli is a
separate implementation from the RFC:

- **Same format, both ways:** the goal is that anything oxbrotli compresses decompresses
  with the reference `brotli` tool, and anything the reference compresses, at any quality
  and window, decompresses with oxbrotli. The tests check both directions against the real
  `brotli`.
- **No `unsafe`:** the crate forbids it, and its policy check makes sure nothing re-allows
  it. Malformed input must give an error, never a panic.
- **No dependencies and no C:** no C compiler in the build, nothing to cross-compile but
  Rust, and nothing else to audit.

**Status:** 0.2.0. Decompression is complete: everything the reference
writes, at every quality and window, decodes byte for byte, and damaged input gives an error.
Compression works at qualities 4, 5 and 6: on the test corpus the output is the size of
`brotli -q 4`, `-q 5` and `-q 6`'s, give or take 0.5% per entry, and the reference decodes
everything it writes, in one go or as a stream with flushes. Qualities 0 to 3 compress as 4
does, and 7 to 11 as 6 does.

| quality | output against 4 | speed against 4 | memory per compressor |
|---|---|---|---|
| 4 | | | sized to the body, up to 4 MiB |
| 5 | 1% to 14% smaller | 55% to 70% | 512 KiB for small bodies, up to 2 MiB |
| 6 | another 1% to 6% smaller | 42% to 57% | 512 KiB for small bodies, up to 4 MiB |

Both directions run at about the reference's speed, at every quality
([`docs/benchmarks.md`](docs/benchmarks.md)).
See [`docs/plan/README.md`](docs/plan/README.md) for the milestones.

## Use

oxbrotli isn't on crates.io yet. Add it from Git:

```toml
[dependencies]
oxbrotli = { git = "https://github.com/dburman/oxbrotli", tag = "0.2.0" }
```

```rust
let text = b"Brotli in safe Rust. ".repeat(100);
let stream = oxbrotli::compress(&text, 4);
assert_eq!(oxbrotli::decompress(&stream).unwrap(), text);
```

`compress_with` takes `Options`, for the window: `Options::new(4).lgwin(18)`.

For a body that arrives in pieces, `StreamCompressor` takes it chunk by chunk; `flush` makes
everything so far decodable. The compressor keeps its memory from one stream to the next, and
`set_options` and `set_size_hint` fit it to each new stream (its window, and its hash table):

```rust
use oxbrotli::{Options, StreamCompressor};

// The smallest window that holds a body of known size, at most 2^19.
let options = Options::for_size(4, Some(11), 19);
let mut c = StreamCompressor::new(options, Some(11));
let mut out = Vec::new();
c.write(b"hello ", &mut out);
c.flush(&mut out);
c.write(b"world", &mut out);
c.finish(&mut out);
assert_eq!(oxbrotli::decompress(&out).unwrap(), b"hello world");
```

**Untrusted input:** `decompress` has no limit on the output size, so a small malicious
stream could expand to gigabytes. Use `decompress_with_limit` or
`Decompressor::decompress_into` with a limit when the input comes from outside: the output
buffer never grows past it.

## Development

`scripts/gate` runs every check: formatting, clippy, the tests, the policy check and
`cargo deny`. The interop tests run the reference `brotli` program (`brew install brotli`,
`apt-get install brotli`) and fail without it. Files put in `bench-data/` (not committed)
are added to the test corpus.

## License

MIT; see [LICENSE](LICENSE).
