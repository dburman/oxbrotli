//! Prefix codes for the encoder: length-limited code lengths from symbol counts, and their
//! canonical codes (RFC 7932 §3.2).

use crate::bitwriter::reverse;

/// A prefix code over an alphabet, ready to write symbols with.
#[derive(Default, Clone)]
pub(crate) struct Code {
    /// Per symbol: its code length, 0 if unused. When only one symbol is used it has length
    /// 0 too, and writing it takes no bits (§3.4).
    pub(crate) lengths: Vec<u8>,
    /// Per symbol: its code, bit-reversed for [`BitWriter::put`](crate::bitwriter::BitWriter).
    pub(crate) codes: Vec<u16>,
    /// The symbols used, in increasing order, when there are at most four (or none).
    pub(crate) few: Option<Vec<u16>>,
}

impl Code {
    /// The code for `counts` (one per symbol of the alphabet), no length over `max_length`.
    pub(crate) fn from_counts(counts: &[u32], max_length: u8) -> Self {
        let mut code = Self::default();
        code.rebuild(counts, max_length);
        code
    }

    /// As [`from_counts`](Self::from_counts), reusing this code's buffers.
    pub(crate) fn rebuild(&mut self, counts: &[u32], max_length: u8) {
        self.lengths.clear();
        self.lengths.resize(counts.len(), 0);
        let used = used_symbols(counts);
        if used.len() > 1 {
            lengths_of_used(counts, &used, max_length, &mut self.lengths);
        }
        self.few = (used.len() <= 4).then_some(used);
        canonical(&self.lengths, &mut self.codes);
    }

    /// Bits to write `counts[s]` copies of each symbol `s` with this code.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "costs choose between codes once there are several"
        )
    )]
    pub(crate) fn cost(&self, counts: &[u32]) -> u64 {
        counts
            .iter()
            .zip(&self.lengths)
            .map(|(&c, &l)| u64::from(c) * u64::from(l))
            .sum()
    }
}

/// Bits of a symbol in a sort key: alphabets have at most 704 symbols.
const SYMBOL_BITS: u32 = 16;

/// One node of a Huffman tree under construction.
#[derive(Clone, Copy)]
struct Node {
    count: u64,
    /// For a leaf, its symbol; for an internal node, its children.
    left: u32,
    right: u32,
    leaf: bool,
}

/// Huffman code lengths for the non-zero `counts`, none longer than `max_length`, into
/// `lengths`. While the tree is too deep, the smallest counts are raised to a floor that
/// doubles each round, which flattens it (as the reference does, `c/enc/entropy_encode.c`,
/// `BrotliCreateHuffmanTree`). Needs at least two non-zero counts, and few enough symbols to
/// fit in `max_length` bits.
pub(crate) fn limited_lengths(counts: &[u32], max_length: u8, lengths: &mut [u8]) {
    lengths_of_used(counts, &used_symbols(counts), max_length, lengths);
}

/// The symbols with non-zero counts, in order, skipping zeros eight at a time: alphabets are
/// mostly unused.
fn used_symbols(counts: &[u32]) -> Vec<u16> {
    debug_assert!(counts.len() <= 1 << SYMBOL_BITS);
    let mut used = Vec::new();
    let (chunks, rest) = counts.as_chunks::<8>();
    for (i, chunk) in chunks.iter().enumerate() {
        if chunk.iter().fold(0, |a, &c| a | c) == 0 {
            continue;
        }
        for (j, &c) in chunk.iter().enumerate() {
            if c != 0 {
                used.push((8 * i + j) as u16);
            }
        }
    }
    for (j, &c) in rest.iter().enumerate() {
        if c != 0 {
            used.push((8 * chunks.len() + j) as u16);
        }
    }
    used
}

