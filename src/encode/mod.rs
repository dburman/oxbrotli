//! The encoder: commands into meta-blocks (RFC 7932 §9), and the stream around them. The
//! match finder is in [`matcher`].

pub(crate) mod chain;
pub(crate) mod command;
pub(crate) mod context;
pub(crate) mod forgetful;
pub(crate) mod hash;
pub(crate) mod histogram;
pub(crate) mod huffman;
pub(crate) mod matcher;
pub(crate) mod metablock;
pub(crate) mod static_dict;
pub(crate) mod store;
pub(crate) mod stream;

use crate::bitwriter::BitWriter;
use crate::tables::{
    BLOCK_COUNT_BASE, BLOCK_COUNT_EXTRA, COPY_BASE, COPY_EXTRA, INSERT_BASE, INSERT_EXTRA,
};
use command::{Coded, Command};
use context::Model;
use huffman::Code;
use metablock::Split;

/// The longest code length of a literal, command or distance code (§3.5).
const MAX_LENGTH: u8 = 15;
const LITERALS: usize = 256;
const COMMANDS: usize = 704;
/// The distance alphabet with `NPOSTFIX = 0` and `NDIRECT = 0` (§4): 16 + 48 codes.
const DISTANCES: usize = 64;
/// The longest meta-block (§9.2).
pub(crate) const MAX_MLEN: usize = 1 << 24;
/// Window sizes of RFC 7932 (§9.1).
pub(crate) const MIN_LGWIN: u32 = 10;
pub(crate) const MAX_LGWIN: u32 = 24;

/// The encoder a quality selects. Qualities 0 to 4 compress as 4, and 7 to 11 as 6, until
/// they have encoders of their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Level {
    Q4,
    Q5,
    Q6,
}

impl Level {
    /// The level for `quality` (0 to 11; higher clamps to 11).
    pub(crate) fn from_quality(quality: u32) -> Self {
        match quality {
            0..=4 => Self::Q4,
            5 => Self::Q5,
            _ => Self::Q6,
        }
    }

    /// The quality the level compresses as.
    pub(crate) fn quality(self) -> u32 {
        match self {
            Self::Q4 => 4,
            Self::Q5 => 5,
            Self::Q6 => 6,
        }
    }
}

/// Writes the stream header for window size `lgwin` (§9.1).
pub(crate) fn write_window(w: &mut BitWriter<'_>, lgwin: u32) {
    match lgwin {
        16 => w.put(1, 0),
        17 => {
            w.put(1, 1);
            w.put(3, 0);
            w.put(3, 0);
        }
        18..=24 => {
            w.put(1, 1);
            w.put(3, lgwin - 17);
        }
        _ => {
            debug_assert!((MIN_LGWIN..16).contains(&lgwin), "lgwin {lgwin}");
            w.put(1, 1);
            w.put(3, 0);
            w.put(3, lgwin - 8);
        }
    }
}

/// The nibbles `MLEN - 1` takes: 4 to 6, the fewest that hold it (§9.2 forbids a last nibble
/// of 0 when there are more than 4).
fn nibbles(mlen: usize) -> u32 {
    match mlen - 1 {
        0..0x1_0000 => 4,
        0x1_0000..0x10_0000 => 5,
        _ => 6,
    }
}

/// Writes ISLAST (and ISLASTEMPTY = 0), MNIBBLES and MLEN; ISUNCOMPRESSED is the caller's.
fn write_length(w: &mut BitWriter<'_>, mlen: usize, is_last: bool) {
    debug_assert!((1..=MAX_MLEN).contains(&mlen), "MLEN {mlen}");
    w.put(1, u32::from(is_last));
    if is_last {
        w.put(1, 0);
    }
    let n = nibbles(mlen);
    w.put(2, n - 4);
    w.put(4 * n, (mlen - 1) as u32);
}

/// Writes the empty last meta-block: ISLAST = 1, ISLASTEMPTY = 1.
pub(crate) fn write_empty_last(w: &mut BitWriter<'_>) {
    w.put(2, 0b11);
}

/// Writes `data` as an uncompressed meta-block (§9.2), which can't be the last one.
fn write_uncompressed(w: &mut BitWriter<'_>, data: &[u8]) {
    write_length(w, data.len(), false);
    w.put(1, 1);
    w.bytes(data);
}

