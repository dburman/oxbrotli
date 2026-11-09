# Benchmarks

oxbrotli against the reference `brotli` tool, at qualities 4, 5 and 6 (the ones with encoders
of their own: 0 to 3 compress as 4, and 7 to 11 as 6). The comparison with the `brotli` crate
belongs to the caller, which can depend on it.

## Machine and versions

- An M1 Mac
- rustc 1.99.1 (2026-10-13), the release profile (fat LTO, one codegen unit)
- `brotli` 1.2.1 (Homebrew), the same release as the source read for the encoder (google/brotli
  `db24a98`)
- oxbrotli 0.2.1, 2026-11-09

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
- Three rounds, oxbrotli and the reference in turn; the median of each. From run to run a
  speed moves by up to 2% or so.
- Two sets of inputs: the generated corpus of the tests, and 22 real responses of the owner's
  sites (`bench-data/`, kept local and out of the repository), named here by type and size.

## Results

Sizes in bytes; speeds in MB/s of input (compression) or output (decompression); the
percentages are oxbrotli's speed over the reference's.

| input | q | w | oxbrotli | brotli | size | ox comp | ox strm | br comp | comp | ox dec | br dec | dec |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| zeros-300k | 4 | 19 | 13 | 13 | +0.00% | 15894 | 12812 | 825 | 1927% | 5125 | 472 | 1085% |
| zeros-300k | 5 | 19 | 13 | 13 | +0.00% | 8228 | 9537 | 840 | 980% | 5125 | 487 | 1053% |
| zeros-300k | 6 | 19 | 13 | 13 | +0.00% | 6475 | 9524 | 797 | 813% | 5125 | 470 | 1091% |
| text-200k | 4 | 18 | 61599 | 61602 | -0.00% | 94.4 | 94.8 |  |  | 349 |  |  |
| text-200k | 5 | 18 | 58616 | 58615 | +0.00% | 62.2 | 61.8 |  |  | 357 |  |  |
| text-200k | 6 | 18 | 56744 | 56739 | +0.01% | 49.4 | 49.5 |  |  | 380 |  |  |
| utf8-text-200k | 4 | 18 | 37921 | 37915 | +0.02% | 161 | 160 |  |  | 557 |  |  |
| utf8-text-200k | 5 | 18 | 33410 | 33406 | +0.01% | 101 | 102 |  |  | 630 |  |  |
| utf8-text-200k | 6 | 18 | 31436 | 31433 | +0.01% | 84.4 | 84.7 |  |  | 705 |  |  |
| html-500k | 4 | 19 | 97121 | 97112 | +0.01% | 148 | 147 | 131 | 113% | 582 | 557 | 105% |
| html-500k | 5 | 19 | 93669 | 93667 | +0.00% | 98.3 | 98.3 | 81.7 | 120% | 584 | 560 | 104% |
| html-500k | 6 | 19 | 91801 | 91787 | +0.02% | 79.9 | 80.0 | 65.3 | 122% | 607 | 584 | 104% |
| css-100k | 4 | 17 | 17101 | 17101 | +0.00% | 165 | 164 |  |  | 608 |  |  |
| css-100k | 5 | 17 | 14715 | 14723 | -0.05% | 115 | 115 |  |  | 717 |  |  |
| css-100k | 6 | 17 | 13995 | 14005 | -0.07% | 95.9 | 97.2 |  |  | 769 |  |  |
| js-200k | 4 | 18 | 28458 | 28464 | -0.02% | 199 | 199 |  |  | 772 |  |  |
| js-200k | 5 | 18 | 26328 | 26340 | -0.05% | 112 | 113 |  |  | 733 |  |  |
| js-200k | 6 | 18 | 26010 | 26024 | -0.05% | 95.3 | 96.4 |  |  | 753 |  |  |
| json-150k | 4 | 18 | 34933 | 34936 | -0.01% | 123 | 123 |  |  | 485 |  |  |
| json-150k | 5 | 18 | 33716 | 33730 | -0.04% | 83.7 | 83.3 |  |  | 482 |  |  |
| json-150k | 6 | 18 | 33148 | 33158 | -0.03% | 73.1 | 73.3 |  |  | 499 |  |  |
| skewed-300k | 4 | 19 | 80914 | 80912 | +0.00% | 76.8 | 76.9 | 65.5 | 117% | 311 | 323 | 96% |
| skewed-300k | 5 | 19 | 77629 | 77628 | +0.00% | 44.8 | 44.7 | 36.6 | 122% | 322 | 313 | 103% |
| skewed-300k | 6 | 19 | 76454 | 76454 | +0.00% | 36.2 | 36.5 | 29.5 | 123% | 353 | 336 | 105% |
| text-5m | 4 | 22 | 1480481 | 1480497 | -0.00% | 110 | 112 | 100 | 110% | 421 | 514 | 82% |
| text-5m | 5 | 22 | 1460050 | 1460022 | +0.00% | 63.9 | 64.6 | 55.7 | 115% | 400 | 483 | 83% |
| text-5m | 6 | 22 | 1425604 | 1425615 | -0.00% | 49.4 | 49.5 | 43.1 | 115% | 425 | 511 | 83% |
| html-1k | 4 | 11 | 329 | 322 | +2.17% | 69.4 | 70.6 |  |  | 177 |  |  |
| html-1k | 5 | 11 | 307 | 305 | +0.66% | 46.2 | 63.2 |  |  | 182 |  |  |
| html-1k | 6 | 11 | 307 | 305 | +0.66% | 46.2 | 63.0 |  |  | 181 |  |  |
| html-4k | 4 | 13 | 975 | 967 | +0.83% | 129 | 128 |  |  | 337 |  |  |
| html-4k | 5 | 13 | 911 | 913 | -0.22% | 90.4 | 105 |  |  | 360 |  |  |
| html-4k | 6 | 13 | 910 | 912 | -0.22% | 88.9 | 102 |  |  | 360 |  |  |
| html-16k | 4 | 15 | 3442 | 3437 | +0.15% | 142 | 142 |  |  | 470 |  |  |
| html-16k | 5 | 15 | 3263 | 3268 | -0.15% | 88.0 | 90.3 |  |  | 480 |  |  |
| html-16k | 6 | 15 | 3255 | 3261 | -0.18% | 77.1 | 78.8 |  |  | 484 |  |  |
| html-64k | 4 | 17 | 13053 | 13044 | +0.07% | 140 | 140 |  |  | 545 |  |  |
| html-64k | 5 | 17 | 12507 | 12508 | -0.01% | 101 | 101 |  |  | 549 |  |  |
| html-64k | 6 | 17 | 12338 | 12340 | -0.02% | 86.5 | 87.7 |  |  | 564 |  |  |
| html-256k | 4 | 19 | 51658 | 51651 | +0.01% | 145 | 143 | 121 | 119% | 566 | 476 | 119% |
| html-256k | 5 | 19 | 49692 | 49691 | +0.00% | 98.3 | 99.0 | 78.6 | 125% | 570 | 477 | 119% |
| html-256k | 6 | 19 | 48820 | 48822 | -0.00% | 81.3 | 82.0 | 64.3 | 126% | 595 | 504 | 118% |
| html-1m | 4 | 21 | 196214 | 196214 | +0.00% | 168 | 167 | 148 | 113% | 631 | 644 | 98% |
| html-1m | 5 | 21 | 188741 | 188725 | +0.01% | 99.2 | 100 | 90.4 | 110% | 627 | 629 | 100% |
| html-1m | 6 | 21 | 185823 | 185815 | +0.00% | 81.8 | 81.7 | 74.5 | 110% | 639 | 649 | 98% |

