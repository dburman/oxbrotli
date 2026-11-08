//! Block splitting (RFC 7932 §6): each category's symbols cut into blocks, each block given a
//! type with its own prefix code. Quality 4 does it greedily, as the reference does
//! (`c/enc/metablock_inc.h`, `BlockSplitter`; `c/enc/metablock.c`,
//! `BrotliBuildMetaBlockGreedy`). From quality 5 literals may also be split by context
//! (`ContextBlockSplitter`): each type then has a histogram per context, and a block's fit to a
//! type is summed over them.

use crate::encode::histogram::bits_entropy;

/// At most this many block types per category (§6).
const MAX_TYPES: usize = 256;

/// A category's blocks: the type and the symbol count of each, the number of types, and the
/// symbol counts of each type: `contexts` histograms in a row, one per context.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Split {
    pub(crate) types: Vec<u8>,
    pub(crate) lengths: Vec<u32>,
    pub(crate) counts: Vec<Vec<u32>>,
    pub(crate) contexts: usize,
}

impl Split {
    pub(crate) fn num_types(&self) -> usize {
        self.counts.len()
    }
}

/// The greedy splitter for one category.
pub(crate) struct Splitter {
    alphabet: usize,
    /// Histograms per type, one per context.
    contexts: usize,
    /// At most this many types: 256 codes in all.
    max_types: usize,
    /// Each block holds at least this many symbols.
    min_block: usize,
    /// Bits a new type must save to be made.
    threshold: f64,
    split: Split,
    /// The counts of the block being collected, per context.
    current: Vec<u32>,
    size: usize,
    /// The size at which the current block is decided.
    target: usize,
    /// The last two types used, most recent first, and their entropies (summed over the
    /// contexts).
    last: [usize; 2],
    last_entropy: [f64; 2],
    /// Joins of the last type in a row.
    joins: usize,
}

impl Splitter {
    fn new(alphabet: usize, contexts: usize, min_block: usize, threshold: f64) -> Self {
        Self {
            alphabet,
            contexts,
            max_types: MAX_TYPES / contexts,
            min_block,
            threshold,
            split: Split {
                types: Vec::new(),
                lengths: Vec::new(),
                counts: Vec::new(),
                contexts,
            },
            current: vec![0; alphabet * contexts],
            size: 0,
            target: min_block,
            last: [0, 0],
            last_entropy: [0.0, 0.0],
            joins: 0,
        }
    }

    /// The literal splitter, with a histogram per type for each of `contexts` (1 to 13).
    pub(crate) fn literals(contexts: usize) -> Self {
        debug_assert!((1..=13).contains(&contexts), "{contexts} contexts");
        Self::new(256, contexts, 512, 400.0)
    }

    pub(crate) fn commands() -> Self {
        Self::new(704, 1, 1024, 500.0)
    }

    pub(crate) fn distances() -> Self {
        Self::new(64, 1, 512, 100.0)
    }

    #[inline]
    pub(crate) fn add(&mut self, symbol: usize) {
        self.add_in(symbol, 0);
    }

    /// Adds `symbol`, seen in `context`.
    #[inline]
    pub(crate) fn add_in(&mut self, symbol: usize, context: usize) {
        self.current[context * self.alphabet + symbol] += 1;
        self.size += 1;
        if self.size == self.target {
            self.finish_block();
        }
    }

    /// The bits `counts` take, coded per context.
    fn entropy(&self, counts: &[u32]) -> f64 {
        counts.chunks_exact(self.alphabet).map(bits_entropy).sum()
    }