/// Writes `n` (1..=256) as §9.2's variable-length count: `n - 1` as 0, or 1 then its bit
/// length less one in 3 bits and its bits below the top one.
fn write_count_256(w: &mut BitWriter<'_>, n: usize) {
    debug_assert!((1..=256).contains(&n));
    let v = (n - 1) as u32;
    if v == 0 {
        w.put(1, 0);
    } else {
        let bits = v.ilog2();
        w.put(1, 1);
        w.put(3, bits);
        w.put(bits, v - (1 << bits));
    }
}

/// The block type code for each block (§6): 1 for "the last type + 1", 0 for "the type before
/// the last", else the type + 2. The first block's code is never written.
struct TypeCoder {
    last: usize,
    second: usize,
}

impl TypeCoder {
    /// Starts as the decoder does: the last type 0, the one before it 1. (The first block is
    /// fed through too, so it starts one step back.)
    fn new() -> Self {
        Self { last: 1, second: 0 }
    }

    fn code(&mut self, t: usize) -> usize {
        let code = if t == self.last + 1 {
            1
        } else if t == self.second {
            0
        } else {
            t + 2
        };
        self.second = self.last;
        self.last = t;
        code
    }
}

/// Writes a category's blocks as the decoder reads them (§6, §9.2), and its block switches
/// between symbols.
struct Blocks<'a> {
    split: &'a Split,
    /// The block now being written, and the symbols left in it.
    block: usize,
    left: u32,
    coder: TypeCoder,
    /// The prefix codes of type codes and block counts, when there is more than one type.
    switch: Option<(Code, Code)>,
}

impl<'a> Blocks<'a> {
    /// Writes NBLTYPES and, with more than one type, the switch codes and the first count.
    fn write_header(w: &mut BitWriter<'_>, split: &'a Split) -> Self {
        let types = split.num_types();
        write_count_256(w, types);
        let mut coder = TypeCoder::new();
        if types == 1 {
            return Self {
                split,
                block: 0,
                left: u32::MAX,
                coder,
                switch: None,
            };
        }
        let mut type_counts = vec![0u32; types + 2];
        let mut length_counts = [0u32; BLOCK_COUNT_BASE.len()];
        for (i, (&t, &len)) in split.types.iter().zip(&split.lengths).enumerate() {
            let code = coder.code(usize::from(t));
            if i != 0 {
                type_counts[code] += 1;
            }
            length_counts[block_count_code(len)] += 1;
        }
        let type_code = Code::from_counts(&type_counts, MAX_LENGTH);
        let length_code = Code::from_counts(&length_counts, MAX_LENGTH);
        store::store(w, &type_code, types + 2);
        store::store(w, &length_code, BLOCK_COUNT_BASE.len());
        write_block_count(w, &length_code, split.lengths[0]);
        let mut coder = TypeCoder::new();
        coder.code(usize::from(split.types[0]));
        Self {
            split,
            block: 0,
            left: split.lengths[0],
            coder,
            switch: Some((type_code, length_code)),
        }
    }

    /// The type of the next symbol, after writing a block switch if a block ends here. Always
    /// inlined: it runs per symbol, and out of line it cost 2% on large inputs.
    #[inline(always)]
    fn next(&mut self, w: &mut BitWriter<'_>) -> usize {
        if self.left == 0
            && let Some((type_code, length_code)) = &self.switch
        {
            self.block += 1;
            let t = usize::from(self.split.types[self.block]);
            let code = self.coder.code(t);
            w.put(
                u32::from(type_code.lengths[code]),
                u32::from(type_code.codes[code]),
            );
            self.left = self.split.lengths[self.block];
            write_block_count(w, length_code, self.left);
        }
        self.left -= 1;
        usize::from(self.split.types[self.block])
    }

    /// The type of the next `wanted` symbols (at least 1), after writing a block switch if a
    /// block ends here, and how many of them fit before the next switch: as many calls of
    /// [`next`](Self::next) would give, one per run.
    #[inline(always)]
    fn run(&mut self, w: &mut BitWriter<'_>, wanted: usize) -> (usize, usize) {
        let t = self.next(w);
        let more = (self.left as usize).min(wanted - 1);
        self.left -= more as u32;
        (t, more + 1)
    }
}

/// The block count code for `len` (§6).
fn block_count_code(len: u32) -> usize {
    BLOCK_COUNT_BASE.partition_point(|&b| b <= len) - 1
}