The real responses, smallest first:

| response | q | w | oxbrotli | brotli | size | ox comp | ox strm | br comp | comp | ox dec | br dec | dec |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| json 135 B | 4 | 10 | 109 | 109 | +0.00% | 20.1 | 20.6 |  |  | 47.7 |  |  |
| json 135 B | 5 | 10 | 103 | 101 | +1.98% | 10.1 | 17.4 |  |  | 46.3 |  |  |
| json 135 B | 6 | 10 | 103 | 101 | +1.98% | 10.1 | 17.5 |  |  | 45.6 |  |  |
| svg 518 B | 4 | 10 | 313 | 315 | -0.63% | 40.2 | 41.2 |  |  | 92.1 |  |  |
| svg 518 B | 5 | 10 | 288 | 293 | -1.71% | 24.4 | 33.9 |  |  | 93.5 |  |  |
| svg 518 B | 6 | 10 | 288 | 293 | -1.71% | 24.4 | 33.8 |  |  | 93.5 |  |  |
| html 1.4 KB | 4 | 11 | 512 | 517 | -0.97% | 72.4 | 72.9 |  |  | 171 |  |  |
| html 1.4 KB | 5 | 11 | 496 | 503 | -1.39% | 49.5 | 62.3 |  |  | 181 |  |  |
| html 1.4 KB | 6 | 11 | 496 | 503 | -1.39% | 49.6 | 62.4 |  |  | 181 |  |  |
| js 3.8 KB | 4 | 12 | 1562 | 1562 | +0.00% | 86.4 | 86.0 |  |  | 201 |  |  |
| js 3.8 KB | 5 | 12 | 1479 | 1478 | +0.07% | 63.0 | 69.3 |  |  | 221 |  |  |
| js 3.8 KB | 6 | 12 | 1479 | 1478 | +0.07% | 62.8 | 69.8 |  |  | 221 |  |  |
| css 6.3 KB | 4 | 13 | 1535 | 1529 | +0.39% | 139 | 140 |  |  | 357 |  |  |
| css 6.3 KB | 5 | 13 | 1401 | 1401 | +0.00% | 93.6 | 99.6 |  |  | 387 |  |  |
| css 6.3 KB | 6 | 13 | 1389 | 1389 | +0.00% | 85.5 | 91.2 |  |  | 394 |  |  |
| html 9.4 KB | 4 | 14 | 2662 | 2658 | +0.15% | 119 | 118 |  |  | 348 |  |  |
| html 9.4 KB | 5 | 14 | 2474 | 2449 | +1.02% | 77.2 | 79.5 |  |  | 352 |  |  |
| html 9.4 KB | 6 | 14 | 2467 | 2444 | +0.94% | 72.8 | 76.5 |  |  | 357 |  |  |
| html 11.8 KB | 4 | 14 | 3349 | 3327 | +0.66% | 111 | 112 |  |  | 338 |  |  |
| html 11.8 KB | 5 | 14 | 3079 | 3042 | +1.22% | 76.9 | 80.4 |  |  | 365 |  |  |
| html 11.8 KB | 6 | 14 | 3063 | 3032 | +1.02% | 71.4 | 73.2 |  |  | 368 |  |  |
| js 15.5 KB | 4 | 14 | 4831 | 4814 | +0.35% | 100 | 98.6 |  |  | 322 |  |  |
| js 15.5 KB | 5 | 14 | 4492 | 4475 | +0.38% | 66.8 | 67.3 |  |  | 348 |  |  |
| js 15.5 KB | 6 | 14 | 4471 | 4459 | +0.27% | 61.5 | 62.6 |  |  | 352 |  |  |
| html 15.8 KB | 4 | 14 | 4093 | 4092 | +0.02% | 123 | 124 |  |  | 383 |  |  |
| html 15.8 KB | 5 | 14 | 3825 | 3782 | +1.14% | 76.9 | 78.0 |  |  | 399 |  |  |
| html 15.8 KB | 6 | 14 | 3811 | 3772 | +1.03% | 72.1 | 73.3 |  |  | 401 |  |  |
| js 16.2 KB | 4 | 14 | 5363 | 5357 | +0.11% | 98.0 | 98.6 |  |  | 312 |  |  |
| js 16.2 KB | 5 | 14 | 5047 | 5038 | +0.18% | 61.8 | 62.6 |  |  | 335 |  |  |
| js 16.2 KB | 6 | 14 | 5049 | 5040 | +0.18% | 59.3 | 61.5 |  |  | 338 |  |  |
| html 20.6 KB | 4 | 15 | 6386 | 6386 | +0.00% | 104 | 106 |  |  | 317 |  |  |
| html 20.6 KB | 5 | 15 | 6033 | 6019 | +0.23% | 65.5 | 67.3 |  |  | 334 |  |  |
| html 20.6 KB | 6 | 15 | 6026 | 6016 | +0.17% | 59.1 | 60.4 |  |  | 334 |  |  |
| html 20.7 KB | 4 | 15 | 4780 | 4778 | +0.04% | 135 | 134 |  |  | 437 |  |  |
| html 20.7 KB | 5 | 15 | 4435 | 4404 | +0.70% | 84.3 | 86.2 |  |  | 456 |  |  |
| html 20.7 KB | 6 | 15 | 4427 | 4401 | +0.59% | 77.6 | 79.4 |  |  | 457 |  |  |
| css 25.6 KB | 4 | 15 | 5417 | 5420 | -0.06% | 123 | 124 |  |  | 437 |  |  |
| css 25.6 KB | 5 | 15 | 5059 | 5050 | +0.18% | 81.8 | 83.1 |  |  | 479 |  |  |
| css 25.6 KB | 6 | 15 | 5054 | 5044 | +0.20% | 72.9 | 73.9 |  |  | 487 |  |  |
| js 28.9 KB | 4 | 15 | 11606 | 11609 | -0.03% | 76.9 | 77.3 |  |  | 237 |  |  |
| js 28.9 KB | 5 | 15 | 10959 | 10934 | +0.23% | 49.6 | 49.5 |  |  | 240 |  |  |
| js 28.9 KB | 6 | 15 | 10956 | 10921 | +0.32% | 47.1 | 47.0 |  |  | 238 |  |  |
| js 37.7 KB | 4 | 16 | 10563 | 10543 | +0.19% | 108 | 107 |  |  | 356 |  |  |
| js 37.7 KB | 5 | 16 | 9868 | 9856 | +0.12% | 68.9 | 69.2 |  |  | 380 |  |  |
| js 37.7 KB | 6 | 16 | 9847 | 9835 | +0.12% | 62.1 | 62.6 |  |  | 378 |  |  |
| js 46.9 KB | 4 | 16 | 14862 | 14871 | -0.06% | 93.3 | 94.0 |  |  | 303 |  |  |
| js 46.9 KB | 5 | 16 | 13903 | 13924 | -0.15% | 59.3 | 59.1 |  |  | 316 |  |  |
| js 46.9 KB | 6 | 16 | 13845 | 13863 | -0.13% | 52.9 | 52.9 |  |  | 314 |  |  |
| html 49.9 KB | 4 | 16 | 36093 | 36107 | -0.04% | 221 | 223 |  |  | 145 |  |  |
| html 49.9 KB | 5 | 16 | 36168 | 36137 | +0.09% | 171 | 174 |  |  | 143 |  |  |
| html 49.9 KB | 6 | 16 | 36148 | 36135 | +0.04% | 173 | 177 |  |  | 145 |  |  |
| html 73.8 KB | 4 | 17 | 13254 | 13246 | +0.06% | 147 | 148 |  |  | 513 |  |  |
| html 73.8 KB | 5 | 17 | 12203 | 12183 | +0.16% | 105 | 106 |  |  | 528 |  |  |
| html 73.8 KB | 6 | 17 | 12155 | 12130 | +0.21% | 96.8 | 97.8 |  |  | 525 |  |  |
| js 134 KB | 4 | 18 | 32814 | 32838 | -0.07% | 113 | 113 |  |  | 396 |  |  |
| js 134 KB | 5 | 18 | 30632 | 30641 | -0.03% | 79.9 | 79.9 |  |  | 414 |  |  |
| js 134 KB | 6 | 18 | 30393 | 30405 | -0.04% | 73.3 | 73.5 |  |  | 428 |  |  |
| js 206 KB | 4 | 18 | 70851 | 70868 | -0.02% | 84.1 | 83.9 |  |  | 287 |  |  |
| js 206 KB | 5 | 18 | 66627 | 66592 | +0.05% | 56.8 | 56.7 |  |  | 300 |  |  |
| js 206 KB | 6 | 18 | 65894 | 65894 | +0.00% | 51.4 | 51.0 |  |  | 304 |  |  |
| html 341 KB | 4 | 19 | 9341 | 9332 | +0.10% | 623 | 615 | 444 | 140% | 3094 | 1434 | 216% |
| html 341 KB | 5 | 19 | 8623 | 8589 | +0.40% | 391 | 389 | 307 | 127% | 3192 | 1403 | 228% |
| html 341 KB | 6 | 19 | 8582 | 8547 | +0.41% | 362 | 366 | 280 | 129% | 3212 | 1396 | 230% |
| js 1.1 MB | 4 | 21 | 285302 | 285299 | +0.00% | 129 | 129 | 122 | 106% | 381 | 475 | 80% |
| js 1.1 MB | 5 | 21 | 259930 | 260053 | -0.05% | 67.4 | 67.3 | 68.6 | 98% | 374 | 431 | 87% |
| js 1.1 MB | 6 | 21 | 256833 | 256647 | +0.07% | 59.6 | 59.8 | 62.0 | 96% | 381 | 417 | 91% |

