# Benchmarks

oxbrotli against the reference `brotli` tool, at qualities 4, 5 and 6 (the ones with encoders
of their own: 0 to 3 compress as 4, and 7 to 11 as 6). The comparison with the `brotli` crate
belongs to the caller, which can depend on it.

## Machine and versions

- An M1 Mac
- rustc 1.99.1 (2026-10-13), the release profile (fat LTO, one codegen unit)
- `brotli` 1.2.1 (Homebrew), the same release as the source read for the encoder (google/brotli
  `db24a98`)
- oxbrotli at B15, 2026-11-08

## How it is measured

`cargo bench --bench versus_brotli -- [--rounds=N] [qualities] [names]` (qualities 4 to 6 by
default)

- Each input is compressed with the window `Options::for_size` gives it (at most 22), by both,
  and told its size: the reference by being given the file, oxbrotli by its size hint. From
  quality 5 the size matters: inputs of 1 MiB or more get a larger hash table at windows from
  19, and may code literals in 13 contexts.
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

| input | q | w | oxbrotli | brotli | size | ox comp | ox strm | br comp | comp | ox dec | br dec | dec |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| zeros-300k | 4 | 19 | 13 | 13 | +0.00% | 16216 | 13091 | 833 | 1946% | 5132 | 483 | 1063% |
| zeros-300k | 5 | 19 | 13 | 13 | +0.00% | 8541 | 9691 | 846 | 1010% | 5132 | 489 | 1050% |
| zeros-300k | 6 | 19 | 13 | 13 | +0.00% | 6704 | 9691 | 851 | 788% | 5132 | 488 | 1052% |
| text-200k | 4 | 18 | 61599 | 61602 | -0.00% | 92.7 | 92.9 |  |  | 349 |  |  |
| text-200k | 5 | 18 | 58616 | 58615 | +0.00% | 59.2 | 59.3 |  |  | 357 |  |  |
| text-200k | 6 | 18 | 56744 | 56739 | +0.01% | 47.1 | 47.3 |  |  | 381 |  |  |
| utf8-text-200k | 4 | 18 | 37921 | 37915 | +0.02% | 157 | 157 |  |  | 558 |  |  |
| utf8-text-200k | 5 | 18 | 33410 | 33406 | +0.01% | 97.0 | 97.3 |  |  | 631 |  |  |
| utf8-text-200k | 6 | 18 | 31436 | 31433 | +0.01% | 80.4 | 81.0 |  |  | 707 |  |  |
| html-500k | 4 | 19 | 97121 | 97112 | +0.01% | 145 | 145 | 139 | 104% | 583 | 585 | 100% |
| html-500k | 5 | 19 | 93669 | 93667 | +0.00% | 94.7 | 94.5 | 86.3 | 110% | 585 | 590 | 99% |
| html-500k | 6 | 19 | 91801 | 91787 | +0.02% | 77.2 | 77.4 | 69.9 | 110% | 610 | 606 | 101% |
| css-100k | 4 | 17 | 17101 | 17101 | +0.00% | 162 | 162 |  |  | 605 |  |  |
| css-100k | 5 | 17 | 14715 | 14723 | -0.05% | 111 | 111 |  |  | 712 |  |  |
| css-100k | 6 | 17 | 13995 | 14005 | -0.07% | 92.5 | 93.6 |  |  | 768 |  |  |
| js-200k | 4 | 18 | 28458 | 28464 | -0.02% | 197 | 196 |  |  | 774 |  |  |
| js-200k | 5 | 18 | 26328 | 26340 | -0.05% | 107 | 107 |  |  | 737 |  |  |
| js-200k | 6 | 18 | 26010 | 26024 | -0.05% | 92.4 | 92.9 |  |  | 753 |  |  |
| json-150k | 4 | 18 | 34933 | 34936 | -0.01% | 123 | 123 |  |  | 487 |  |  |
| json-150k | 5 | 18 | 33716 | 33730 | -0.04% | 77.7 | 78.0 |  |  | 486 |  |  |
| json-150k | 6 | 18 | 33148 | 33158 | -0.03% | 68.7 | 69.0 |  |  | 501 |  |  |
| skewed-300k | 4 | 19 | 80914 | 80912 | +0.00% | 75.2 | 75.3 | 67.3 | 112% | 311 | 345 | 90% |
| skewed-300k | 5 | 19 | 77629 | 77628 | +0.00% | 43.9 | 44.0 | 38.5 | 114% | 322 | 353 | 91% |
| skewed-300k | 6 | 19 | 76454 | 76454 | +0.00% | 36.1 | 36.2 | 30.9 | 117% | 353 | 380 | 93% |
| text-5m | 4 | 22 | 1480481 | 1480497 | -0.00% | 112 | 113 | 102 | 110% | 431 | 539 | 80% |
| text-5m | 5 | 22 | 1460050 | 1460022 | +0.00% | 60.4 | 60.9 | 56.8 | 106% | 407 | 509 | 80% |
| text-5m | 6 | 22 | 1425604 | 1425615 | -0.00% | 46.8 | 47.2 | 43.3 | 108% | 434 | 537 | 81% |
| html-1k | 4 | 11 | 329 | 322 | +2.17% | 70.8 | 72.7 |  |  | 177 |  |  |
| html-1k | 5 | 11 | 307 | 305 | +0.66% | 45.6 | 63.7 |  |  | 177 |  |  |
| html-1k | 6 | 11 | 307 | 305 | +0.66% | 45.6 | 63.7 |  |  | 177 |  |  |
| html-4k | 4 | 13 | 975 | 967 | +0.83% | 127 | 131 |  |  | 326 |  |  |
| html-4k | 5 | 13 | 911 | 913 | -0.22% | 86.5 | 101 |  |  | 349 |  |  |
| html-4k | 6 | 13 | 910 | 912 | -0.22% | 84.2 | 99.5 |  |  | 351 |  |  |
| html-16k | 4 | 15 | 3442 | 3437 | +0.15% | 135 | 138 |  |  | 451 |  |  |
| html-16k | 5 | 15 | 3263 | 3268 | -0.15% | 81.6 | 84.9 |  |  | 468 |  |  |
| html-16k | 6 | 15 | 3255 | 3261 | -0.18% | 72.3 | 73.6 |  |  | 477 |  |  |
| html-64k | 4 | 17 | 13053 | 13044 | +0.07% | 134 | 134 |  |  | 526 |  |  |
| html-64k | 5 | 17 | 12507 | 12508 | -0.01% | 95.7 | 94.9 |  |  | 547 |  |  |
| html-64k | 6 | 17 | 12338 | 12340 | -0.02% | 83.0 | 84.3 |  |  | 561 |  |  |
| html-256k | 4 | 19 | 51658 | 51651 | +0.01% | 141 | 141 | 129 | 110% | 566 | 505 | 112% |
| html-256k | 5 | 19 | 49692 | 49691 | +0.00% | 93.7 | 93.8 | 82.8 | 113% | 570 | 500 | 114% |
| html-256k | 6 | 19 | 48820 | 48822 | -0.00% | 77.8 | 78.0 | 67.2 | 116% | 595 | 516 | 115% |
| html-1m | 4 | 21 | 196214 | 196214 | +0.00% | 164 | 164 | 156 | 105% | 630 | 674 | 94% |
| html-1m | 5 | 21 | 188741 | 188725 | +0.01% | 93.5 | 93.5 | 93.8 | 100% | 628 | 647 | 97% |
| html-1m | 6 | 21 | 185823 | 185815 | +0.00% | 77.9 | 77.8 | 75.5 | 103% | 645 | 670 | 96% |