    /// Decides the block collected so far: a new type, the second-last type again, or more of
    /// the last type.
    fn finish_block(&mut self) {
        // The first and the final block count as at least the minimum, as the reference's do.
        let size = self.size.max(self.min_block) as u32;
        if self.split.types.is_empty() {
            self.last_entropy = [self.entropy(&self.current); 2];
            let split = &mut self.split;
            split.types.push(0);
            split.lengths.push(size);
            split.counts.push(std::mem::replace(
                &mut self.current,
                vec![0; self.alphabet * self.contexts],
            ));
            self.size = 0;
            return;
        }
        let entropy = self.entropy(&self.current);
        let combined: [Vec<u32>; 2] = self.last.map(|t| {
            self.split.counts[t]
                .iter()
                .zip(&self.current)
                .map(|(a, b)| a + b)
                .collect()
        });
        let combined_entropy = [self.entropy(&combined[0]), self.entropy(&combined[1])];
        let split = &mut self.split;
        let diff = [
            combined_entropy[0] - entropy - self.last_entropy[0],
            combined_entropy[1] - entropy - self.last_entropy[1],
        ];
        let [first, second] = combined;
        if split.counts.len() < self.max_types
            && diff[0] > self.threshold
            && diff[1] > self.threshold
        {
            let new = split.counts.len();
            split.types.push(new as u8);
            split.lengths.push(size);
            split.counts.push(std::mem::replace(
                &mut self.current,
                vec![0; self.alphabet * self.contexts],
            ));
            self.last = [new, self.last[0]];
            self.last_entropy = [entropy, self.last_entropy[0]];
            self.joins = 0;
            self.target = self.min_block;
        } else if diff[1] < diff[0] - 20.0 {
            // Back to the type before the last.
            let back = split.types[split.types.len() - 2];
            split.types.push(back);
            split.lengths.push(size);
            self.last = [self.last[1], self.last[0]];
            split.counts[self.last[0]] = second;
            self.last_entropy = [combined_entropy[1], self.last_entropy[0]];
            self.current.fill(0);
            self.joins = 0;
            self.target = self.min_block;
        } else {
            if let Some(length) = split.lengths.last_mut() {
                *length += size;
            }
            split.counts[self.last[0]] = first;
            self.last_entropy[0] = combined_entropy[0];
            if split.counts.len() == 1 {
                self.last_entropy[1] = self.last_entropy[0];
            }
            self.current.fill(0);
            self.joins += 1;
            if self.joins > 1 {
                self.target += self.min_block;
            }
        }
        self.size = 0;
    }

    /// The split, after the last block.
    pub(crate) fn finish(mut self) -> Split {
        self.finish_block();
        self.split
    }
}

#[cfg(test)]
mod tests {
    use super::{Split, Splitter};

    fn split_of(symbols: impl IntoIterator<Item = usize>) -> Split {
        let mut s = Splitter::literals(1);
        for x in symbols {
            s.add(x);
        }
        s.finish()
    }

    /// Checks the split's own invariants: types start at 0 and each new one is the next, the
    /// counts add up per type, and no block is shorter than the minimum.
    fn check(split: &Split, total: usize) {
        let mut seen = 0;
        for &t in &split.types {
            assert!(usize::from(t) <= seen, "type {t} after {seen} types");
            seen = seen.max(usize::from(t) + 1);
        }
        assert_eq!(seen, split.num_types());
        let counted: u32 = split.counts.iter().flatten().sum();
        assert_eq!(counted as usize, total);
        assert!(split.lengths.iter().all(|&l| l >= 512));
        assert!(split.lengths.iter().map(|&l| l as usize).sum::<usize>() >= total);
    }

    #[test]
    fn uniform_symbols_stay_one_block() {
        let symbols: Vec<usize> = (0..20_000).map(|i| (i * 7919) % 26).collect();
        let split = split_of(symbols.iter().copied());
        check(&split, symbols.len());
        assert_eq!(split.num_types(), 1);
        assert_eq!(split.types, [0]);
    }

    #[test]
    fn different_sections_get_their_own_types_and_come_back() {
        // Lowercase and digits in turn, a block's worth each: two types, alternating, each
        // block going back to the type before the last.
        let lower = (0..512).map(|i| usize::from(b'a') + (i * 7) % 26);
        let digits = (0..512).map(|i| usize::from(b'0') + (i * 3) % 10);
        let both: Vec<usize> = lower.chain(digits).collect();
        let symbols: Vec<usize> = both.iter().copied().cycle().take(6 * 512).collect();
        let split = split_of(symbols.iter().copied());
        check(&split, symbols.len());
        assert_eq!(split.types, [0, 1, 0, 1, 0, 1]);
        // The final, empty block counts as the minimum and joins the last, as the
        // reference's does: the decoder never gets to the end of the last block.
        assert_eq!(split.lengths, [512, 512, 512, 512, 512, 1024]);
    }

