# Benchmarks

oxbrotli against the reference `brotli` tool, at quality 4 (the only quality implemented so
far). The comparison with the `brotli` crate belongs to the caller, which can depend on it.

## Machine and versions

- An M1 Mac
- rustc 1.99.1 (2026-10-13), the release profile (fat LTO, one codegen unit)
- `brotli` 1.2.1 (Homebrew), the same release as the source read for the encoder (google/brotli
  `db24a98`)
- oxbrotli at B9, 2026-11-06

## How it is measured

`cargo bench --bench versus_brotli -- [--rounds=N] [qualities] [names]`

- Each input is compressed with the window `Options::for_size` gives it (at most 22), by both.
- oxbrotli: in memory, the best of runs filling 400 ms, with the compressor and decompressor
  reused from run to run. "ox comp" is `compress_with`; "ox strm" a reused `StreamCompressor`
  (one write, then finish), as a server would compress responses.
- The reference has no benchmark mode, but takes many files in one run: it is timed on 40
  copies of the input and on 20, and the difference over 20 is its time per input, so that
  starting the process (about 2.3 ms) cancels out; the median of three. Its time includes
  writing an output file (here in the page cache), which flatters oxbrotli where the
  reference is fast: decompressing zeros, chiefly. Under 256 KiB that cost is a large part of
  the work, so its speeds are left out.
- Three rounds, oxbrotli and the reference in turn; the median of each.

## Results

Sizes in bytes; speeds in MB/s of input (compression) or output (decompression); the
percentages are oxbrotli's speed over the reference's.

| input | w | oxbrotli | brotli | size | ox comp | ox strm | br comp | comp | ox dec | br dec | dec |
|---|---|---|---|---|---|---|---|---|---|---|---|
| zeros-300k | 19 | 13 | 13 | +0.00% | 1893 | 1852 | 786 | 241% | 5070 | 481 | 1054% |
| text-200k | 18 | 61599 | 61602 | -0.00% | 85.1 | 85.5 | | | 348 | | |
| html-500k | 19 | 97124 | 97112 | +0.01% | 134 | 134 | 133 | 101% | 567 | 564 | 100% |
| css-100k | 17 | 17101 | 17101 | +0.00% | 154 | 151 | | | 594 | | |
| js-200k | 18 | 28458 | 28464 | -0.02% | 176 | 178 | | | 746 | | |
| json-150k | 18 | 34933 | 34936 | -0.01% | 112 | 114 | | | 478 | | |
| skewed-300k | 19 | 80914 | 80912 | +0.00% | 70.5 | 71.2 | 65.4 | 108% | 303 | 336 | 90% |
| text-5m | 22 | 1480482 | 1480497 | -0.00% | 101 | 102 | 99.7 | 102% | 418 | 514 | 81% |
| html-1k | 11 | 330 | 322 | +2.48% | 69.2 | 71.9 | | | 173 | | |
| html-4k | 13 | 980 | 967 | +1.34% | 118 | 123 | | | 322 | | |
| html-16k | 15 | 3462 | 3437 | +0.73% | 121 | 121 | | | 451 | | |
| html-64k | 17 | 13052 | 13044 | +0.06% | 123 | 124 | | | 520 | | |
| html-256k | 19 | 51658 | 51651 | +0.01% | 129 | 129 | 125 | 103% | 565 | 482 | 117% |
| html-1m | 21 | 196213 | 196214 | -0.00% | 149 | 149 | 152 | 97% | 611 | 655 | 93% |

In short: compression runs at the reference's speed (97% to 108%, more on runs of one byte),
decompression at 81% to 117% of it, and the output is the reference's size from 64 KiB up.
Small HTML bodies come out larger than the reference's (+0.7% to +2.5%, from 1 to 16 KiB): not
investigated yet; see the B9 notes in `docs/plan/README.md`.

## Memory

Peak resident memory (MiB) of a whole process compressing or decompressing a file in one go,
from `scripts/bench-memory.sh` (after `cargo build --release --examples`), on HTML at quality 4:

| input | w | ox comp | br comp | ox dec | br dec |
|---|---|---|---|---|---|
| html 1 KiB | 11 | 2.0 | 2.8 | 1.9 | 2.0 |
| html 64 KiB | 17 | 2.9 | 3.1 | 2.0 | 2.1 |
| html 1 MiB | 21 | 9.4 | 10.5 | 3.2 | 3.7 |
| html 8 MiB | 22 | 29.4 | 175.7 | 11.7 | 7.0 |

oxbrotli's examples hold the whole input and output; the reference streams its output through
a window, which is why it decompresses 8 MiB in less. A `StreamCompressor` holds less than one
shot: see the footprint table in the B8 notes.

## What helped

Each change was profiled first (macOS `sample`; inside the meta-block writer, where inlining
hides the calls, a scratch copy with timers around each stage) and measured after.

| change | before | after |
|---|---|---|
| Decoder: copies in fixed 16-byte chunks, not `extend_from_within` (a `memmove` call per short copy) | html 500 KiB 400, 1 MiB 444, text 5 MiB 291, zeros 714 | 583, 618, 418, 4,925 |
| Encoder: `canonical` and the leaf scan skip unused symbols eight at a time | `canonical` over 704 symbols 1.8 µs | 0.43 µs |
| Encoder: histogram tuning without a division, and through runs already coded well in one step | command histogram 3.5 µs | 2.0 µs |
| The two above, end to end: a 1 KiB body through a reused `StreamCompressor` | 52 MB/s | 67 MB/s |
| ... a 4 KiB body | 106 MB/s | 118 MB/s |

Tried and dropped: a branchless `canonical` (zero lengths counted in `next[0]`): 1.8 µs to
3.0 µs, since each zero then waits on the last update of the same counter.

Where the time goes now, for a 1 KiB body (15 µs in all): matching 4.2 µs, building the
prefix codes 3.2, histogram tuning 1.8, block splitting 1.8, writing the codes 2.2, writing the
data 1.3. From 4 KiB up, matching is most of it.