In short:

- **Size:** the reference's at every quality, at most 0.07% larger per entry from 64 KiB up
  (and up to 0.07% smaller at 5 and 6). Quality 5 is 1% to 14% smaller than 4: 1.4% on 5 MiB
  of text, 3% to 8% on text, markup and code, 12% on UTF-8 text (two literal contexts), 14% on
  CSS; 6 is another 1% to 6% smaller.
- **Compression speed:** 100% to 117% of the reference's at 5 and 6 where measured (104% to
  112% at 4, more on runs of one byte). Quality 5 runs at 55% to 70% of quality 4's speed, and
  6 at 42% to 57%.
- **Decompression:** 80% to 115% of the reference's, as at quality 4. Streams from 5 and 6
  decode within -6% to +27% of quality 4's speed.
- **Small bodies:** a 1 KiB body comes out 2 bytes larger than the reference's at 5 and 6
  (+0.7%), and 4 KiB to 64 KiB slightly smaller (-0.01% to -0.22%). Through a reused
  `StreamCompressor` a 1 KiB body compresses at 64 MB/s at 5 and 6, against 46 MB/s one shot:
  one shot sets up its hash table each time (512 KiB at windows of 16 or less).

## Memory

Bytes a `StreamCompressor` holds after one stream (`footprint`): its hash tables and the input
it keeps, on HTML, told its size:

