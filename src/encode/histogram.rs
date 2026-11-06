//! Symbol counts: their entropy, and the reference's tuning of them for run-length coded code
//! lengths (`c/enc/bit_cost.c`, `BrotliBitsEntropy`; `c/enc/entropy_encode.c`,
//! `BrotliOptimizeHuffmanCountsForRle`).

/// How far, in 1/256 units, a count may be from its stride's average and still join it.
const STREAK: i64 = 1240;

/// The bits `counts` take with an ideal code: Shannon's sum, but at least one bit per
/// symbol, as a prefix code needs.
pub(crate) fn bits_entropy(counts: &[u32]) -> f64 {
    let mut sum = 0u64;
    let mut bits = 0.0;
    for &c in counts {
        if c != 0 {
            sum += u64::from(c);
            bits -= f64::from(c) * f64::from(c).log2();
        }
    }
    if sum != 0 {
        bits += sum as f64 * (sum as f64).log2();
    }
    bits.max(sum as f64)
}

/// Adjusts `counts` so that their code lengths run-length code better: runs of similar counts
/// (where a code length would barely change) are set to their average. Used symbols keep a
/// count; unused ones may get one, which costs nothing but a code length.
pub(crate) fn optimize_for_rle(counts: &mut [u32]) {
    if counts.iter().filter(|&&c| c != 0).count() < 16 {
        return;
    }
    let length = counts.iter().rposition(|&c| c != 0).map_or(0, |i| i + 1);
    let counts = &mut counts[..length];
    let nonzeros = counts.iter().filter(|&&c| c != 0).count();
    if nonzeros < 5 {
        return;
    }
    let smallest = counts
        .iter()
        .copied()
        .filter(|&c| c != 0)
        .min()
        .unwrap_or(0);
    if smallest < 4 && length - nonzeros < 6 {
        // Few holes among small counts: fill single ones, which lets their neighbors run.
        for i in 1..length - 1 {
            if counts[i - 1] != 0 && counts[i] == 0 && counts[i + 1] != 0 {
                counts[i] = 1;
            }
        }
    }
    if nonzeros < 28 {
        return;
    }
    // Runs that already code well: 5 or more zeros, 7 or more of one count. They are kept.
    let mut good = vec![false; length];
    let mut start = 0;
    for i in 1..=length {
        if i == length || counts[i] != counts[start] {
            let run = i - start;
            if (counts[start] == 0 && run >= 5) || (counts[start] != 0 && run >= 7) {
                good[start..i].fill(true);
            }
            start = i;
        }
    }
    // Then strides of counts close to their running average (in 1/256 units, within
    // STREAK) become that average, when at least 4 long (3 for zeros). From 4 on, the
    // average is `(256 * sum + stride / 2) / stride`, plus 120 at exactly 4; it is compared
    // without dividing: floor(a / s) <= k is a < (k + 1) * s, and floor(a / s) > k is
    // a >= (k + 1) * s.
    let average3 = |c: &[u32], i: usize| {
        256 * (i64::from(c[i]) + i64::from(c[i + 1]) + i64::from(c[i + 2])) / 3 + 420
    };
    let mut stride = 0usize;
    let mut sum = 0u64;
    // The limit while the stride is under 4.
    let mut fixed = average3(counts, 0);
    let mut i = 0;
    while i <= length {
        let ends = i == length || good[i] || (i != 0 && good[i - 1]) || {
            let c = 256 * i64::from(counts[i]);
            if stride >= 4 {
                let bump = if stride == 4 { 120 } else { 0 };
                let s = stride as i64;
                let a = 256 * sum as i64 + s / 2;
                // Not (c - STREAK < limit <= c + STREAK).
                !(a < (c + STREAK - bump + 1) * s && a >= (c - STREAK - bump + 1) * s)
            } else {
                !(-STREAK..STREAK).contains(&(c - fixed))
            }
        };
        if ends {
            if stride >= 4 || (stride >= 3 && sum == 0) {
                // Zeros stay zeros; anything else becomes the average, at least 1.
                let value = if sum == 0 {
                    0
                } else {
                    ((sum + stride as u64 / 2) / stride as u64).max(1) as u32
                };
                counts[i - stride..i].fill(value);
            }
            stride = 0;
            sum = 0;
            fixed = if i + 2 < length {
                average3(counts, i)
            } else if i < length {
                256 * i64::from(counts[i])
            } else {
                0
            };
        }
        stride += 1;
        if i != length {
            sum += u64::from(counts[i]);
        }
        if i < length && good[i] {
            // Each symbol up to the end of this good run, and the one after it, only ends a
            // stride of 1 and starts the next: go straight to the one after. There the stride
            // ends again (the symbol before it is good) and the limit is set afresh; a stride
            // of 0 fills nothing.
            i = good[i..].iter().position(|&g| !g).map_or(length, |n| i + n);
            stride = 0;
            sum = 0;
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{STREAK, bits_entropy, optimize_for_rle};

    /// The tuning as the reference writes it, one symbol at a time with a division each: the
    /// fast version must give the same counts.
    #[allow(clippy::many_single_char_names, reason = "a copy of the plain version")]
    fn optimize_for_rle_by_the_book(counts: &mut [u32]) {
        if counts.iter().filter(|&&c| c != 0).count() < 16 {
            return;
        }
        let length = counts.iter().rposition(|&c| c != 0).map_or(0, |i| i + 1);
        let counts = &mut counts[..length];
        let nonzeros = counts.iter().filter(|&&c| c != 0).count();
        if nonzeros < 5 {
            return;
        }
        let smallest = counts
            .iter()
            .copied()
            .filter(|&c| c != 0)
            .min()
            .unwrap_or(0);
        if smallest < 4 && length - nonzeros < 6 {
            // Few holes among small counts: fill single ones, which lets their neighbors run.
            for i in 1..length - 1 {
                if counts[i - 1] != 0 && counts[i] == 0 && counts[i + 1] != 0 {
                    counts[i] = 1;
                }
            }
        }
        if nonzeros < 28 {
            return;
        }
        // Runs that already code well: 5 or more zeros, 7 or more of one count. They are kept.
        let mut good = vec![false; length];
        let mut start = 0;
        for i in 1..=length {
            if i == length || counts[i] != counts[start] {
                let run = i - start;
                if (counts[start] == 0 && run >= 5) || (counts[start] != 0 && run >= 7) {
                    good[start..i].fill(true);
                }
                start = i;
            }
        }
        // Then strides of counts close to their running average (in 1/256 units, within
        // STREAK) become that average, when at least 4 long (3 for zeros).
        let average3 = |c: &[u32], i: usize| {
            256 * (i64::from(c[i]) + i64::from(c[i + 1]) + i64::from(c[i + 2])) / 3 + 420
        };
        let mut stride = 0usize;
        let mut sum = 0u64;
        let mut limit = average3(counts, 0);
        for i in 0..=length {
            let ends = i == length
                || good[i]
                || (i != 0 && good[i - 1])
                || !(-STREAK..STREAK).contains(&(256 * i64::from(counts[i]) - limit));
            if ends {
                if stride >= 4 || (stride >= 3 && sum == 0) {
                    // Zeros stay zeros; anything else becomes the average, at least 1.
                    let value = if sum == 0 {
                        0
                    } else {
                        ((sum + stride as u64 / 2) / stride as u64).max(1) as u32
                    };
                    counts[i - stride..i].fill(value);
                }
                stride = 0;
                sum = 0;
                limit = if i + 2 < length {
                    average3(counts, i)
                } else if i < length {
                    256 * i64::from(counts[i])
                } else {
                    0
                };
            }
            stride += 1;
            if i != length {
                sum += u64::from(counts[i]);
                if stride >= 4 {
                    limit = ((256 * sum + stride as u64 / 2) / stride as u64) as i64;
                }
                if stride == 4 {
                    limit += 120;
                }
            }
        }
    }

    #[test]
    fn the_fast_tuning_is_the_plain_one() {
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut below = |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % n
        };
        for case in 0..20_000 {
            let size = [26usize, 64, 256, 704][case % 4];
            let density = below(100);
            let scale = [2u64, 10, 1000, 1 << 20][below(4) as usize];
            let mut counts: Vec<u32> = (0..size)
                .map(|_| {
                    if below(100) < density {
                        below(scale) as u32
                    } else {
                        0
                    }
                })
                .collect();
            if case % 3 == 0 {
                // Runs of equal and nearly equal counts.
                let mut i = 0;
                while i < size {
                    let (v, run) = (counts[i], below(12) as usize);
                    for c in &mut counts[i..(i + run).min(size)] {
                        *c = v + below(2) as u32;
                    }
                    i += run.max(1);
                }
            }
            let (mut fast, mut plain) = (counts.clone(), counts.clone());
            optimize_for_rle(&mut fast);
            optimize_for_rle_by_the_book(&mut plain);
            assert!(fast == plain, "case {case}: {counts:?}");
        }
    }

    #[test]
    fn entropy_of_known_counts() {
        assert_eq!(bits_entropy(&[]), 0.0);
        assert_eq!(bits_entropy(&[0, 7, 0]), 7.0, "one symbol: a bit each");
        assert!((bits_entropy(&[4, 4, 4, 4]) - 32.0).abs() < 1e-9);
        assert!((bits_entropy(&[1, 1, 2]) - 6.0).abs() < 1e-9);
    }

    #[test]
    fn near_equal_counts_are_flattened_and_used_symbols_kept() {
        // 40 symbols with counts wobbling around 100: one average after tuning.
        let mut counts: Vec<u32> = (0..40).map(|i| 100 + (i * 7 % 5)).collect();
        let before = counts.clone();
        optimize_for_rle(&mut counts);
        assert!(counts.iter().all(|&c| c != 0));
        let distinct: std::collections::BTreeSet<u32> = counts.iter().copied().collect();
        assert!(distinct.len() < 5, "{counts:?}");
        assert_ne!(counts, before);
    }

    #[test]
    fn small_or_peaky_histograms_are_left_alone() {
        let mut few = vec![0u32; 256];
        for (i, c) in [(3, 50), (90, 2), (200, 7)] {
            few[i] = c;
        }
        let before = few.clone();
        optimize_for_rle(&mut few);
        assert_eq!(few, before, "fewer than 16 symbols");
        // Counts far apart are not runs: the large ones stay. (The small ones, within a few
        // of each other, may be averaged.)
        let mut peaky: Vec<u32> = (0..64).map(|i| 1 << (i % 16)).collect();
        let before = peaky.clone();
        optimize_for_rle(&mut peaky);
        for (i, (&a, &b)) in peaky.iter().zip(&before).enumerate() {
            assert!(b < 64 || a == b, "symbol {i}: {b} became {a}");
        }
    }

    #[test]
    fn every_used_symbol_keeps_a_count() {
        let mut x = 1u32;
        for round in 0..2000 {
            let len = [64, 256, 704][round % 3];
            let mut counts: Vec<u32> = (0..len)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    if x.is_multiple_of(3) {
                        0
                    } else {
                        x % [3, 50, 5000][round % 3]
                    }
                })
                .collect();
            let used: Vec<bool> = counts.iter().map(|&c| c != 0).collect();
            optimize_for_rle(&mut counts);
            for (i, &u) in used.iter().enumerate() {
                assert!(
                    !u || counts[i] != 0,
                    "round {round}: symbol {i} lost its count"
                );
            }
        }
    }
}