In short:

- **Size:** the reference's at every quality. On the generated corpus at most 0.07% larger per
  entry from 64 KiB up (and up to 0.07% smaller at 5 and 6). On the real responses, in all,
  +0.00% at 4, +0.04% at 5 and +0.09% at 6; per response at most 0.7% larger from 16 KiB up,
  and 1.0% to 2.0% on some under 16 KiB at 5 and 6 (2 to 43 bytes, from the dictionary words
  chosen). Quality 5 is 1% to 14% smaller than 4: 1.4% on 5 MiB of text, 3% to 8% on text,
  markup and code (9% on the 1.1 MB script), 12% on UTF-8 text (two literal contexts), 14% on
  CSS; 6 is another 1% to 6% smaller.
- **Compression speed,** where the reference's is measured (256 KiB and up):
  - Generated inputs: 110% to 119% of the reference's at 4, 110% to 125% at 5 and 110% to
    126% at 6 (more on runs of one byte).
  - The 341 KB real page: 140%, 127% and 129%.
  - The 1.1 MB real script: 106% at 4, but 98% at 5 and 96% at 6 (98% to 100% and 96% to 98%
    over three runs). It is the one input here coded with 13 literal contexts, and the one
    where the reference is still a little ahead: see "Where the time goes".
  - Against quality 4: quality 5 runs at 56% to 72% of its speed, and 6 at 45% to 69%.