    #[test]
    fn long_sections_split_where_they_change() {
        // Longer sections: block edges fall inside them, so the mixed blocks may get types of
        // their own; the digits are never coded with the letters' type.
        let lower = (0..8000).map(|i| usize::from(b'a') + (i * 7) % 26);
        let digits = (0..8000).map(|i| usize::from(b'0') + (i * 3) % 10);
        let symbols: Vec<usize> = lower.chain(digits).collect();
        let split = split_of(symbols.iter().copied());
        check(&split, symbols.len());
        assert!(split.num_types() >= 2);
        let last = usize::from(*split.types.last().unwrap());
        assert_eq!(
            split.counts[last][usize::from(b'a')..=usize::from(b'z')]
                .iter()
                .sum::<u32>(),
            0
        );
    }

    #[test]
    fn fewer_symbols_than_a_block_are_one_block() {
        let split = split_of([1, 2, 3]);
        check(&split, 3);
        assert_eq!(
            (split.types.as_slice(), split.lengths.as_slice()),
            (&[0u8][..], &[512u32][..])
        );
        let empty = split_of([]);
        check(&empty, 0);
        assert_eq!(empty.num_types(), 1);
    }

    #[test]
    fn contexts_keep_a_histogram_each_and_split_together() {
        // Two contexts, each with its own alphabet, the same throughout: one type, the
        // symbols counted in their context's histogram.
        let mut s = Splitter::literals(2);
        for i in 0..20_000 {
            if i % 2 == 0 {
                s.add_in(usize::from(b'a') + i % 26, 0);
            } else {
                s.add_in(usize::from(b'0') + i % 10, 1);
            }
        }
        let split = s.finish();
        assert_eq!((split.num_types(), split.contexts), (1, 2));
        let (first, second) = split.counts[0].split_at(256);
        assert_eq!(first.iter().sum::<u32>(), 10_000);
        assert_eq!(first[usize::from(b'0')..=usize::from(b'9')], [0; 10]);
        assert_eq!(second.iter().sum::<u32>(), 10_000);
        assert_eq!(
            second[usize::from(b'0')..=usize::from(b'9')]
                .iter()
                .sum::<u32>(),
            10_000
        );
        // A change in one context alone splits: the sum over contexts decides.
        let mut s = Splitter::literals(2);
        for i in 0..8192 {
            let other = if i < 4096 { b'A' } else { b'0' };
            let ctx = i % 2;
            let b = if ctx == 0 {
                b'a' + (i % 26) as u8
            } else {
                other + (i % 10) as u8
            };
            s.add_in(usize::from(b), ctx);
        }
        assert!(s.finish().num_types() >= 2);
    }

    #[test]
    fn a_change_in_the_mix_of_contexts_alone_keeps_the_type() {
        // Each context's symbols keep their spread, but the share of each context turns
        // around halfway: coded per context, nothing changed, so one type.
        let mut s = Splitter::literals(2);
        for i in 0..20_000 {
            let first = (i % 100 < 99) == (i < 10_000);
            if first {
                s.add_in(usize::from(b'a') + i % 26, 0);
            } else {
                s.add_in(usize::from(b'0') + i % 10, 1);
            }
        }
        assert_eq!(s.finish().num_types(), 1);
    }

    #[test]
    fn types_stop_at_256_codes_with_contexts() {
        // 13 contexts: at most 19 types, 247 codes.
        let mut s = Splitter::literals(13);
        for i in 0..400 * 512 {
            s.add_in((i / 512 * 37 + i % 3) % 256, i % 13);
        }
        let split = s.finish();
        assert_eq!(split.num_types(), 256 / 13);
        assert!(split.counts.iter().all(|c| c.len() == 13 * 256));
    }

    #[test]
    fn types_stop_at_256() {
        // A new alphabet every block: as many types as allowed, then joins.
        let symbols: Vec<usize> = (0..400 * 512)
            .map(|i| (i / 512 * 37 + i % 3) % 256)
            .collect();
        let split = split_of(symbols.iter().copied());
        check(&split, symbols.len());
        assert!(split.num_types() <= 256);
    }
}