fn write_block_count(w: &mut BitWriter<'_>, code: &Code, len: u32) {
    let c = block_count_code(len);
    w.put(u32::from(code.lengths[c]), u32::from(code.codes[c]));
    w.put(u32::from(BLOCK_COUNT_EXTRA[c]), len - BLOCK_COUNT_BASE[c]);
}

/// Writes NTREES and the context map for `types` block types of `1 << context_bits`
/// contexts each, every context of a type on that type's own code (§7.3). As the reference's
/// `StoreTrivialContextMap`: after move-to-front each type's contexts are its index then
/// zeros, one zero run each.
fn write_trivial_context_map(w: &mut BitWriter<'_>, types: usize, context_bits: u32) {
    write_count_256(w, types);
    if types == 1 {
        return;
    }
    let rle_max = context_bits - 1;
    w.put(1, 1);
    w.put(4, rle_max - 1);
    let alphabet = types + rle_max as usize;
    let mut counts = vec![0u32; alphabet];
    counts[0] = 1;
    counts[rle_max as usize] = types as u32;
    counts[rle_max as usize + 1..].fill(1);
    let code = Code::from_counts(&counts, MAX_LENGTH);
    store::store(w, &code, alphabet);
    let put = |w: &mut BitWriter<'_>, s: usize| {
        w.put(u32::from(code.lengths[s]), u32::from(code.codes[s]));
    };
    for t in 0..types {
        put(w, if t == 0 { 0 } else { t + rle_max as usize });
        // The longest run the code says: 2^rle_max + (2^rle_max - 1) zeros, the rest.
        put(w, rle_max as usize);
        w.put(rle_max, (1 << rle_max) - 1);
    }
    w.put(1, 1); // IMTF
}