- **Decompression:** 82% to 119% of the reference's on generated inputs, 80% to 91% on the
  1.1 MB script and over twice its speed on the 341 KB page. Streams from 5 and 6 decode within
  -5% to +27% of quality 4's speed.
- **Small bodies:** a 1 KiB body comes out 2 bytes larger than the reference's at 5 and 6
  (+0.7%), and 4 KiB to 64 KiB slightly smaller (-0.01% to -0.22%). Through a reused
  `StreamCompressor` a 1 KiB body compresses at 63 MB/s at 5 and 6, against 46 MB/s one shot:
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

Measured at 0.2.0; 0.2.1 changed no table.

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

macOS `sample` of a reused `StreamCompressor` at quality 5, by stage (at 0.2.0):

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

### The 1.1 MB script against the reference (0.2.1)

At 0.2.0 the 1.1 MB script compressed at 91% to 93% of the reference's speed at 5 and 6, while
generated inputs were at 100% to 117%. Reading the bucket early (below) brought it to about
96% at 6. Profiled then at quality 6, both of them on that script (macOS `sample`; the
reference is Homebrew's 1.2.1, file reading left out):

| | match search | the rest (contexts, splitting, codes, writing) |
|---|---|---|
| reference | 76% | 24% |
| oxbrotli | 70% | 30% |

Taking the totals into account (oxbrotli took about 1/0.96 of the reference's time), oxbrotli's
match search is a little faster; the difference is spread over the meta-block side,
where 13 contexts make the most work: assigning each literal its context (3%), splitting into
blocks (7%, of which 3.5% weighing the 13 histograms per block), and building and writing the
codes (12%). Each is close to the reference's own share; none is much slower.

On 64-bit machines the reference doesn't use H5 and H6 at 5 and 6 but H58 and H68
(`BROTLI_MAX_SIMD_QUALITY`): the same chains, with an 8-bit tag per position (more bits of the
same hash), compared 16 at a time with SIMD so that only positions whose tag matches are read.
The tag is more of the same hash, so H58 finds the matches H5 does, and H68 those of H6 but for
4-byte matches whose 5-byte hash only collided (none on the 1.1 MB script: the same output). Without SIMD, tags cost
oxbrotli more than they save (see "Tried and dropped").

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
| Encoder, 0.2.1: the chain search reads the bucket's cache lines before trying the last distances (the reference prefetches them), and is inlined into the matcher loop | 1.1 MB script, compressing in a loop: 60.5 MB/s at 5, 53.7 at 6 | 65.1, 57.9 |
| Encoder, 0.2.1: insert and copy length codes worked out, not found by a binary search of the 24 bases | 2.2% of the time at 6 on the 1.1 MB script | 1.4% |
| Encoder, 0.2.1: `bits_entropy` without a branch on zero counts (`log2(0)` is 0 in the table, so a zero adds exactly 0) | | within 1%, every input |
| The three above, end to end, against 0.2.0 | 1.1 MB script 62.1 MB/s at 5, 55.6 at 6; html 1 MiB 92.9, 76.7; text 5 MiB 58.5, 44.9 | 67.4, 59.6; 99.2, 81.8; 63.9, 49.4 |

Tried and dropped:

- A branchless `canonical` (zero lengths counted in `next[0]`): 1.8 µs to 3.0 µs, since each
  zero then waits on the last update of the same counter.
- Tags as the reference's H58 and H68 keep them (0.2.1), with the same output, on four real
  files from 74 KB to 1.1 MB. Compared 8 at a time in a `u64`: 0% to 10% slower at 5 and 7% to
  17% at 6. Compared 16 at a time in a loop the compiler vectorizes itself: 1% to 11% slower.
  Checked one at a time during the walk: 14% to 20% slower. Filing a tag per position costs a
  write to one more table, and safe Rust has no SIMD on stable to compare 16 tags in one
  instruction.
- A tighter walk (the 4 bytes compared as a word kept between candidates, and a stop once a
  match reaches the longest allowed): no change; the walk waits on memory, not instructions.

Considered and not done: weighing only the contexts a block touched, from cached entropies. A
block touches 8 to 10 of the 13, and the splitter decides only about 70 blocks on the 1.1 MB
script, so it would save under 1%.

Where the time went after the first five changes (B9), for a 1 KiB body at quality 4 (15 µs
in all): matching 4.2 µs, building the prefix codes 3.2, histogram tuning 1.8, block splitting
1.8, writing the codes 2.2, writing the data 1.3. From 4 KiB up, matching is most of it.