/// As [`limited_lengths`], given the symbols `used` (those with non-zero counts, in order).
fn lengths_of_used(counts: &[u32], used: &[u16], max_length: u8, lengths: &mut [u8]) {
    let mut floor: u64 = 1;
    let mut keys: Vec<u64> = Vec::with_capacity(used.len());
    let mut nodes: Vec<Node> = Vec::with_capacity(2 * used.len());
    loop {
        // Leaves by count, ties by symbol: the same code for the same counts everywhere. Each
        // as one key, the count above the symbol, so the sort moves 8 bytes, not a node; the
        // keys are all different, so an unstable sort orders them as a stable one would.
        keys.clear();
        keys.extend(used.iter().map(|&s| {
            (u64::from(counts[usize::from(s)]).max(floor) << SYMBOL_BITS) | u64::from(s)
        }));
        keys.sort_unstable();
        nodes.clear();
        nodes.extend(keys.iter().map(|&k| Node {
            count: k >> SYMBOL_BITS,
            left: (k & ((1 << SYMBOL_BITS) - 1)) as u32,
            right: 0,
            leaf: true,
        }));
        let leaves = nodes.len();
        debug_assert!(leaves >= 2);
        // Two queues: the sorted leaves, and the internal nodes in the order they are made
        // (their counts never decrease).
        let (mut next_leaf, mut next_inner) = (0, leaves);
        let mut take = |nodes: &Vec<Node>| {
            let leaf_ok = next_leaf < leaves;
            let inner_ok = next_inner < nodes.len();
            if leaf_ok && (!inner_ok || nodes[next_leaf].count <= nodes[next_inner].count) {
                next_leaf += 1;
                next_leaf - 1
            } else {
                next_inner += 1;
                next_inner - 1
            }
        };
        for _ in 1..leaves {
            let a = take(&nodes);
            let b = take(&nodes);
            nodes.push(Node {
                count: nodes[a].count + nodes[b].count,
                left: a as u32,
                right: b as u32,
                leaf: false,
            });
        }
        // Depths from the root (the last node) down.
        let mut depth = vec![0u8; nodes.len()];
        let mut deepest = 0;
        for i in (0..nodes.len()).rev() {
            let n = nodes[i];
            if n.leaf {
                deepest = deepest.max(depth[i]);
            } else {
                depth[n.left as usize] = depth[i] + 1;
                depth[n.right as usize] = depth[i] + 1;
            }
        }
        if deepest <= max_length {
            for (i, n) in nodes[..leaves].iter().enumerate() {
                lengths[n.left as usize] = depth[i];
            }
            return;
        }
        floor *= 2;
    }
}

/// Calls `f(symbol, length)` for each symbol with a non-zero length, skipping zeros eight
/// at a time. Alphabets are mostly unused, and a pass that touches every symbol's counter
/// waits on the last update of the same counter each time.
#[inline(always)]
fn each_used(lengths: &[u8], mut f: impl FnMut(usize, u8)) {
    let (chunks, rest) = lengths.as_chunks::<8>();
    for (i, chunk) in chunks.iter().enumerate() {
        if u64::from_ne_bytes(*chunk) == 0 {
            continue;
        }
        for (j, &l) in chunk.iter().enumerate() {
            if l != 0 {
                f(8 * i + j, l);
            }
        }
    }
    for (j, &l) in rest.iter().enumerate() {
        if l != 0 {
            f(8 * chunks.len() + j, l);
        }
    }
}

/// Canonical codes for `lengths` (§3.2), bit-reversed for writing, into `codes`; 0 for unused
/// symbols.
pub(crate) fn canonical(lengths: &[u8], codes: &mut Vec<u16>) {
    let mut count = [0u16; 16];
    each_used(lengths, |_, l| count[usize::from(l)] += 1);
    let mut next = [0u32; 16];
    let mut code = 0u32;
    for bits in 1..16 {
        code = (code + u32::from(count[bits - 1])) << 1;
        next[bits] = code;
    }
    codes.clear();
    codes.resize(lengths.len(), 0);
    each_used(lengths, |s, l| {
        let c = next[usize::from(l)];
        next[usize::from(l)] += 1;
        codes[s] = reverse(c, u32::from(l)) as u16;
    });
}

#[cfg(test)]
mod tests {
    use super::{Code, canonical, limited_lengths};