/// Writes NTREES and `map`, the context map of a category, whose values are below `trees`
/// (§7.3), as the reference's `EncodeContextMap`: move-to-front, runs of zeros coded with
/// prefixes up to 6, a code for the result, then the symbols.
pub(crate) fn write_context_map(w: &mut BitWriter<'_>, map: &[u8], trees: usize) {
    write_count_256(w, trees);
    if trees == 1 {
        return;
    }
    // Move-to-front: each value as its index in a list of the values, most recent first.
    let mut order: Vec<u8> = (0..=255).collect();
    let moved: Vec<u32> = map
        .iter()
        .map(|&v| {
            let mut i = 0;
            while order[i] != v {
                i += 1;
            }
            order.copy_within(..i, 1);
            order[0] = v;
            i as u32
        })
        .collect();
    // Zero runs: a run of `n` is prefix code `log2(n)` and its bits below the top one, while
    // `n < 2 << rle_max`; a longer run takes `rle_max` with all its bits set, the most one
    // symbol says, and goes on. Other values move up by `rle_max`.
    let mut longest = 0u32;
    let mut run = 0;
    for &v in &moved {
        run = if v == 0 { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let rle_max = if longest == 0 {
        0
    } else {
        longest.ilog2().min(6)
    };
    let mut symbols: Vec<(u32, u32)> = Vec::with_capacity(moved.len());
    let mut i = 0;
    while i < moved.len() {
        if moved[i] != 0 {
            symbols.push((moved[i] + rle_max, 0));
            i += 1;
            continue;
        }
        let mut reps = moved[i..].iter().take_while(|&&v| v == 0).count() as u32;
        i += reps as usize;
        while reps >= 2 << rle_max {
            symbols.push((rle_max, (1 << rle_max) - 1));
            reps -= (2 << rle_max) - 1;
        }
        let prefix = reps.ilog2();
        symbols.push((prefix, reps - (1 << prefix)));
    }
    w.put(1, u32::from(rle_max > 0));
    if rle_max > 0 {
        w.put(4, rle_max - 1);
    }
    let alphabet = trees + rle_max as usize;
    let mut counts = vec![0u32; alphabet];
    for &(s, _) in &symbols {
        counts[s as usize] += 1;
    }
    let code = Code::from_counts(&counts, MAX_LENGTH);
    store::store(w, &code, alphabet);
    for (s, extra) in symbols {
        let s = s as usize;
        w.put(u32::from(code.lengths[s]), u32::from(code.codes[s]));
        if (1..=rle_max as usize).contains(&s) {
            w.put(s as u32, extra);
        }
    }
    w.put(1, 1); // IMTF
}

/// A meta-block's literals, and how they are coded: by `model`, and with more than one
/// context, each literal's folded context in `contexts`.
struct Literals<'a> {
    bytes: &'a [u8],
    model: Model,
    contexts: Vec<u8>,
}

impl<'a> Literals<'a> {
    /// The literals `bytes` of the meta-block `data`, whose inserts and copies are
    /// `commands`, coded by `model`. A literal's context is the two bytes before it in the
    /// stream: `prev` holds the two before `data` (the last first, 0 at the stream's start).
    fn new(
        bytes: &'a [u8],
        data: &[u8],
        prev: [u8; 2],
        commands: &[Command],
        model: Model,
    ) -> Self {
        let mut contexts = Vec::new();
        if model.count > 1 {
            contexts.reserve(bytes.len());
            let before = |i: usize, back: usize| {
                if i >= back {
                    data[i - back]
                } else {
                    prev[back - i - 1]
                }
            };
            let mut pos = 0;
            for c in commands {
                let insert = c.insert as usize;
                contexts
                    .extend((pos..pos + insert).map(|i| model.context(before(i, 1), before(i, 2))));
                pos += insert + c.copied();
            }
            debug_assert_eq!(pos, data.len(), "the commands cover the meta-block");
        }
        Self {
            bytes,
            model,
            contexts,
        }
    }
}

/// The three categories' greedy block splits for a meta-block of `commands` and `literals`;
/// the literals' with a histogram per context.
fn plan(literals: &Literals<'_>, commands: &[Command], coded: &[Coded]) -> [Split; 3] {
    let mut l = metablock::Splitter::literals(literals.model.count);
    let mut c = metablock::Splitter::commands();
    let mut d = metablock::Splitter::distances();
    let mut next = 0;
    for (cmd, k) in commands.iter().zip(coded) {
        c.add(k.code as usize);
        let range = next..next + cmd.insert as usize;
        if literals.contexts.is_empty() {
            for &b in &literals.bytes[range] {
                l.add(b as usize);
            }
        } else {
            for (&b, &ctx) in literals.bytes[range.clone()]
                .iter()
                .zip(&literals.contexts[range])
            {
                l.add_in(b as usize, usize::from(ctx));
            }
        }
        next += cmd.insert as usize;
        if k.reads_distance(cmd) {
            d.add(cmd.distance.code as usize);
        }
    }
    [l.finish(), c.finish(), d.finish()]
}

/// Writes a compressed meta-block of `mlen` bytes: `commands`, whose inserts take `literals`
/// in order, with block types as `splits` (literals, commands, distances) give them.
/// `NPOSTFIX = 0` and `NDIRECT = 0`; each literal block type has a code per context of its
/// model, and each distance block type one for its four. `BY_CONTEXT` is whether the model
/// has more than one context: each case gets its own copy of the per-literal loop, so that
/// quality 4's stays as tight as before contexts.
fn write_compressed<const BY_CONTEXT: bool>(
    w: &mut BitWriter<'_>,
    mlen: usize,
    literals: &Literals<'_>,
    commands: &[Command],
    coded: &[Coded],
    splits: &mut [Split; 3],
    is_last: bool,
) {
    // Counts tuned for run-length coded lengths, then the codes (§3.5), per block type.
    let mut codes: [Vec<Code>; 3] = Default::default();
    for (split, codes) in splits.iter_mut().zip(&mut codes) {
        let contexts = split.contexts;
        for counts in &mut split.counts {
            let alphabet = counts.len() / contexts;
            for counts in counts.chunks_exact_mut(alphabet) {
                histogram::optimize_for_rle(counts);
                codes.push(Code::from_counts(counts, MAX_LENGTH));
            }
        }
    }
    let [literal_split, command_split, distance_split] = &*splits;
    let [literal_codes, command_codes, distance_codes] = &codes;
    // Each literal code's length and bits in one word, by byte: one load per literal, and no
    // bounds check.
    let literal_words: Vec<[u32; 256]> = literal_codes
        .iter()
        .map(|code| {
            std::array::from_fn(|b| u32::from(code.lengths[b]) << 16 | u32::from(code.codes[b]))
        })
        .collect();

    write_length(w, mlen, is_last);
    if !is_last {
        w.put(1, 0); // ISUNCOMPRESSED
    }
    let mut literal_blocks = Blocks::write_header(w, literal_split);
    let mut command_blocks = Blocks::write_header(w, command_split);
    let mut distance_blocks = Blocks::write_header(w, distance_split);
    w.put(2, 0); // NPOSTFIX
    w.put(4, 0); // NDIRECT >> NPOSTFIX
    let n = literals.model.count;
    for _ in 0..literal_split.num_types() {
        // With one code per type any mode would do: LSB6, as quality 4 always wrote.
        w.put(2, if n > 1 { u32::from(context::UTF8) } else { 0 });
    }
    if n > 1 {
        // Each type's contexts on its own `n` codes, folded by the model's map.
        let map: Vec<u8> = (0..literal_split.num_types())
            .flat_map(|t| literals.model.map.iter().map(move |&c| (t * n) as u8 + c))
            .collect();
        write_context_map(w, &map, literal_codes.len());
    } else {
        write_trivial_context_map(w, literal_split.num_types(), 6);
    }
    write_trivial_context_map(w, distance_split.num_types(), 2);
    for code in literal_codes {
        store::store(w, code, LITERALS);
    }
    for code in command_codes {
        store::store(w, code, COMMANDS);
    }
    for code in distance_codes {
        store::store(w, code, DISTANCES);
    }

    let mut next = 0;
    for (c, k) in commands.iter().zip(coded) {
        let code = &command_codes[command_blocks.next(w)];
        let s = k.code as usize;
        w.put(u32::from(code.lengths[s]), u32::from(code.codes[s]));
        w.put(
            u32::from(INSERT_EXTRA[k.insert_code as usize]),
            c.insert - INSERT_BASE[k.insert_code as usize],
        );
        if c.copy != 0 {
            w.put(
                u32::from(COPY_EXTRA[k.copy_code as usize]),
                c.copy - COPY_BASE[k.copy_code as usize],
            );
        } else {
            w.put(u32::from(COPY_EXTRA[0]), 0);
        }
        let mut rest = &literals.bytes[next..next + c.insert as usize];
        let mut at = next;
        while !rest.is_empty() {
            let (t, run) = literal_blocks.run(w, rest.len());
            let (now, later) = rest.split_at(run);
            if BY_CONTEXT {
                let words = &literal_words[t * n..(t + 1) * n];
                for (&b, &ctx) in now.iter().zip(&literals.contexts[at..at + run]) {
                    let word = words[usize::from(ctx)][usize::from(b)];
                    w.put(word >> 16, word & 0xffff);
                }
            } else {
                let words = &literal_words[t];
                for &b in now {
                    let word = words[usize::from(b)];
                    w.put(word >> 16, word & 0xffff);
                }
            }
            at += run;
            rest = later;
        }
        next += c.insert as usize;
        if k.reads_distance(c) {
            let code = &distance_codes[distance_blocks.next(w)];
            let d = c.distance.code as usize;
            w.put(u32::from(code.lengths[d]), u32::from(code.codes[d]));
            w.put(u32::from(c.distance.extra_bits), c.distance.extra);
        }
    }
    debug_assert_eq!(next, literals.bytes.len(), "the inserts take every literal");
}

/// Writes the meta-block holding `data`: compressed as `commands` and `literals`, the
/// literals coded by `model`, or uncompressed if that is smaller (then, if `is_last`, an empty
/// last meta-block follows). `prev` holds the two bytes before `data`, the last first (0 at
/// the stream's start). Returns whether it was written compressed: an uncompressed meta-block
/// leaves the decoder's distance ring as it was, and the caller's has to match.
pub(crate) fn write_meta_block(
    w: &mut BitWriter<'_>,
    data: &[u8],
    prev: [u8; 2],
    model: Model,
    literals: &[u8],
    commands: &[Command],
    is_last: bool,
) -> bool {
    let mark = w.mark();
    let offset = w.bit_offset();
    let coded: Vec<Coded> = commands.iter().map(Coded::new).collect();
    let literals = Literals::new(literals, data, prev, commands, model);
    let mut splits = plan(&literals, commands, &coded);
    let write = if literals.model.count > 1 {
        write_compressed::<true>
    } else {
        write_compressed::<false>
    };
    write(
        w,
        data.len(),
        &literals,
        commands,
        &coded,
        &mut splits,
        is_last,
    );
    let round = |bits: usize| bits.div_ceil(8) * 8;
    // Both counted from the byte `mark` is in; a last meta-block ends with padding to a byte.
    let mut compressed = offset + w.bits_since(mark);
    if is_last {
        compressed = round(compressed);
    }
    let header = 1 + 2 + 4 * nibbles(data.len()) as usize + 1;
    let uncompressed = round(offset + header) + 8 * data.len() + if is_last { 8 } else { 0 };
    if compressed <= uncompressed {
        return true;
    }
    w.rollback(mark);
    write_uncompressed(w, data);
    if is_last {
        write_empty_last(w);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::command::DistanceCode;
    use super::matcher::compress;
    use super::{Command, Level, write_meta_block, write_window};
    use crate::bitwriter::BitWriter;

    fn decompress(stream: &[u8]) -> Vec<u8> {
        crate::decompress(stream).unwrap()
    }

    #[test]
    fn levels_follow_the_quality() {
        for quality in 0..=4 {
            assert_eq!(Level::from_quality(quality), Level::Q4, "quality {quality}");
        }
        assert_eq!(Level::from_quality(5), Level::Q5);
        for quality in [6, 7, 9, 11, 12, u32::MAX] {
            assert_eq!(Level::from_quality(quality), Level::Q6, "quality {quality}");
        }
    }

    #[test]
    fn every_block_count_has_its_code() {
        use crate::tables::{BLOCK_COUNT_BASE, BLOCK_COUNT_EXTRA};
        for len in 1..40_000u32 {
            let c = super::block_count_code(len);
            let (base, extra) = (BLOCK_COUNT_BASE[c], BLOCK_COUNT_EXTRA[c]);
            assert!(
                base <= len && len - base < 1 << extra,
                "{len}: code {c} (from {base}, {extra} extra bits)"
            );
        }
    }

    #[test]
    fn every_window_size_is_read_back() {
        for lgwin in 10..=24 {
            let mut out = Vec::new();
            compress(b"window", lgwin, Level::Q4, &mut out);
            assert_eq!(decompress(&out), b"window", "lgwin {lgwin}");
            // The header alone, then an empty last meta-block.
            let mut header = Vec::new();
            let mut w = BitWriter::new(&mut header);
            write_window(&mut w, lgwin);
            w.put(2, 0b11);
            w.finish();
            assert_eq!(decompress(&header), b"", "lgwin {lgwin}");
        }
    }

    #[test]
    fn the_empty_stream_is_one_byte() {
        let mut out = Vec::new();
        compress(b"", 16, Level::Q4, &mut out);
        assert_eq!(out, [0x06]);
        assert_eq!(decompress(&out), b"");
    }

    /// Writes `data` as one meta-block of `commands` and returns the stream.
    fn stream(data: &[u8], commands: &[Command]) -> (Vec<u8>, bool) {
        let mut literals = Vec::new();
        let mut at = 0;
        for c in commands {
            literals.extend_from_slice(&data[at..at + c.insert as usize]);
            at += (c.insert + c.copy) as usize;
        }
        assert_eq!(at, data.len());
        let mut out = Vec::new();
        let mut w = BitWriter::new(&mut out);
        write_window(&mut w, 22);
        let compressed = write_meta_block(
            &mut w,
            data,
            [0, 0],
            super::Model::ONE,
            &literals,
            commands,
            true,
        );
        w.finish();
        (out, compressed)
    }

    fn command(insert: u32, copy: u32, distance: DistanceCode) -> Command {
        Command {
            insert,
            copy,
            distance,
            cut: 0,
        }
    }

    #[test]
    fn copies_at_explicit_and_last_distances() {
        // "abcabcabc...": insert 3, copy from 3 back; then the last distance again, in cell 0
        // (no distance written), and after an insert of 6, too long for cells 0 and 1, as a
        // written distance code 0.
        let data: Vec<u8> = b"abc"
            .iter()
            .copied()
            .cycle()
            .take(3 + 20 + 7 + 6 + 9 + 3)
            .collect();
        let commands = [
            command(3, 20, DistanceCode::explicit(3)),
            command(0, 7, DistanceCode::LAST),
            command(6, 9, DistanceCode::LAST),
            command(3, 0, DistanceCode::LAST),
        ];
        let (out, compressed) = stream(&data, &commands);
        assert!(compressed);
        assert_eq!(decompress(&out), data);
    }

    /// Greek text: two-byte letters, so a literal's context says much about it.
    fn greek(len: usize) -> Vec<u8> {
        let words = [
            "καλημέρα ",
            "κόσμε, ",
            "λόγος ",
            "ήλιος. ",
            "θάλασσα ",
            "ουρανός ",
        ];
        let mut x = 3u32;
        let mut out = Vec::new();
        while out.len() < len {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            out.extend_from_slice(words[(x >> 16) as usize % words.len()].as_bytes());
        }
        out.truncate(len);
        out
    }

    #[test]
    fn literals_coded_by_context_read_back_after_the_bytes_before() {
        use super::context::{COMPLEX, Model, SIMPLE};
        // An uncompressed meta-block ending inside a letter, then a compressed one starting
        // with its continuation byte: the first literals' contexts are the bytes before.
        let mut all = greek(6000);
        let cut = (1000..2000).find(|&i| all[i - 1] >= 0xc0).unwrap();
        all.copy_within(cut + 500..cut + 1200, cut + 2000);
        let (prefix, data) = all.split_at(cut);
        // Literals, a copy (whose last bytes give the next literals' context), literals.
        let commands = [
            command(2000, 700, DistanceCode::explicit(1500)),
            command(data.len() as u32 - 2700, 0, DistanceCode::LAST),
        ];
        let literals = [&data[..2000], &data[2700..]].concat();
        let mut sizes = Vec::new();
        for (count, map) in [(1, &[0; 64]), (2, &SIMPLE), (13, &COMPLEX)] {
            let model = Model { count, map };
            let mut out = Vec::new();
            let mut w = BitWriter::new(&mut out);
            write_window(&mut w, 22);
            super::write_uncompressed(&mut w, prefix);
            let prev = [prefix[cut - 1], prefix[cut - 2]];
            assert!(write_meta_block(
                &mut w, data, prev, model, &literals, &commands, true
            ));
            w.finish();
            assert_eq!(decompress(&out), all, "{count} contexts");
            sizes.push(out.len());
        }
        assert!(sizes[1] < sizes[0], "contexts pay on Greek text: {sizes:?}");
    }

    #[test]
    fn long_inserts_and_copies_use_every_cell() {
        // Inserts and copies from each length code's range, at long distances, so cells 2-10
        // all appear.
        let mut data = Vec::new();
        let mut commands = Vec::new();
        let mut seed = 7u32;
        for (insert, copy) in [
            (1, 2),
            (9, 3),
            (30, 12),
            (100, 70),
            (2000, 300),
            (7000, 2200),
            (23_000, 10),
            (5, 9000),
            (40_000, 20_000),
            (0, 2),
        ] {
            for _ in 0..insert {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                data.push((seed >> 16) as u8);
            }
            let distance = (data.len() as u32).min(1000 + insert);
            for _ in 0..copy {
                data.push(data[data.len() - distance as usize]);
            }
            commands.push(command(insert, copy, DistanceCode::explicit(distance)));
        }
        let (out, _) = stream(&data, &commands);
        assert_eq!(decompress(&out), data);
    }

    #[test]
    fn a_larger_compressed_block_is_stored_instead() {
        // Random bytes: the literal code costs more than it saves.
        let mut seed = 99u32;
        let data: Vec<u8> = (0..5000)
            .map(|_| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (seed >> 16) as u8
            })
            .collect();
        let (out, compressed) = stream(&data, &[command(5000, 0, DistanceCode::LAST)]);
        assert!(!compressed);
        // The window (4 bits) and meta-block header (1 + 2 + 16 + 1 bits) in 3 bytes, the
        // data, and the empty last meta-block.
        assert_eq!(out.len(), 3 + 5000 + 1);
        assert_eq!(decompress(&out), data);
    }

    #[test]
    fn literal_streams_round_trip() {
        let text = b"the quick brown fox jumps over the lazy dog. ".repeat(4000);
        for len in [1, 2, 100, 65_535, 65_536, 65_537, text.len()] {
            let mut out = Vec::new();
            compress(&text[..len], 22, Level::Q4, &mut out);
            assert_eq!(decompress(&out), &text[..len], "{len} bytes");
            if len > 1000 {
                assert!(out.len() < len * 3 / 4, "{len} bytes: {}", out.len());
            }
        }
    }
}
