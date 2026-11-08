# oxbrotli implementation plan

oxbrotli is a Brotli (RFC 7932) compressor and decompressor in safe Rust: `unsafe` is
forbidden, there are no dependencies, and no C is built or linked. The C `brotli`
command-line tool is used only from outside, as a reference: tests check that it decodes
everything oxbrotli writes and that oxbrotli decodes everything it writes, and benchmarks
compare ratio and speed.

Its first user is a web proxy that compresses responses on the fly at quality 4, with
windows of 2^10 to 2^19 bytes, flushing as it goes. So quality 4 comes first: the goal for
0.1 is output within 0.5% of the reference's size at quality 4, compression at least as fast
as the `brotli` crate's, and a decoder for everything the reference writes. The other
qualities come after.

## Milestones

| Milestone | Contents | Exit | Status |
|---|---|---|---|
| B0 Skeleton | Repository, crate, lints, policy, gate, CI | Gate passes on an empty library | Done 2026-11-05 |
| B1 Bits and prefix codes | Bit reader; simple and complex prefix codes; table decoding | Unit tests for every code shape in RFC §3 | Done 2026-11-05 |
| B2 Meta-blocks | Stream header, meta-block header, uncompressed and metadata meta-blocks, block switches, context maps, commands, distances | Decodes `brotli -q 0..=9` output that uses no dictionary references | Done 2026-11-05: all reference output decodes or fails only at a dictionary reference; qualities 0, 1 and 3 decode in full |
| B3 Dictionary and transforms | The 122,784-byte static dictionary, 121 transforms, dictionary references | Decodes everything `brotli -q 0..=11 -w 10..=24` writes on the corpus | Done 2026-11-05: 2,160 reference streams decode byte for byte. Dictionary SHA-256 `20e42eb1b511c21806d4d227d07e5dd06877d8ce7b3a817f378f313653f35c70`, identical to google/brotli's |
| B4 Decoder hardening | Output limits, `Decompressor` reuse, damage tests, interop suite | Every truncation and bit flip gives an error, never a panic | Done 2026-11-05 |
| B5 Entropy encoder | Bit writer, length-limited prefix codes, storing codes, one-block meta-blocks, uncompressed fallback | Placeholder (literal-only) output decodes with `brotli -d` | Done 2026-11-05 |
| B6 Quality 4 matching | Hash match finder at quality 4, insert-and-copy commands, distance ring, dictionary matches | Ratio within 1% of `brotli -q 4` on the corpus, at each window | Done 2026-11-05 |
| B7 Block splitting | Greedy block splitting and histogram tuning as the reference does at quality 4 | Ratio within 0.5% of `brotli -q 4` | Done 2026-11-06 |
| B8 Streaming | `StreamCompressor`: write, flush (byte-aligned), finish, reset, size hint | Write, flush, finish and reuse, with every flushed prefix decodable | Done 2026-11-06 |
| B9 Speed | Profiling-driven work on the hot loops; `benches/versus_brotli.rs`; `docs/benchmarks.md` | Faster than the `brotli` crate at q4 on web-sized bodies (1 KiB to 1 MiB) | Done 2026-11-06: as fast as the C reference (the crate comparison is the caller's, B10) |
| B11 Quality levels | `Options::quality` chooses the encoder: 5 and 6 get their own, 0-4 compress as 4, 7-11 as 6 | Tests pin the mapping; quality 4 output unchanged | Done 2026-11-06 |
| B12 Chain hashers | H5 and H6: buckets of the last 2^(q-1) positions; four last distances tried; two dictionary words per position | Ratio within 1% of `brotli -q 5` / `-q 6` at window 19, with one literal context | Done 2026-11-06: corpus -0.00% in total at both; real files +0.68% (q5), web bodies -0.12% |
| B13 Forgetful chains | H40 for windows of 16 or less | Ratio within 1% of the reference at windows 10 and 15 | Done 2026-11-08: -0.78% to +0.07% per entry; web bodies -0.13% |
| B14 Context modeling | Literal contexts (UTF-8 mode): the choice of 1, 2 or 13 contexts, the context block splitter, context maps written in full | Ratio within 0.5% of `brotli -q 5` / `-q 6` at windows 10, 15 and 19 | Done 2026-11-08: corpus -0.03% in total at both; real files +0.04% (q5), +0.09% (q6) |
| B15 Release | Speed and memory at 5 and 6 measured; docs; 0.2.0 | `docs/benchmarks.md` has quality 5 and 6 rows; gate passes | Done 2026-11-08: 5 and 6 at 100% to 117% of the reference's speed; tag on the owner's word |


Qualities 5 and 6 are B11 to B15 (planned 2026-11-06). Later and not scheduled: the other
quality levels (0-3, 7-11), a streaming decoder,
large-window Brotli (2^25 to 2^30, not part of RFC 7932) and shared dictionaries (RFC 9842).
Each one needs a plan before it starts.


---

## Rules for every work order

Read these before every work order.

1. **No `unsafe`, no dependencies, no C.** `Cargo.toml` forbids `unsafe_code`, `deny.toml`
   allows no crates, and `scripts/check-policy.sh` checks both. That rules out
   `[dev-dependencies]` too: the reference is the `brotli` program, not a crate. If something
   seems to need a crate, **stop and ask**.
2. **Tests in the same change**, for the success path and every failure path. Malformed input
   must return an error, never panic. `tests/damage.rs` enforces this from B4 on.
3. **Interoperability:** `brotli -d` must decompress everything oxbrotli compresses, and
   oxbrotli must decompress everything `brotli` compresses (every quality, every window).
   `tests/interop.rs` checks both directions. If the `brotli` program is missing, those tests
   **fail**. They are never skipped.
4. **Mutation check** on new logic: break it on purpose, confirm a test fails, then restore it.
5. **Gate:** `scripts/gate` in the oxbrotli repository runs `cargo fmt --check`,
   `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo bench --no-run`,
   `scripts/check-policy.sh` and `cargo deny check`, and stops at the first failure.
6. **Commit only when the owner says so,** and always through the pre-commit hook
   (`git config core.hooksPath .githooks`): it strips watermarks from the staged files and
   stops the commit if it changed any. Never use `--no-verify` on new content.
7. **No AI attribution** anywhere: code, comments, docs, commit messages.
8. **The RFC is the spec.** Section numbers below refer to RFC 7932. Where the RFC and the
   reference program disagree, **stop and report it**. Never quietly match one of them.
9. **The reference C source may be read, never copied.** google/brotli (MIT) is the place to
   look up what the reference *does* at quality 4. Write our own code from what you learn.
   Name the reference file and function in a comment where our code follows its approach,
   as oxzstd does.
10. **Report format:**

    ```
    Work order: Bn <title>
    Files changed: <list>
    Tests added: <count> (<file>: <test names>)
    Mutation check: <what you broke, which test caught it>
    Gate: scripts/gate ok (<N> tests passed)
    Ratio vs brotli -q 4: <table, from B6 on>
    Deviations from the work order: <none | what and why>
    Open questions: <none | ...>
    ```

### Code conventions

Match oxzstd. `oxbrotli/Cargo.toml` copies `oxzstd/Cargo.toml`'s lints exactly, including
the codec allowances (`indexing_slicing`, the `cast_*` lints, `similar_names`,
`inline_always`) and their reasons. Specifically:

- **Indexing is allowed and bounds-checked.** A panic is still a bug. The damage tests and
  the corpus look for panics.
- **Errors:** one hand-written `Error` enum (no `thiserror`) shaped like oxzstd's:
  `Truncated`, `Corrupt(&'static str)` (the text names the field or RFC rule that failed),
  `TooLarge`, `TrailingData`. `Display` messages start with `"... Brotli data"` the way
  oxzstd's start with `"... Zstandard data"`.
- **Bounded everything:** output limited by the caller, window at most 2^24, nothing an
  attacker controls can allocate more than `limit + window`.
- **Docs on every public item,** with a runnable example on each entry point.

### Test conventions

- **Corpus:** `tests/common/mod.rs` is a copy of oxzstd's: the xorshift `Rng`, the generated
  text and the HTML. Add generated **CSS, JavaScript and JSON**, since those are what web
  servers compress, plus edge inputs: empty, 1 byte, 1 KiB of one byte, 64 KiB of random bytes, and
  a 3 MiB text (bigger than the largest window). Files in `bench-data/` (gitignored) are
  added when present. Never commit third-party files.
- **`brotli` runner:** `common::brotli(args, input) -> Vec<u8>` and
  `common::unbrotli(input) -> Result<Vec<u8>, String>` pipe through the program, as
  `common::zstd` does.
- **Table-driven** where there are many cases. Every assertion message names the input,
  quality and window.

---

---

## B0 Skeleton

Done 2026-11-05. The repository has oxzstd's lints, profiles, toolchain pin, `deny.toml`
(no crates allowed) and CI (with `brotli` for `zstd`), plus:

- `scripts/check-policy.sh`: oxzstd's checks, and no `include!`, `include_bytes!` or
  `include_str!` of an absolute path or one with `..`.
- `scripts/gate`: `brotli --version`, fmt, clippy, tests (release), benchmark build, policy,
  `cargo deny`; `--ratio` adds the ratio comparison from B6 on.
- `.githooks/pre-commit`: the watermark cleaner, as rule 6 says.
- `tests/common/mod.rs`: the corpus (text, HTML, CSS, JavaScript, JSON, zeros, random,
  skewed, and a 5 MiB text bigger than the reference's default window; `small_corpus()` is
  the entries up to 512 KiB for the slow matrix tests) and the `brotli` runners. Its tests
  are in `tests/corpus.rs`, so they run once rather than in every test binary.

---

## B1 Bits and prefix codes

Goal: read bits and decode RFC 7932 prefix codes.
Depends on: B0.
Read first: RFC 7932 §2 (bit order) and §3 (all of it).
Files: `src/bits.rs`, `src/prefix.rs`.

Spec:
- `BitReader<'a>` over `&'a [u8]`: LSB-first. It has `bits(n: u32) -> Result<u32>` for
  `n <= 24`, `bit()`, `align_to_byte()` (which returns the skipped bits so the caller can
  check they are zero), `bytes(n) -> Result<&[u8]>` (only when aligned) and `position()`.
  Reading past the end returns `Error::Truncated`. Refill 64 bits at a time, as oxzstd's
  `bits.rs` does.
- `PrefixCode` with `read_from(r, alphabet_size)`:
  - **Simple codes** (§3.4): `HSKIP == 1`, NSYM 1 to 4, symbols of `ALPHABET_BITS` width. For
    NSYM 4, the tree-select bit picks lengths `2,2,2,2` or `1,2,3,3`. A repeated symbol, or a
    symbol at or above the alphabet size, is `Corrupt`.
  - **Complex codes** (§3.5): the code-length code lengths, in the order
    `1,2,3,4,0,5,17,6,16,7,8,9,10,11,12,13,14,15`, are read with the fixed variable-length
    code from §3.5 and skip `HSKIP` entries. Then come the symbol code lengths, including
    repeat code 16 (repeat the previous non-zero length; the initial "previous" is 8) and
    17 (repeat zero), with the §3.5 rule for consecutive repeat codes adding to each other.
  - **Completeness:** reading stops as soon as a code is complete, so only the last length
    can oversubscribe it. The code-length code must have a Kraft sum of exactly 1 (in units
    of 2^-5), or exactly one non-zero length, which is a code with one symbol and no bits.
    The symbol code lengths must sum to exactly 2^15 (in units of 2^-15). One non-zero
    length is not enough there: a complex code needs at least two (§3.5). Anything else is
    `Corrupt`.
- Decoding uses a two-level lookup table: 8 root bits, with second-level tables for longer
  codes, so one symbol costs one or two lookups.
- The alphabets (§3.3) are `256` (literals), `704` (insert-and-copy),
  `16 + NDIRECT + (48 << NPOSTFIX)` (distances), `NBLTYPES + 2` (block types) and `26`
  (block counts). They are the caller's responsibility; `PrefixCode` takes the size.

Tests (`#[cfg(test)]` in each file; hand-built bit strings):
- Bit reader: reads across byte boundaries, reads of 0 bits, reads of 24 bits, truncation at
  every byte of a 4-byte input, non-zero alignment bits reported.
- Simple codes: NSYM 1, 2, 3, 4 (both tree shapes) decode a fixed message. A duplicate symbol
  is `Corrupt("simple prefix code repeats a symbol")`. A symbol past the alphabet is
  `Corrupt`.
- Complex codes: a 256-symbol code with all lengths 8. A code using 16 and 17 with
  consecutive repeats (the §3.5 example). An incomplete code (Kraft sum under 2^15) and an
  oversubscribed code are both `Corrupt`. A one-symbol complex code decodes with zero bits.
- Round trip: 1,000 random length sets (Kraft-complete, made by the test) are written by a
  test-only canonical encoder and decoded back.

Done when: the gate passes and the mutation check is done (break the Kraft check; a test
fails).

---

## B2 Meta-blocks

Goal: decode every Brotli stream that has no dictionary references.
Depends on: B1.
Read first: RFC 7932 §4, §5, §6, §7, §9 and §10.
Files: `src/decode.rs` (the stream and meta-block state machine), `src/context.rs`,
`src/tables.rs`, `src/lib.rs` (public functions).

Spec:
- **Stream header** (§9.1): decode WBITS exactly as specified. The window is
  `(1 << WBITS) - 16` bytes, and valid WBITS run from 10 to 24. The 7-bit pattern that
  RFC 7932 leaves reserved (the reference uses it to mark large-window streams) is
  `Corrupt("large-window Brotli is not supported")`.
- **Meta-block header** (§9.2): `ISLAST`, `ISLASTEMPTY`, `MNIBBLES` (4, 5, 6, or 0 for
  metadata), `MLEN-1`. When `MNIBBLES > 4`, a last nibble of zero is `Corrupt`. Then
  `ISUNCOMPRESSED` (only when not `ISLAST`).
- **Metadata meta-blocks:** a reserved bit that must be 0, then `MSKIPBYTES`, then skipped
  bytes. A last skip byte of zero (when `MSKIPBYTES > 1`) is `Corrupt`. The skipped content
  is ignored.
- **Uncompressed meta-blocks:** align to a byte. The padding bits must be zero, otherwise
  `Corrupt`. Then copy `MLEN` bytes.
- **Compressed meta-blocks** (§9.2, §9.3): for each category L, I, D, read `NBLTYPES`, its
  block-type and block-count codes, and the first block count. Then `NPOSTFIX` (2 bits),
  `NDIRECT` (4 bits `<< NPOSTFIX`), one 2-bit context mode per literal block type,
  `NTREESL` and the literal context map (64 per type), `NTREESD` and the distance context map
  (4 per type), then the `NTREESL` literal codes, `NBLTYPESI` insert-and-copy codes and
  `NTREESD` distance codes.
- **Context maps** (§7.3): `RLEMAX` and run-length zero codes, and the inverse move-to-front
  when the IMTF bit is set. A map value of `NTREES` or more is `Corrupt`.
- **Block switches** (§6): the block-type code uses the two-previous-types rule. Block counts
  come from the 26-entry table (§6) plus their extra bits.
- **Commands** (§5): the 704 insert-and-copy codes, laid out as in the §5 table. Codes 0 to
  127 use the implicit distance (the last distance, which is not pushed). Insert lengths are
  0-23 and copy lengths 0-23, each with offsets and extra bits from the §5 tables.
- **Distances** (§4): the 16 short codes against the ring of the last four distances (with
  the ±1..±3 adjustments), direct codes, and the `NPOSTFIX` and `NDIRECT` formula for the
  rest. Distance code 0 doesn't change the ring. A resulting distance of 0 or less is
  `Corrupt`.
- **Literal contexts** (§7.1): the `LSB6`, `MSB6`, `UTF8` and `Signed` modes from the last
  two output bytes, using the Lut0/Lut1/Lut2 tables. (Done differently: the tables are in
  `src/context_lut.rs`, generated by `scripts/extract-rfc.py` from the RFC text and checked
  against the CRC-32 values §7.1 lists, so none of them is typed by hand.) The distance context is `copy_len - 2` clamped to 0..=3 (§7.2).
- A distance larger than `min(window, bytes output so far)` is a dictionary reference. In
  B2 that returns `Error::Corrupt("dictionary reference")`; B3 replaces it.
- After the `ISLAST` meta-block, the remaining bits of the last byte must be zero (§9.2).
  In `decompress`, more bytes after the stream is `TrailingData`.
- **Public API (this milestone):**
  `pub fn decompress(src: &[u8]) -> Result<Vec<u8>, Error>` and
  `pub fn decompress_with_limit(src: &[u8], limit: usize) -> Result<Vec<u8>, Error>`.
  `TooLarge` is returned as soon as the output would pass `limit`, before allocating past it.

Tests:
- Unit tests for each table: insert and copy length ranges at both ends of every code, and
  every short distance code against a known ring.
- Hand-assembled streams (built with B1's test encoder): an empty stream (`ISLAST`,
  `ISLASTEMPTY`), one uncompressed meta-block, a metadata block followed by data, and
  non-zero padding (`Corrupt`).
- `tests/interop.rs` (first part): for qualities 0 to 9 and windows `10, 16, 19, 22, 24`,
  every corpus entry compressed by `brotli -q Q -w W` decodes byte for byte. **If a stream
  fails only with "dictionary reference",** mark that case `B3` and move on. Any other
  failure is a bug.
- `decompress_with_limit`: limit one byte under the output gives `TooLarge`, exactly the
  output size works.

Done when: the gate passes; all of the interop cases above decode except the ones marked B3.
Stop and ask if: the reference produces a stream the RFC calls invalid.

---

## B3 Dictionary and transforms

Goal: dictionary references, the last thing the decoder needs.
Depends on: B2.
Read first: RFC 7932 §8, Appendix A and Appendix B.
Files: `src/dictionary.bin`, `src/dictionary.rs`, `src/transform.rs`,
`scripts/extract-dictionary.sh`.

Done differently: one script, `scripts/extract-rfc.py`, extracts the dictionary, NDBITS and
the transforms (and B2's context tables) and checks each against the length and CRC-32 the
RFC lists, so nothing is typed by hand. The RFC calls the uppercase transforms
`FermentFirst` and `FermentAll` and defines them with an XOR rule on UTF-8 lead bytes;
oxbrotli follows that definition exactly.

Spec:
- `scripts/extract-dictionary.sh` takes a copy of the RFC text (`rfc7932.txt`, which the
  owner downloads; the script does no network access). It rebuilds `src/dictionary.bin`
  from Appendix A's hex and prints its size and `shasum -a 256`. The size must be **122,784
  bytes**. Write the hash into a comment in `dictionary.rs` and into this plan's status line
  for B3. Also compare the file with google/brotli's `c/common/dictionary.bin` (the owner
  supplies it) and report whether they are identical. **Stop if they differ.**
- `dictionary.rs`: `static DICTIONARY: &[u8; 122_784] = include_bytes!("dictionary.bin");`,
  the `NDBITS` table for word lengths 4 to 24 and the offsets derived from it (§8). A
  `#[test]` checks that the offsets sum to 122,784.
- `transform.rs`: the 121 transforms from Appendix B, as `(prefix, kind, suffix)` with `kind`
  one of `Identity`, `OmitLast(1..=9)`, `UppercaseFirst`, `UppercaseAll` and
  `OmitFirst(1..=9)`. The uppercase rules are §8's UTF-8 rule (1, 2 or 3 bytes), not ASCII.
  Generate the table with the same script from Appendix B, so nothing is typed by hand.
- Decoding a reference (§8): with `max = min(window, output so far)`,
  `word_id = distance - max - 1`, `index = word_id & ((1 << NDBITS[len]) - 1)` and
  `transform_id = word_id >> NDBITS[len]`. A `len` outside 4..=24, a length whose `NDBITS`
  is 0, or `transform_id >= 121` is `Corrupt`. Dictionary references don't go into the
  distance ring.

Tests:
- One test per transform kind against a fixed word (`"time"`, `"él"` for UTF-8 uppercase,
  `"a"` with `OmitLast(9)` giving the empty string).
- `tests/interop.rs`: the full matrix of qualities 0 to 11 and windows 10 to 24, every value,
  on the whole corpus. Nothing marked B3 is left.
- A stream with `transform_id = 121`, built by hand, is `Corrupt`.

Done when: the gate passes, the full interop matrix decodes, and the dictionary hash is
recorded.

---

## B4 Decoder hardening

Goal: prove that malformed input never panics and that limits hold.
Depends on: B3.
Files: `tests/damage.rs`, `src/lib.rs` (`Decompressor`), `examples/decompress.rs`.

Done differently: the decoder keeps the whole output, so it needs no window buffer of its
own: memory is the output buffer, which never grows past the limit (even by `Vec`'s
doubling), plus the prefix code tables of one meta-block. The damage tests use the
reference's streams (oxbrotli has no compressor yet) and sample the long ones, and
`scripts/gate` also runs them in debug, with overflow checks on.

Spec:
- `pub struct Decompressor` with `new()` and `decompress_into(&mut self, src, out: &mut
  Vec<u8>, limit: usize) -> Result<(), Error>`. It reuses its tables between calls, like
  oxzstd's, and appends to `out`.
- Peak memory for a stream with window `W` and output limit `L` is at most `L + 2^W` plus
  tables of fixed size. Document this on `decompress_with_limit`.

Tests (`tests/damage.rs`, copied in shape from oxzstd's):
- For three corpus entries at qualities 1, 4 and 11: every truncation (each prefix length)
  returns an error, never `Ok` with short output.
- Every single bit flip in the first 4 KiB, and 10,000 random flips over the rest (seeded
  `Rng`), returns `Ok` or `Err` and never panics. When it returns `Ok` and the length
  matches, the data may differ, because Brotli has no checksum. Note that in a comment.
- 100,000 random byte strings, 0 to 512 bytes long, never panic.
- A hand-built "zip bomb" (a short stream claiming a 16 MiB output) with a 1 MiB limit
  returns `TooLarge`, and `out.capacity()` after the error is no more than the limit plus one
  window. (Don't use a counting global allocator: implementing `GlobalAlloc` needs
  `unsafe impl`.)

Done when: the gate passes, and `cargo test --release --test damage` finishes in under two
minutes.
Stop and ask if: a bit flip makes the decoder loop forever. That's a bug to fix, but report
what caused it.

---

## B5 Entropy encoder

Goal: write valid Brotli from a list of commands, with no match finding yet.
Depends on: B4 (the decoder checks every encoder test).
Read first: RFC 7932 §3 and §9 again, from the writer's side. google/brotli
`c/enc/entropy_encode.c` and `c/enc/brotli_bit_stream.c` for the reference's approach.
Files: `src/bitwriter.rs`, `src/encode/huffman.rs`, `src/encode/store.rs`,
`src/encode/mod.rs`.

Done differently: the meta-block writer already takes real commands (`Command`: insert,
copy, `DistanceCode`), with copies, explicit distances and distance code 0 in every cell,
tested through our decoder; B6 adds the match finder and the distance ring, not the writer.
`write_meta_block` returns whether it wrote the block compressed, because an uncompressed
fallback leaves the decoder's ring as it was. The literal-only compressor writes 64 KiB
meta-blocks. The last one is marked ISLAST when compressed; when it falls back to
uncompressed, an empty last meta-block follows. `Options::new(quality).lgwin(w)` builds
options, and `Options::default()` is quality 4, window 22. The 1% bound on length-limited
codes is checked at the limit of 15 only. At 5, any code for a skewed 18-symbol histogram
costs much more than Huffman's, so only completeness and the limit are checked there. The
length-16 mutation needed an input whose Huffman code is exactly 16 deep (17 Fibonacci
counts): count doubling jumps from deeper trees straight to 15 or less.

Spec:
- `BitWriter`: LSB-first, 64-bit accumulator, writing into a caller's `Vec<u8>`, with
  `align_to_byte()`.
- Building prefix codes: from symbol counts to code lengths with a **maximum length of 15**
  (literal, command and distance codes) or 5 (the code-length code). Use any correct
  length-limited method; the reference uses a Huffman tree with count doubling. Then
  canonical codes.
- Storing codes: use a simple code when 4 symbols or fewer are used. Otherwise use a complex
  code with run-length coding of the lengths (codes 16 and 17) and the best `HSKIP`.
- Meta-block writer: one block type per category, `NPOSTFIX = 0`, `NDIRECT = 0`, literal
  context mode `LSB6` with a context map that sends all 64 contexts to one tree (`NTREESL =
  1`, `NTREESD = 1`).
- If a compressed meta-block would be larger than storing its bytes, write an uncompressed
  meta-block instead (§9.2), and keep the distance ring as it was.
- Stream header for any `lgwin` from 10 to 24 (§9.1).
- `pub fn compress(src: &[u8], quality: u32) -> Vec<u8>` exists from here on, with a
  literal-only placeholder encoder. `Options { quality, lgwin }`. Qualities other than 4 are
  clamped to 4 for now; document it.

Tests:
- Length-limited codes: for 1,000 random histograms, including skewed ones that would give
  lengths over 15, the lengths are Kraft-complete, none is over the limit, and the encoded
  size is within 1% of the unlimited code's.
- Every corpus entry round-trips through our decoder and `brotli -d`, at windows 10, 16, 19
  and 24.
- Incompressible input (random bytes) comes out as uncompressed meta-blocks, no more than
  `len + len / 16 + 16` bytes.
- The empty input gives the shortest valid stream, and `brotli -d` accepts it.

Done when: the gate passes, the mutation check is done (allow length 16; a test fails).

---

## B6 Quality 4 matching

Goal: real compression at quality 4, the setting the first user needs.
Depends on: B5.
Read first: google/brotli `c/enc/quality.h` (`ChooseHasher` and the `MIN_QUALITY_FOR_*`
constants), `c/enc/hash.h`, `c/enc/hash_longest_match_quickly_inc.h`,
`c/enc/backward_references_inc.h`, `c/enc/static_dict.c`.
Files: `src/encode/hash.rs`, `src/encode/matcher.rs`, `src/encode/command.rs`,
`src/encode/static_dict.rs`.

**What the reference does at quality 4** (google/brotli v1.2.1, `db24a98`, read 2026-11-05):

- *Hasher* (`quality.h`, `ChooseHasher`; `hash.h`): H4 (the "quickly" hasher: 2^17 slots,
  each hash filed in one of 4 slots 8 apart, 5 bytes hashed, static dictionary on), or H54
  (2^20 slots, 7 bytes hashed, no dictionary) when the size hint is 1 MiB or more.
  Confirmed. The table size is fixed by the hasher, not by `lgwin`.
- *Match choice* (`hash_longest_match_quickly_inc.h`): the last distance is tried first,
  then the 4 slots. Score is `1920 + 135 * len - 30 * log2(distance)`, and the last distance
  scores `1920 + 135 * len + 15`. A match needs at least 4 bytes and a score over 2020.
  Only the last distance is tried, not the 4 to 16 ring candidates of higher qualities.
  Confirmed (the "last-distance bonus").
- *Lazy matching* (`backward_references_inc.h`): a match one byte later wins if it scores
  175 more, up to 4 times in a row. After 64 literals without a match, positions are tried
  every 2 bytes, and after 256 more every 4. Not in the guess; implemented.
- *Distance codes* (`backward_references.c`, `ComputeDistanceCode`): short codes in a fixed
  order: last, second, last ±1..3, second ±1..3, third, fourth. Not "the shortest code", which
  isn't known before the codes are built. Implemented in that order.
- *Static dictionary* (`hash.h`, `SearchInStaticDictionary`, shallow): one lookup per
  position, only when no copy was found, for a word of 8 bytes or more filed under the hash
  of the next 4 bytes. It matches whole or with its last 1 to 9 bytes cut (OmitLast1-9). The
  lookups stop when fewer than 1 in 128 find a word. Confirmed.
- *Meta-blocks* (`encode.c`, `EncodeData`): input in 64 KiB blocks (`lgblock` 16). Blocks
  join one meta-block up to `2^(1 + max(lgwin, 16))` bytes, or until literals or commands
  reach an eighth of that. A copy ending at a block's end runs on into the next
  (`ExtendLastCommand`).
- *For B7, already read*: block splitting is on at quality 4
  (`MIN_QUALITY_FOR_BLOCK_SPLIT` 4, `BrotliBuildMetaBlockGreedy`), with one literal context
  (context modeling starts at 5). `BrotliOptimizeHistograms` runs (4). `NPOSTFIX` and
  `NDIRECT` stay 0 unless the caller sets them.

Done differently:
- *The hash table follows the input size*: twice the input, rounded up to a power of two,
  between 4 KiB and the reference's size. The reference always takes the full 512 KiB (H4)
  or 4 MiB (H54), which for a 1 KiB body is most of its memory. It keeps the same hash (its
  top bits), so the ratio is unchanged on the corpus.
- *Dictionary slots, our rule*: the reference picks the word for each slot with a tuned
  1,688-byte bitmap (`dictionary_hash.c`, `frozen_idx`), which rule 9 doesn't let us copy.
  Ours keeps the shortest word, then the lowest index. The other way round (longest first)
  was tried: short text 6% smaller, but css-100k at w10 1.4% larger than the reference's.
  Building the table takes 8 µs (release, M-series Mac): far under 2 ms, so no generated
  table.
- *Uncompressed fallback*: the exact sizes are compared, instead of the reference's
  entropy sampling (`ShouldCompress`) plus its "compressed > bytes + 4" rule.
- *The last insert-only command* uses cells 0 and 1 when its insert code allows. The
  reference always uses an explicit cell with copy code 4. Both are valid; ours is no larger.
- *The ratio test gives the reference a file*, not stdin, so it gets the size hint our
  one-shot `compress` has, and both pick H4 or H54 by the same rule.

Ratio against `brotli -q 4` (`scripts/gate --ratio`, ours/reference, bytes):

| entry | w10 | w15 | w19 |
|---|---|---|---|
| short-text (100) | 77/78 -1.28% | 77/78 -1.28% | 77/77 0.00% |
| text-200k | 67471/67361 +0.16% | 61096/60976 +0.20% | 61793/61602 +0.31% |
| html-500k | 110746/110840 -0.08% | 97394/97341 +0.05% | 97383/97112 +0.28% |
| css-100k | 18335/18375 -0.22% | 17072/17069 +0.02% | 17102/17101 +0.01% |
| js-200k | 29357/29378 -0.07% | 27810/27802 +0.03% | 28659/28464 +0.69% |
| json-150k | 40360/40435 -0.19% | 35271/35199 +0.20% | 35164/34936 +0.65% |
| skewed-300k | 77259/77253 +0.01% | 80356/80343 +0.02% | 80987/80912 +0.09% |
| text-5m | 1986799/1986712 0.00% | 1480578/1480592 0.00% | 1471457/1470333 +0.08% |
| total (all entries) | | | 6119439/6117128 +0.04% |

The small entries (one byte, 1 KiB of ones, 300 KB of zeros, 64 KiB random) are byte-for-byte
the reference's size.

Encoder memory (the hash table, commands, literals and the writer's codes; same inputs as
oxgate's `brotli_pool.rs` measurement, plus text):

| input | window | oxbrotli, `brotli_pool.rs` input | oxbrotli, text | `brotli` crate (pooled) |
|---|---|---|---|---|
| 1 KiB | 10 | 15 KiB | 18 KiB | about 512 KiB |
| 32 KiB | 15 | 263 KiB | 426 KiB | about 1 MiB |
| 1 MiB | 19 | 4.0 MiB | 7.0 MiB | about 23 MiB |

Spec:
- **Read first, then write down** in this plan (as a comment under B6) what the reference
  uses at quality 4. The constants named in `quality.h` decide this. As of the version read
  for this plan, the expected answer is: the "quickly" hasher (H4 for most inputs, H54 when
  the size hint is 1 MiB or more), score-based match choice with the last-distance bonus,
  and static-dictionary lookups. **Confirm each point** and implement what the source says,
  not this guess. Where you deliberately differ, note it.
- Commands: insert length, copy length and distance, turned into the 704-code alphabet with
  the implicit-distance codes (0 to 127) used whenever the distance equals the last one.
- The distance ring (§4) in the encoder must track the decoder's exactly. Pick the shortest
  code among short codes 0 to 15 when one matches.
- Dictionary matches: build the lookup from the dictionary at first use (`OnceLock`), not
  with a committed generated table and not in `build.rs`. Measure how long building it takes
  and report it. If it's over 2 ms, **stop and ask** whether to commit a generated table
  instead.
- Memory: the hash table size follows `lgwin`, as in the reference. Report the encoder's
  peak bytes for 1 KiB, 32 KiB and 1 MiB inputs at windows 10, 15 and 19 (the same table as
  `brotli_pool.rs`'s test, so they can be compared).

Tests:
- Round-trip and `brotli -d` on the corpus, at windows 10 to 24.
- `tests/ratio.rs` (`#[ignore]`, run by `scripts/gate --ratio`): for each corpus entry and
  windows 10, 15 and 19, compare our size with `brotli -q 4 -w W`'s. Print a table and fail
  if any entry is more than 1% larger.
- A unit test with a planted repeat at the largest distance the window allows, and one just
  past it (which must not be used).

Done when: the gate passes, the ratio test passes, and the report has the ratio table and
the memory table.

---

## B7 Block splitting

Goal: match the reference's ratio at quality 4.
Depends on: B6.
Read first: `c/enc/metablock.c` (the greedy builder and the "fast" path), `c/enc/
block_splitter*`, `c/enc/entropy_encode.c` (`BrotliOptimizeHuffmanCountsForRle`).
Files: `src/encode/metablock.rs`, `src/encode/histogram.rs`.

**What the reference does at quality 4** (google/brotli v1.2.1, read 2026-11-06):

- *Block splitting* (`metablock.c`, `BrotliBuildMetaBlockGreedy`; `metablock_inc.h`): one
  greedy splitter per category, fed the meta-block's symbols in order. Literals use blocks
  of at least 512 symbols and a split threshold of 400 bits, commands 1024 and 500,
  distances 512 and 100. Each time the current block reaches its target size, it is
  compared (by `BrotliBitsEntropy`: Shannon bits, at least 1 per symbol) with the last two
  block types. It becomes a new type if joining either would cost more than the threshold
  (at most 256 types). It goes back to the second-last type if that costs 20 bits less
  than joining the last one. Otherwise it joins the last type, and after two joins in a row
  the target grows by the minimum size. The first and the final block are counted as at
  least the minimum size.
- *Context modeling*: none (`MIN_QUALITY_FOR_CONTEXT_MODELING` is 5). Each literal block
  type has its own prefix code, and its 64 contexts all map to it. The context maps are
  written as `StoreTrivialContextMap` does: `RLEMAX` 5 (1 for distances), each type's
  move-to-front index followed by one zero run, and IMTF on.
- *Histogram tuning* (`BrotliOptimizeHistograms`, `entropy_encode.c`,
  `BrotliOptimizeHuffmanCountsForRle`): every literal, command and distance histogram is
  adjusted before its code is built. Runs of near-equal counts are flattened to their
  average, so that their code lengths repeat and run-length code well.
- *Block switch codes* (`brotli_bit_stream.c`, `BuildAndStoreBlockSplitCode`,
  `StoreBlockSwitch`): a type code (1 for "last + 1", 0 for "second last", else the type +
  2) and a block count code. Each has a prefix code per category, built from the whole
  split. The first block's count is written in the header.

Done differently:
- *No second try with one block per category.* Splitting and then also writing the
  meta-block unsplit, keeping the smaller, was implemented and measured. Over every split
  meta-block of the corpus, cut into 2 KB, 8 KB and 32 KB slices and whole, it saved 1,628
  bits of 20.8 million (0.008%), at the cost of writing those meta-blocks twice. It was taken
  out. A split with one type per category is the one-block writer already.
- *Histogram tuning kept, though it doesn't help us.* Without it the corpus is 106 bytes
  smaller in total (0.002%): our code-length storage always run-length codes, where the
  reference's decides per code. It is what the reference does at quality 4, so it stays.

Ratio against `brotli -q 4` with block splitting (ours/reference, bytes):

| entry | w10 | w15 | w19 |
|---|---|---|---|
| text-200k | 67477/67361 +0.17% | 60969/60976 -0.01% | 61599/61602 -0.00% |
| html-500k | 110742/110840 -0.09% | 97358/97341 +0.02% | 97124/97112 +0.01% |
| css-100k | 18338/18375 -0.20% | 17070/17069 +0.01% | 17101/17101 0.00% |
| js-200k | 29372/29378 -0.02% | 27794/27802 -0.03% | 28458/28464 -0.02% |
| json-150k | 40365/40435 -0.17% | 35190/35199 -0.03% | 34933/34936 -0.01% |
| skewed-300k | 77247/77253 -0.01% | 80349/80343 +0.01% | 80914/80912 0.00% |
| text-5m | 1986685/1986712 -0.00% | 1480598/1480592 +0.00% | 1470314/1470333 -0.00% |
| total (all entries) | | | 6116987/6117128 -0.00% |

The other entries (one byte, short text, ones, zeros, random) are as in B6.

Spec: read the source, and before writing code, record in this plan what quality 4 does
about block splitting, context modeling and histogram tuning. Then implement exactly that,
with meta-blocks sized as the reference sizes them. Keep the B5 one-block writer for inputs
where it is smaller.
Tests: the B6 ratio test with the limit tightened to 0.5%. Every corpus entry, with block
splitting on, still decodes with `brotli -d`.
Done when: the gate passes and the ratio test passes at 0.5%.
Stop and ask if: 0.5% isn't reachable without quality-5 features. Report the gap per entry.

---

## B8 Streaming

Goal: a streaming encoder for on-the-fly response compression, shaped like oxzstd's `StreamCompressor`.
Depends on: B6 (B7 may land before or after).
Read first: oxzstd's `StreamCompressor` (its `src/lib.rs`), which serves the same caller. The
caller's pattern: one compressor per response, `write` as body chunks arrive, `flush` after
every 256 KiB of input and whenever the upstream pauses, `finish`, then `reset` and reuse for
another response on the same thread. A response that declares its length gets the smallest
window that holds it, between 2^10 and 2^19.
Files: `src/lib.rs`, `src/encode/stream.rs`, `tests/stream.rs`.

Spec:
```rust
pub struct StreamCompressor { /* ... */ }
impl StreamCompressor {
    pub fn new(options: Options, size_hint: Option<u64>) -> Self;
    pub fn set_size_hint(&mut self, size_hint: Option<u64>);
    pub fn reset(&mut self);                           // ready for a new stream, tables kept
    pub fn write(&mut self, data: &[u8], out: &mut Vec<u8>);
    pub fn flush(&mut self, out: &mut Vec<u8>);        // everything written so far is decodable
    pub fn finish(&mut self, out: &mut Vec<u8>);       // ISLAST, then ready for reset
}
```
- `write` emits a meta-block once the block buffer is full. The block buffer is bounded by
  the window; document the bound.
- `flush` ends the current meta-block. If the stream isn't byte-aligned, it adds an empty
  metadata meta-block (`ISLAST = 0`, `MNIBBLES` = metadata, reserved 0, `MSKIPBYTES = 0`) and
  pads to a byte, which is how the reference flushes. Matches may still reach back across a
  flush. Two flushes in a row add nothing.
- `reset` keeps every allocation. A compressor reused for 1,000 streams of 1 KiB grows no
  further after the first one (assert on capacities through a `#[doc(hidden)] pub fn
  footprint(&self) -> usize`).
- `Options::for_size(quality, size_hint, max_lgwin)` gives the smallest window that holds
  `size_hint` bytes, between 10 and `max_lgwin` (`max_lgwin` when there is no hint), so the
  caller doesn't compute it.

Tests (`tests/stream.rs`):
- Writing in chunks of 1, 7, 4096 and 65,536 bytes gives output that decodes to the input.
- After each `flush`, the output so far decodes, by both our decoder in a streaming test
  harness and by `brotli -d` on the prefix plus a final empty last meta-block, to exactly
  the bytes written.
- `finish` without writing gives a valid empty stream. `reset` then reuse gives identical
  output to a fresh compressor.
- The footprint test above.

Done when: the gate passes.

Done 2026-11-06, in `src/encode/stream.rs`. How it works:
- One buffer holds the window and the meta-block being built. The input is cut into the
  one-shot path's 64 KiB blocks, and a block is processed once the byte after it arrives, when
  it is known not to be the last. So a stream with no flushes and the same size hint gives
  exactly `compress_with`'s output; a test checks it on the corpus. The one-shot path keeps
  working on the caller's slice, with no copy.
- After a meta-block, the bytes before its window are dropped once they are a window's worth
  (each byte moved at most about twice, however often the caller flushes). Hash table entries
  move down with them, saturating at 0. Two details keep the output the same as unflushed:
  at least one byte more than the window is kept, so an entry that saturated to 0 is out of
  reach as it was; and the drop is a multiple of 32, since a position's bits 3 and 4 pick its
  table slot. A dropped entry is never wrong in any case: matches are checked byte by byte.
- The bit writer suspends between calls, keeping fewer than 8 bits; the window header starts
  each stream as those bits.

Done differently:
- *The bound.* The buffer holds at most `2^(lgwin + 1) + 2^(1 + max(lgwin, 16)) + 2^16`
  bytes: two windows, the longest meta-block and an input block; 16 MiB at window 22, 1.1 MiB
  at 18. The reference's ring buffer is `2^(1 + max(lgwin, 16))`; ours trades the extra window
  for not moving the window at every flush. A unit test checks the bound.
- *Reservations.* A reset reserves, for streams hinted at up to 16 KiB, all the literals and
  commands they can need (a command covers at least 3 bytes), so many small streams never
  grow the compressor. Larger ones grow it as needed; the worst case up front would be 5 bytes
  per input byte.
- *`finish` leaves the compressor ready for a new stream* (as oxzstd's does), and `reset` drops
  one under way. The encoder is set up at a stream's first call, so a finish then a reset
  clears the table once.

Memory held after a stream (`footprint`), at the window `Options::for_size(4, hint, 19)` gives:

| stream | window | held |
|---|---|---|
| 1 KiB, hinted | 11 | 15 KiB |
| 16 KiB, hinted | 15 | 245 KiB |
| 32 KiB, hinted | 16 | 389 KiB |
| 1 MiB, hinted (H54 table, 4 MiB) | 19 | 5.1 MiB |
| 1 MiB, no hint | 19 | 1.8 MiB |
| 4 MiB, no hint, window 22 | 22 | 4.8 MiB |

(Measured at B8. Since the small-body follow-up after B9, a hinted table has 16 slots per byte
instead of 2: a 1 KiB stream holds 71 KiB, 16 KiB holds 629 KiB; the others are unchanged.)

Mutation check: keeping exactly the window (no extra byte), skipping the rebase, the metadata
block, the partial block at a flush, the reset, the bits carried between calls, or the 16-byte
gap in `for_size`: a test fails for each.

---

## B9 Speed

Goal: faster than the crate being replaced, and measured against C.
Depends on: B7, B8.
Files: `benches/versus_brotli.rs`, `docs/benchmarks.md`, `scripts/bench-memory.sh` (copied
from oxzstd and adapted).

Spec:
- `cargo bench --bench versus_brotli -- [qualities] [names]` prints, per corpus entry, the
  ratio, compression MB/s and decompression MB/s for oxbrotli and for the reference
  (`brotli` timed on in-memory repeats, as oxzstd's bench times `zstd -b`; if `brotli` has no
  benchmark mode, time 20 runs on a file in a RAM-backed temp dir and say so).
- The comparison with the `brotli` **crate** happens in the caller's repository, since
  oxbrotli can't depend on it.
- Speed work follows oxzstd's experience: profile first (`cargo flamegraph` or Instruments),
  `#[inline(always)]` only on bit reads and table lookups and only where a benchmark shows it
  helps, and no `unsafe`. Record every change that helps and its gain in
  `docs/benchmarks.md`.
Done when: the benchmarks are recorded, with the machine and versions.

Done 2026-11-06; the numbers are in `docs/benchmarks.md`. Done differently:
- *The reference's timing.* `brotli` has no benchmark mode, and 20 runs on a file less the
  time to start the process gave noise (2.3 ms of start against well under a millisecond of
  work). It takes many files in one run, so it is timed on 40 copies and on 20, and the
  difference over 20 is its time per input: the start cancels out. Its times include writing
  an output file, so they flatter us where it is fast, and are left out under 256 KiB.
- *No profiler installed.* macOS's `sample` showed where the time goes; within the meta-block
  writer, where inlining hides the calls, a scratch copy of the crate with timers around each
  stage did. Nothing of either is in the repository.

Changes, each measured (MB/s on the bench machine):
- Decoder: copies in fixed 16-byte chunks instead of `extend_from_within`, whose `memmove` call
  cost more than the bytes of a short copy; a short distance copies from the next multiple of
  itself of 16 or more. Decompression of html 500 KiB 400 to 583, 1 MiB 444 to 618, text
  5 MiB 291 to 418, zeros 714 to 4,925.
- Encoder, for small bodies, where writing the meta-block costs more than matching:
  `canonical` and the leaf scan skip unused symbols eight at a time (a pass over every
  symbol waits on the same counter's last update at each zero: 1.8 µs to 0.43 µs for the
  704 command symbols); the histogram tuning compares without a division and goes through
  runs already coded well in one step (3.5 to 2.0 µs for the command histogram). A 1 KiB body
  through a reused `StreamCompressor`: 52 to 67; 4 KiB: 106 to 118. Each fast version is
  checked against a plain one in a unit test.
- Tried and dropped: making `canonical` branchless (a zero length through `next[0]`). It was
  slower, 1.8 to 3.0 µs, for the same reason as above.

Small bodies, followed up after B9 (2026-11-06). HTML bodies of 1 to 16 KiB came out 0.7% to
2.5% larger than the reference's. Two causes, each measured in a scratch copy:
- *The hash table.* A hinted table had 2 slots per byte; collisions lost matches. It now has
  16 per byte, up to the reference's 512 KiB (owner's choice): 4 KiB +1.34% to +0.83%, 16 KiB
  +0.73% to +0.15%, for 2% to 7% more time and, held per compressor, 71 KiB instead of 15 at
  1 KiB, 285 instead of 61 at 4 KiB, 629 instead of 245 at 16 KiB.
- *The dictionary slots.* With the same copies, the reference picks better dictionary words.
  Its table is built by our rule (shortest word, then lowest index), except that a tuned
  bitmap (`frozen_idx`, `c/enc/dictionary_hash.c`) marks 1,117 of the 13,504 words final: no
  shorter word replaces them. With that bitmap and the full table the sizes match the
  reference's (-0.2% to +0.65%). Copying it would break rule 9. Before deriving our own,
  real responses from the owner's sites were measured (22 files, 135 B to 1.1 MB, kept in
  `bench-data/`, not committed). Compressed as served, at the window `for_size` picks, they
  already match the reference: +0.00% in total (525,598 bytes against 525,577), +0.66% at
  worst (a 12 KiB page, 22 bytes). At smaller windows the dictionary weighs more (a 15 KiB
  script at window 10: +1.07%); with the bitmap every one of those is under +0.1%. The
  generated HTML leans on the dictionary far more than real pages do, hence its +1.7%. The
  owner chose to keep our rule.
- Also: code-length codes now use run-length codes only where the reference does
  (`DecideOverRleUse`: not for alphabets of 50 or fewer, and zeros and other lengths decided
  apart), with its plain first symbol for runs of exactly 7 and 11. 6 bytes on the corpus.

`tests/ratio.rs` now also checks generated web-sized bodies (HTML, CSS, JS, JSON at 1 to
64 KiB, at the window `for_size` picks: total +0.07%, worst HTML 1 KiB +1.68%, held to 2%),
and the files in `bench-data/` as served (at the window `for_size` picks, held to 1%); the
generated corpus stays at 0.5% at windows 10, 15 and 19.

---

## Qualities 5 and 6 (B11 to B15)

Goal: `Options::quality` 5 and 6 compress as the reference does at those qualities: smaller
than quality 4, and slower. The output of quality 4 doesn't change.

**What the reference does at qualities 5 and 6** (google/brotli v1.2.1, `db24a98`, read
2026-11-06; `reference/c/enc/`). Each work order confirms its part before writing code, as
B6 did:

- *Hasher* (`quality.h`, `ChooseHasher`). Three cases:

  | case | hasher | buckets | per bucket | hashed bytes |
  |---|---|---|---|---|
  | `lgwin` 16 or less | H40 (`hash_forgetful_chain_inc.h`) | 2^15 chains in one bank of 2^16 nodes | up to 16 (q5) or 32 (q6) hops | 4 |
  | size hint 1 MiB or more and `lgwin` 19 or more | H6 (`hash_longest_match64_inc.h`) | 2^15 | the last 2^(q-1) positions | 5 (of an 8-byte read) |
  | otherwise | H5 (`hash_longest_match_inc.h`) | 2^14 | the last 2^(q-1) positions | 4 |

  The defines in `hash.h` say how the reference builds it with SIMD (H58, H68); ignore those.
- *Last distances* (all three): the four ring distances are tried first, with
  `BackwardReferenceScoreUsingLastDistance` minus `BackwardReferencePenaltyUsingLastDistance(i)`
  for i > 0. A length of 2 counts for the first two (H5, H6) or for all (H40); H40 checks
  candidates 1 to 3 against a one-byte "tiny hash" of each position first. At quality 5 and 6
  `num_last_distances_to_check` is 4, so `PrepareDistanceCache` adds nothing.
- *Bucket search* (H5, H6): newest to oldest, stopping at the first that is too far. A candidate
  must match the 4 bytes ending at the best length so far + 1 (at least 4). H6 also compares
  the first 4 bytes. Then the position is filed: `bucket[num & mask] = pos; num += 1`.
- *Static dictionary*: `SearchInStaticDictionary` with `shallow` false: **two** words per hash
  key (`kStaticDictionaryHash[key]` and `[key + 1]`), each counted as a lookup.
- *Lazy matching* (`backward_references_inc.h`): as at quality 4, but the second search starts
  from length 0, not `best.len - 1` (`MIN_QUALITY_FOR_EXTENSIVE_REFERENCE_SEARCH`).
- *Literal context modeling* (`encode.c`, `DecideOverLiteralContextModeling`, from quality 5,
  meta-blocks of 64 bytes or more; context mode always UTF-8 below quality 10):
  - If the size hint is 1 MiB or more, `ShouldUseComplexStaticContextMap` samples 64-byte
    strides every 4 KiB. It uses the 13-context `kStaticContextMapComplexUTF8` if that saves
    0.2 bits a byte or more and the entropy with contexts is at most 3 bits.
  - Otherwise `ChooseContextMap` takes a bigram histogram of the bytes' top two bits (same
    sampling). Below quality 7 the 3-context map is ruled out, so the choice is 1 context or
    2 (`kStaticContextMapSimpleUTF8`), which needs a saving of 0.2 bits a byte.
- *Block splitting* (`metablock.c`, `BrotliBuildMetaBlockGreedy` with `num_contexts` > 1): a
  `ContextBlockSplitter` with a histogram per (type, context). It has the same sizes and
  thresholds as quality 4, and at most 256 / `num_contexts` types. The literal context map is
  then `type * num_contexts + static_map[context]` (`MapStaticContexts`).
- *Writing* (`brotli_bit_stream.c`, `BrotliStoreMetaBlock`): a literal context map that isn't
  trivial is written by `EncodeContextMap`: move-to-front, zero runs (`RunLengthCodeZeros`),
  a prefix code for the result, and the IMTF bit set. Each literal is coded with the
  histogram its type and context select.
- Unchanged from quality 4: `lgblock` 16, meta-block sizes, histogram tuning
  (`BrotliOptimizeHistograms`), distance parameters, the uncompressed fallback.

Mapping of the other qualities (owner can change this; record the decision in B11): 0 to 4
compress as 4, as now; 7 to 11 compress as 6 until they have their own plans.

---

## B11 Quality levels

Goal: one switch from `Options::quality` to the encoder settings, so B12 to B14 each add a
piece behind it.
Depends on: B9.
Files: `src/lib.rs`, `src/encode/mod.rs`, `src/encode/matcher.rs`, `src/encode/stream.rs`.

Spec:
- A crate-private `Level` enum (`Q4`, `Q5`, `Q6`) from `Options::quality`: 0-4 `Q4`, 5 `Q5`,
  6-11 `Q6`. `StreamCompressor::set_options` takes the quality for the next stream as well
  as the window.
- `Encoder::new`/`reset` take the level. Until B12 lands, `Q5` and `Q6` run the quality 4
  code, so this order changes no output.
- Public docs: `Options::quality` says which qualities are implemented and what the others do.

Tests:
- The mapping, for every quality 0 to 11 and one past (12 clamps to 6).
- Quality 4 output is byte-identical to before on the corpus (compare against
  `compress(src, 4)` from before the change, saved by the test as a hash table in the test
  file, or with a scratch copy of the previous commit, as the speed work did).
- A reused `StreamCompressor` given quality 4, then 5, then 4 writes what fresh ones do.

Done when: the gate passes and quality 4 output is unchanged.

---

## B12 Chain hashers (H5, H6)

Goal: the matching of qualities 5 and 6 for windows of 17 or more.
Depends on: B11.
Read first: `c/enc/hash_longest_match_inc.h`, `c/enc/hash_longest_match64_inc.h`,
`c/enc/hash.h` (`PrepareDistanceCache`, the score functions, `SearchInStaticDictionary`),
`c/enc/backward_references_inc.h`.
Files: `src/encode/chain.rs` (new), `src/encode/matcher.rs`, `src/encode/static_dict.rs`.

Spec:
- `chain::Hasher` with `num: Vec<u16>` (one count per bucket) and `buckets: Vec<u32>`
  (2^`block_bits` positions per bucket), `bucket_bits` 14 (H5) or 15 (H6), `block_bits`
  `quality - 1`, and the hash of 4 bytes (H5: `u32 * 0x1e35a7bd >> (32 - 14)`) or of 5 bytes
  of an 8-byte read (H6: `(u64 << 24) * kHashMul64 >> (64 - 15)`). `store`, `store_range`
  and `find_longest_match` as the reference does, in that order of checks.
- The matcher calls one of the two hashers through an enum or a generic parameter, not a
  trait object. **Measure** both against quality 4 to see which keeps quality 4 as fast as
  it is now; it must not get slower by more than 1% (the B9 numbers).
- Four last distances, scored with the penalty. The ring already holds the four.
- The lazy search's second lookup starts from length 0 at quality 5 and 6.
- Dictionary, two words per key: our table has one word per hash. Give it a second, by our
  own rule (rule 9 still applies). Start with the second-shortest word of the hash, then try
  the longest, and keep whichever is smaller on the corpus and on `bench-data/`. Record both
  numbers here. Quality 4 keeps using the first word only.
- Memory: the reference's tables are fixed: H5 1 MiB at quality 5 and 2 MiB at 6, H6 2 and
  4 MiB, plus the counts. Do the same first. Then try sizing the bucket count from the size
  hint, as H4 does; report the ratio change at 1 KiB to 64 KiB. Keep it only if the ratio
  moves by less than 0.1%.

Tests:
- Unit: `store` then `find_longest_match` finds a planted repeat; a bucket that has wrapped
  keeps only its newest 2^`block_bits` positions; a candidate past `max_backward` stops the
  walk.
- Round trip and `brotli -d` on the corpus at quality 5 and 6, windows 17 to 24.
- `tests/ratio.rs` takes a quality: at 5 and 6, window 19 only for now (the smaller windows
  need B13). Fail above 1% (B14 tightens it).

Done when: the gate passes; at window 19 the ratio is within 1% of `brotli -q 5` and `-q 6`.
Stop and ask if: the dictionary's second word can't get within 1% without the reference's
table.

Notes (2026-11-06):
- *Calling the hashers.* A generic parameter: `Finder`, implemented by quality 4's `Hasher` and
  by `chain::Chain`; `process` takes the level's hasher out of the encoder and runs the loop
  monomorphized for it. Quality 4's output is byte-identical to 0.1.1's on the 90 files of
  `bench-data/` and the reference's test data, at windows 10, 16, 19, 22 and 24, hinted and
  not. Its speed, against 0.1.1 in the same binary, interleaved: -1.7% to +1.0% on small files,
  +0.3% to +0.9% on the largest (a 1.1 MB script, a 470 KiB text). The second loop costs
  code layout: with the chain path stubbed out the gap halves. Tried and dropped: copying
  the ring per position (+1.0 to +1.8%), `#[inline(never)]` per hasher (+1.0 to +1.9%).
- *The dictionary's second word.* The reference's table has two slots per hash: one for words
  of 8 bytes or more, one for words of 4 to 7 (`c/enc/encoder_dict.c`,
  `BuildDictionaryHashTable`, files words so; the static table adds the tuned bitmap). Three
  rules measured at window 19, ours against the reference's (q5; q6 within 0.05% of each):

  | second word | corpus | web bodies | `bench-data/` | worst real file |
  |---|---|---|---|---|
  | the next shortest of 8 bytes or more | +0.009% | +0.53% | +1.08% | +11.9% |
  | the longest of 8 bytes or more | +0.008% | +0.57% | +1.09% | +11.9% |
  | the shortest of 4 to 7 bytes (kept) | -0.001% | -0.12% | +0.68% | +1.4% |

  Each slot holds the shortest, then the lowest-index word of its lengths, as quality 4's.
- *Short inputs.* The corpus's 100-byte text comes out at 81 bytes against 77: the reference's
  short-word slots differ from ours (the bitmap), and in 100 bytes one word is 4%. Its
  quality 5 commands, traced through our decoder, make the same 2-byte copies from the last
  distances as ours. `tests/ratio.rs` lets an output under 1 KiB be 4 bytes over the limit at
  qualities 5 and 6 (owner's choice). The owner also confirmed the two-slot layout under
  rule 9 (it is the reference's documented filing rule, not its data) and the level mapping
  (0-4, 5, 6-11).
- *Memory.* The reference-size tables first (H5 1 MiB at quality 5, 2 MiB at 6). Then sized
  from the hint, at so many buckets per byte, against the fixed size, on the generated web
  bodies and the files of `bench-data/` up to 64 KiB, at window 19 and at `for_size`'s:
  a quarter per byte, up to +0.54% (a 16 KiB stylesheet); one per byte, at most +0.00%
  (one group -0.04%); 4 and 16 per byte, identical. Kept: one per byte, at least 256 buckets.
  A 1 KiB body's table at quality 5 is 64 KiB instead of 1 MiB.
- *Interop.* Besides the tests (the corpus at qualities 4 to 6 and every window through our
  decoder and `brotli -d`, streams in chunks and with flushes at 5 and 6), a scratch sweep:
  the 90 files of `bench-data/` and the reference's test data, at qualities 4 to 6 and
  windows 10 to 24, ours one-shot and streamed with flushes through `brotli -d`, and
  `brotli -q -w` through ours: 12,150 checks, none failed.
- *Mutations.* 17 tried. Killed by the tests: the penalty, length 2 from all four last
  distances, the search not filing its position, the dictionary shallow or off, the short
  slot off or short of 7-byte words, the bucket bits, H6 never chosen, H6's hash reading 6
  bytes, the block-end stitch off, no rebase, quality 5 run as 6. Killed only by the ratio
  test: the lazy search starting from the match in hand. Equivalent: a bucket match of 3
  (the 4-byte check already rules it out where it could win), the walk going on past the
  window (older positions are farther), rebase wrapping instead of saturating (a wrapped
  position is past `cur`, out of reach).

---

## B13 Forgetful chains (H40)

Goal: qualities 5 and 6 at windows of 16 or less: every small response compressed with
`Options::for_size`.
Depends on: B12.
Read first: `c/enc/hash_forgetful_chain_inc.h`.
Files: `src/encode/forgetful.rs` (new), `src/encode/matcher.rs`.

Spec:
- The reference's layout: `addr: [u32; 2^15]` (last position per chain, initialized to
  `0xCCCCCCCC` so new chains end after one node), `head: [u16; 2^15]`, `tiny_hash: [u8; 2^16]`
  (the low byte of each position's hash, indexed by the low 16 bits of the position), and one
  bank of 2^16 nodes (`delta: u16`, `next: u16`), filled in a ring. `max_hops` is
  `8 << (quality - 4)`: 16 at quality 5, 32 at 6.
- Deltas over 65,535 are stored as 65,535 (`CAPPED_CHAINS` 0): the walk then reaches past the
  window and stops.
- The last-distance checks use the tiny hash for candidates 1 to 3 and accept a length of 2.
- Memory: about 512 KiB per compressor, all reset per stream. Measure the reset at 1 KiB:
  the reference clears only the `addr`/`head` entries the input touches for small one-shot
  inputs (`partial_prepare_threshold`). Do the same in `reset` when the size hint is known
  and small.

Tests:
- Unit: a chain longer than `max_hops` is cut; a delta over 65,535 ends the walk; the tiny
  hash rejects a wrong last-distance candidate.
- Round trip and `brotli -d` at quality 5 and 6, windows 10 to 16.
- `tests/ratio.rs` at quality 5 and 6, windows 10 and 15, and the web-sized bodies. Fail
  above 1%.

Done when: the gate passes and the ratio is within 1% at windows 10 and 15.

Notes (2026-11-07):
- *The hasher.* `src/encode/forgetful.rs`, the reference's layout and order of checks, behind
  the same `Finder` as the others; the encoder picks it as `ChooseHasher` does: qualities 5
  and 6 at windows of 16 or less. Rebasing a stream moves the tiny hashes with their
  positions (a rotation of the 64 KiB array): they are indexed by a position's low 16 bits.
- *Ratio, generated corpus* (ours against `brotli -q 5`; quality 6 against `-q 6` alike):
  window 10 -0.78% to +0.01% (the 100-byte text +2 bytes), window 15 -0.07% to +0.01%,
  window 19 unchanged. Window 10 coming out smaller is quality 4's behaviour too.
- *Ratio, web-sized bodies* (1 to 64 KiB, at the window `for_size` picks): -0.13% in total at
  quality 5, -0.15% at 6, worst +0.41% (HTML 4 KiB). `tests/ratio.rs` holds them to 1%.
- *Ratio, `bench-data/`* as served: +0.73% in total at quality 5. Four dictionary-heavy
  HTML pages at windows 14 and 15 come out +0.7% to +1.2%. Their commands, traced through our
  decoder, match the reference's in kind (the same 2- and 3-byte copies, within 1% of its
  copies from the window), but its dictionary copies are longer: 197 copying 1,425 bytes
  against our 204 copying 1,339 (`nightsky`). That is the reference's tuned word choice
  (B9, B12), weighing more with two words looked up per position. The 1.1 MB script
  (+1.25%) is hinted at 1 MiB or more, where the reference also models literals by context
  (B14). The owner chose an interim limit: 1.5% for real files at 5 and 6 (outputs under
  1 KiB 4 bytes more), revisited after B14.
- *Reset.* Clearing the newest positions and tiny hashes, 256 KiB, cost a 135-byte body
  4.4 µs at quality 5 (8.5 µs at quality 4, 12.9 at 5), reused as oxgate's pool does.
  Tried: noting the chains a stream starts as it files them, to clear just those. Small
  bodies gained, but 9 to 20 KiB bodies lost 15% to 20% (the branch and push on every
  position). Kept: when a stream ends, if it was 4 KiB or less and never rebased, it hashes
  its positions again and empties their chains and tiny hashes, as the reference prepares a
  small input, but at the end; the next reset then skips the clear. The heads and nodes are
  never cleared: a chain's first node holds the capped delta, so no walk goes past it.
  Reused, at quality 5: 135 B 12.9 to 9.9 µs, 518 B 23.9 to 20.5, 1.4 KiB 34.2 to 31.5;
  larger bodies unchanged. A 16 KiB limit was slower from 6 KiB on (re-hashing costs more
  than the clear there).
- *Memory.* 512 KiB per compressor at these windows (newest positions 128 KiB, heads 64,
  tiny hashes 64, nodes 256), as the reference.
- *Interop.* The 90-file sweep of B12 again (qualities 4 to 6, windows 10 to 24, both ways,
  streamed with flushes too): 12,150 checks, none failed. A reused compressor at windows 10
  and 16 writes, after short, long, rebased and empty streams, what a fresh one does.
- *Mutations.* 22 tried. Killed by the tests: the tiny hash skipped or checked from the
  first distance, the penalty, deltas cut to 16 bits instead of capped, 4 hops a quality
  step instead of 8, the search not filing its position, the tiny hashes not rotated, a
  rebase leaving the newest positions, each part of the end-of-stream clean-up (positions,
  tiny hashes, one short, after a rebase, a rebase after it, never called, the reset not
  checking it), and the window that picks the hasher (17, or never). Killed by the ratio
  test: last-distance copies of 2 refused, the dictionary shallow or off. Equivalent: the
  walk going on past the window (deltas only add), a chain match of 3 (as in B12).

---

## B14 Context modeling

Goal: literal contexts, the rest of the ratio gap.
Depends on: B12 (B13 helps measure it at small windows, but isn't required).
Read first: `c/enc/encode.c` (`DecideOverLiteralContextModeling`, `ChooseContextMap`,
`ShouldUseComplexStaticContextMap`, `WriteMetaBlockInternal`), `c/enc/metablock.c`
(`ContextBlockSplitter*`, `MapStaticContexts`, `BrotliBuildMetaBlockGreedyInternal`),
`c/enc/brotli_bit_stream.c` (`EncodeContextMap`, `MoveToFrontTransform`,
`RunLengthCodeZeros`, `BrotliStoreMetaBlock`), RFC §7.
Files: `src/encode/context.rs` (new: the decision and the static maps), `src/encode/
metablock.rs`, `src/encode/mod.rs`, `src/context.rs` (reuse the decoder's UTF-8 lookup;
don't copy it).

Spec:
- The decision, per meta-block, as listed above. The three static maps are context data
  from the reference's encoder, not from the RFC: write them from the reference's comments
  (special, lf, space, punctuation, digits, upper, lower), and check each entry in a test
  that says why it has that value.
- The context block splitter: one histogram per (type, context). Per context, the same
  entropy comparisons as the plain splitter, summed over contexts. At most
  256 / `num_contexts` types.
- The context map writer, `EncodeContextMap`: move-to-front, zero runs with the largest run
  prefix that helps (at most 6), the prefix code, IMTF on. With one context it still writes
  the trivial map as now.
- Writing literals: each literal's code is chosen by `context_map[type << 6 | context]`.
  `context` comes from the two bytes before it, including across commands (copies too) and
  from the previous meta-block (the reference passes `prev_byte` and `prev_byte2`).
- Quality 4 stays with one context and unchanged output.

Tests:
- Unit: the context map writer round-trips through the decoder's reader for maps of 1 to 256
  clusters, with and without long zero runs; the decision picks 1 context for random bytes
  and 2 for UTF-8 text with many two-byte letters.
- Round trip and `brotli -d` at quality 5 and 6 on the corpus, windows 10 to 24, one shot and
  streamed with flushes (the context of a meta-block's first literals comes from before the
  flush).
- `tests/ratio.rs`: quality 5 and 6 at windows 10, 15 and 19, limit 0.5%; web-sized bodies
  2%; `bench-data/` 1%.

Done when: the gate passes and the ratio tests pass at those limits.
Stop and ask if: 0.5% isn't reachable; report the gap per entry and what it comes from.

Notes (2026-11-08):
- *The decision.* `src/encode/context.rs`: per meta-block, as the reference's
  `DecideOverLiteralContextModeling`, from 64 bytes sampled every 4096. 13 contexts for a
  stream hinted at 1 MiB or more when they save 0.2 bits per byte on the top 5 bits and leave
  at most 3; else 2 when knowing that the byte before was a lead byte saves 0.2 bits on the
  top 2; else 1. The three-context map is never chosen below quality 7, so it isn't there.
  The entropies are checked against values worked out apart from the code. The size hint
  matters: the `brotli` tool hints only when given a file (`c/tools/brotli.c`), so compare
  against it with a file, never through a pipe (through one, inputs of 1 MiB or more came
  out "1% to 5% smaller" than the reference's).
- *The static maps.* Row 0 of the 13-context map ORs the two bytes' classes, and `LUT1` puts
  lead bytes of three or four bytes in class 2: context 12 is where a continuation byte comes
  next (after any lead byte, or inside a three-byte letter), 11 where a letter is complete.
- *The splitter.* One histogram per (type, context), the entropy differences summed over the
  contexts, at most 256 / n types. The reference's per-context entropies are always replaced
  together, so one sum per type decides the same. With one context it is the old splitter.
- *Writing.* A literal's context is from the two bytes before it in the stream: after a copy,
  the copied bytes (a dictionary word cut by its transform counts as the bytes it gives,
  `Command::cut`), and at a meta-block's start the bytes before it (0 at the stream's start).
  With one context the context mode stays LSB6 and the map trivial, so quality 4 writes what
  it did: byte for byte on 96 files (corpus, `bench-data/`, the reference's test data, and
  generated Greek, Russian, Chinese and English) at 5 windows, hinted and not; three streams
  are now pinned in `tests/compress.rs`.
- *Ratio* (ours against `brotli -q 5` / `-q 6`): generated corpus -0.03% in total at both, no
  entry over 0.3% but the 100-byte text (2 to 4 bytes); a new multilingual UTF-8 entry within
  0.05%. Web-sized bodies -0.13% (q5) and -0.15% (q6), worst +0.41%. Generated Greek of 1.5 MB
  within 5 bytes of the reference at every window, English of 2 MiB within 0.06%, the 1.1 MB
  script from +1.25% to -0.38%..+0.07%. `bench-data/` as served: +0.04% in total (q5), +0.09%
  (q6); three dictionary-heavy pages at windows 14 and 15 stay over 1% (`nightsky` +1.22%,
  `zoologist` +1.14%, `weather` +1.02% at q5; two at q6 just over). The reference codes them
  with one literal context as we do: the gap is its tuned word choice (B13 notes). The owner
  kept the 1.5% limit for real files at 5 and 6 (2026-11-08).
- *Interop.* Generated UTF-8 and English at qualities 5 and 6, windows 10 to 24, one shot and
  streamed with flushes every 3 pieces, hinted and not: none failed `brotli -d`. The flush
  tests now include UTF-8 text (meta-blocks starting inside a letter) and a hinted 1.2 MB
  stream, which reach 1, 2 and 13 contexts.
- *Speed.* Quality 4 first lost 1% to 3% (small pages most): the context branch in the
  per-literal loop. The meta-block writer is now generic over whether literals are coded by
  context, each case with its own loop, and quality 4 is back within noise of before.
  Quality 5 is 1% to 4% slower where it codes by two contexts and 7% to 12% with 13 (more
  histograms to weigh per block, as the reference's); B15 measures it against the reference.
- *Mutations.* 28 tried, all killed by the tests: map entries (a row, a column, the
  two-context map), the sampling (stride spacing, a stride's first byte or two, the byte
  classes, the top 5 bits, the two-prefix grouping, 64 bytes the least), the thresholds (gain
  0.1 or 0.4, the entropy cap off or at 2), the 1 MiB hint and the hint dropped, the decision
  off, the splitter (256 types at any context count, entropies not per context, contexts
  ignored), the map writer (run prefix 5 or none, a run of exactly 128), the mode (LSB6 with
  contexts, UTF-8 at quality 4), and the literal contexts (a cut word's length, context 0
  written, the bytes before a meta-block swapped or zero). Twelve first survived the unit
  tests as written, which only used inputs far from every edge; the tests added for them
  measure exact entropies, thresholds, lengths and runs at the edges.

---

## B15 Release

Goal: qualities 5 and 6 documented and measured, then released.
Depends on: B14.
Files: `docs/benchmarks.md`, `README.md`, `src/lib.rs`, `Cargo.toml`, `benches/versus_brotli.rs`.

Spec:
- `benches/versus_brotli.rs` runs qualities 4, 5 and 6; `docs/benchmarks.md` gets their size
  and speed against the reference, and memory per compressor.
- Profile quality 5 and 6 the way B9 did. They should run within 10% of the reference's
  speed. If not, stop and report.
- README and crate docs: which qualities are implemented, and what the others do.
- Version 0.2.0 (the meaning of `Options::quality` changed), tag and release when the owner
  says so.

Done when: the gate passes and the owner has the numbers.

Notes (2026-11-08):
- *Benchmarks.* `benches/versus_brotli.rs` runs qualities 4 to 6 by default;
  `docs/benchmarks.md` has their sizes and speeds, memory per compressor and per process, and
  where quality 5's time goes. Compression at 5 and 6 runs at 100% to 117% of the reference's
  speed where it is measured (from 256 KiB), within the 10% asked; the output is within 0.07%
  of the reference's per entry from 64 KiB up.
- *Profile* (macOS `sample`, quality 5): the chain walk is 56% of a 1.1 MB script's time and
  39% of a 3 KB page's, the matcher loop 13% and 18%, the dictionary 4% and 12%; literal
  contexts and their histograms 10% on the script (13 contexts). Nothing stood out to tune
  before the release.
- *Memory.* At 5 and 6 and windows of 16 or less (bodies to 64 KiB, by `for_size`) a
  compressor holds 512 KiB of chains whatever the body's size, as the reference; quality 4
  holds 70 KiB for a 1 KiB body. A server pooling compressors for small bodies at quality 5
  pays that per compressor. Peak process memory is the reference's or less, but for 1 KiB at 5
  and 6 (2.6 MiB against 2.5).
- *Release.* Version 0.2.0: what `Options::quality` does changed (5 and 6 are their own;
  7 to 11 compress as 6). oxgate requires oxbrotli `0.1.1` by path, which 0.2.0 doesn't meet:
  its `Cargo.toml` and submodule pointer move to 0.2.0 together, when the owner releases.