| input | w | q4 | q5 | q6 |
|---|---|---|---|---|
| html 1 KiB | 11 | 0.07 MiB | 0.51 MiB | 0.51 MiB |
| html 64 KiB | 17 | 0.68 MiB | 1.21 MiB | 2.21 MiB |
| html 1 MiB | 21 | 6.73 MiB | 4.76 MiB | 6.76 MiB |
| html 8 MiB | 22 | 25.6 MiB | 23.5 MiB | 25.5 MiB |

Quality 4's table is sized to the hint (up to 4 MiB for 1 MiB or more). Qualities 5 and 6 keep
the last 16 or 32 positions per hash bucket: at windows of 16 or less a fixed 512 KiB of
chains, as the reference's (so every small body pays it); above, a table sized to the hint, up
to 1 MiB (q5) or 2 MiB (q6), or 2 or 4 MiB for 1 MiB or more at windows from 19.

Peak resident memory (MiB) of a whole process compressing or decompressing a file in one go,
from `[QUALITY=q] scripts/bench-memory.sh` (after `cargo build --release --examples`), on
HTML:

| input | w | q | ox comp | br comp | ox dec | br dec |
|---|---|---|---|---|---|---|
| html 1 KiB | 11 | 4 | 2.2 | 2.8 | 1.9 | 2.0 |
| | | 5 | 2.6 | 2.5 | 1.9 | 2.1 |
| | | 6 | 2.6 | 2.5 | 1.9 | 2.0 |
| html 64 KiB | 17 | 4 | 2.9 | 3.0 | 2.0 | 2.1 |
| | | 5 | 3.4 | 3.9 | 2.0 | 2.2 |
| | | 6 | 4.5 | 5.1 | 2.0 | 2.2 |
| html 1 MiB | 21 | 4 | 8.8 | 10.4 | 3.2 | 3.6 |
| | | 5 | 7.3 | 9.1 | 3.3 | 3.7 |
| | | 6 | 9.4 | 12.0 | 3.3 | 3.8 |
| html 8 MiB | 22 | 4 | 29.2 | 120.9 | 11.5 | 7.0 |
| | | 5 | 28.2 | 152.4 | 11.6 | 7.1 |
| | | 6 | 29.2 | 126.1 | 11.6 | 7.1 |

oxbrotli's examples hold the whole input and output; the reference streams its output through
a window, which is why it decompresses 8 MiB in less. A `StreamCompressor` holds less than one
shot: see the footprint table in the B8 notes.

## Where the time goes at quality 5

macOS `sample` of a reused `StreamCompressor` at quality 5, by stage:

| stage | 1.1 MB script (w21, 13 contexts) | 3 KB page (w12) |
|---|---|---|
| match search (hash chains) | 56% | 39% |
| matcher loop (scoring, commands) | 13% | 18% |
| dictionary lookup | 4% | 12% |
| planning (literal contexts, histograms) | 10% | 7% |
| writing the meta-block | 9% | 8% |
| building and storing prefix codes | 4% | 9% |
| block splitting | 3% | 1% |
| memory (copies, clears, allocation) | 2% | 5% |

The chain walk is most of it, as in the reference. Coding literals by context costs where it
is used: quality 5 runs 1% to 4% slower on text it codes with two contexts, and 7% to 12% with
13 (more histograms to weigh per block). Quality 4 has its own copy of the per-literal loop, so
contexts cost it nothing.

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