    fn kraft(lengths: &[u8]) -> u64 {
        lengths
            .iter()
            .filter(|&&l| l != 0)
            .map(|&l| 1u64 << (15 - l))
            .sum()
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    /// Bits for `counts` with a plain Huffman code, its lengths not limited.
    fn unlimited_cost(counts: &[u32]) -> u64 {
        let mut lengths = vec![0u8; counts.len()];
        limited_lengths(counts, u8::MAX, &mut lengths);
        counts
            .iter()
            .zip(&lengths)
            .map(|(&c, &l)| u64::from(c) * u64::from(l))
            .sum()
    }

    /// Canonical codes one symbol at a time, as §3.2 gives them, reversed.
    fn canonical_by_the_book(lengths: &[u8]) -> Vec<u16> {
        let mut next = [0u32; 16];
        let mut code = 0;
        for (bits, next) in next.iter_mut().enumerate().skip(1) {
            let shorter = lengths
                .iter()
                .filter(|&&l| usize::from(l) == bits - 1)
                .count();
            code = (code + if bits == 1 { 0 } else { shorter as u32 }) << 1;
            *next = code;
        }
        lengths
            .iter()
            .map(|&l| {
                if l == 0 {
                    return 0;
                }
                let c = next[usize::from(l)];
                next[usize::from(l)] += 1;
                (c.reverse_bits() >> (32 - u32::from(l))) as u16
            })
            .collect()
    }

    #[test]
    fn canonical_codes_skip_unused_symbols_correctly() {
        // Codes from real counts, with gaps of every size and alphabets that don't end on a
        // multiple of 8.
        for size in [2, 7, 8, 9, 26, 64, 255, 256, 704] {
            for stride in [1, 2, 3, 7, 8, 9, 17, 60] {
                let counts: Vec<u32> = (0..size)
                    .map(|i| {
                        if i % stride == 0 || i == size - 1 {
                            1 + (i as u32 * 7) % 13
                        } else {
                            0
                        }
                    })
                    .collect();
                if counts.iter().filter(|&&c| c != 0).count() < 2 {
                    continue;
                }
                let code = Code::from_counts(&counts, 15);
                let mut codes = Vec::new();
                canonical(&code.lengths, &mut codes);
                assert_eq!(
                    codes,
                    canonical_by_the_book(&code.lengths),
                    "{size} symbols, every {stride}"
                );
            }
        }
    }

    #[test]
    fn huffman_lengths_for_a_known_histogram() {
        // Counts 1, 1, 2, 4: lengths 3, 3, 2, 1.
        let mut lengths = [0u8; 4];
        limited_lengths(&[1, 1, 2, 4], 15, &mut lengths);
        assert_eq!(lengths, [3, 3, 2, 1]);
    }

    #[test]
    fn limited_codes_are_complete_short_and_nearly_optimal() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for round in 0..1000 {
            let alphabet = [2, 3, 18, 26, 64, 256, 704][round % 7];
            let counts: Vec<u32> = (0..alphabet)
                .map(|i| match round % 3 {
                    // Fibonacci-like counts: an unlimited tree would be as deep as the
                    // alphabet is long.
                    0 if i < 40 => (1u32 << (i.min(30))).max(1) / 2 + 1,
                    1 => (rng.next() % 1000) as u32,
                    _ => (rng.next() % 2) as u32 * (rng.next() % 50_000) as u32,
                })
                .collect();
            if counts.iter().filter(|&&c| c != 0).count() < 2 {
                continue;
            }
            for max in [5u8, 15] {
                if alphabet > 1 << max {
                    continue;
                }
                let limited = Code::from_counts(&counts, max);
                assert_eq!(kraft(&limited.lengths), 1 << 15, "round {round}, max {max}");
                assert!(limited.lengths.iter().all(|&l| l <= max), "round {round}");
                // Within 1% of Huffman at the limit of 15. At 5 (the code-length code, 18
                // symbols) a skewed histogram must cost more; only the shape is checked.
                let (a, b) = (limited.cost(&counts), unlimited_cost(&counts));
                assert!(
                    max == 5 || a as f64 <= b as f64 * 1.01 + 8.0,
                    "round {round}, max {max}: {a} bits limited, {b} unlimited"
                );
                for (s, &c) in counts.iter().enumerate() {
                    assert_eq!(c == 0, limited.lengths[s] == 0, "round {round}, symbol {s}");
                }
            }
        }
    }

    #[test]
    fn deep_counts_are_flattened_to_the_limit() {
        // Powers of two: Huffman's tree is a chain 20 deep.
        let counts: Vec<u32> = (0..21).map(|i| 1 << i).collect();
        let code = Code::from_counts(&counts, 15);
        assert_eq!(code.lengths.iter().max(), Some(&15));
        assert_eq!(kraft(&code.lengths), 1 << 15);
        let mut unlimited = vec![0u8; counts.len()];
        limited_lengths(&counts, u8::MAX, &mut unlimited);
        assert_eq!(unlimited.iter().max(), Some(&20));
    }

    #[test]
    fn few_symbols_are_listed() {
        let mut counts = vec![0u32; 256];
        counts[200] = 5;
        let one = Code::from_counts(&counts, 15);
        assert_eq!(one.few.as_deref(), Some(&[200][..]));
        assert_eq!(one.lengths[200], 0, "a lone symbol takes no bits");
        counts[3] = 1;
        counts[9] = 1;
        counts[100] = 1;
        let four = Code::from_counts(&counts, 15);
        assert_eq!(four.few.as_deref(), Some(&[3, 9, 100, 200][..]));
        counts[4] = 1;
        assert_eq!(Code::from_counts(&counts, 15).few, None);
        assert_eq!(
            Code::from_counts(&[0; 10], 15).few.as_deref(),
            Some(&[][..])
        );
    }
}
