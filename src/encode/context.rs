//! Literal context modeling (RFC 7932 §7): from quality 5, a meta-block's literals may be
//! coded by context, each literal with a code picked by the two bytes before it, as the
//! reference decides (`c/enc/encode.c`, `DecideOverLiteralContextModeling`).
//!
//! The context of a literal is the UTF-8 one of §7.1 (mode 2): 64 values, by the class of the
//! byte before it (`p1`: a control byte, a newline, a space, groups of punctuation, a digit, an
//! upper- or lower-case vowel or consonant) and of the byte before that (`p2`). A static map
//! folds the 64 into a few: 2 for text with many multi-byte UTF-8 letters, or 13 for long
//! inputs. The block splitter then keeps one histogram per (block type, folded context).

use crate::context;

/// The context mode the maps are written for: UTF-8 (§7.1).
pub(crate) const UTF8: u8 = 2;

/// The two-context map: context 1 after a UTF-8 lead byte (`p1` from 0xc0), where the next
/// byte is a continuation byte; context 0 everywhere else. (Context values 2 and 3 are the
/// lead bytes, `LUT0` 2 and 3, with `p2` of class 0.)
pub(crate) static SIMPLE: [u8; 64] = {
    let mut map = [0; 64];
    map[2] = 1;
    map[3] = 1;
    map
};

/// The 13-context map. Each row is a class of `p1`, by `LUT0` (the context value's top four
/// bits); its four entries are by the class of `p2`, by `LUT1`: 0 a control byte, a space or
/// not ASCII, 1 punctuation, 2 a digit or an upper-case letter, 3 a lower-case letter. (In row
/// 0, where `p1` isn't ASCII or is a control byte, the column is `LUT0` of `p1` `OR`ed with
/// `LUT1` of `p2`: 0 or 1 for a continuation byte, 2 or 3 for a lead byte, and 2 or 3 as well
/// for a continuation byte after the lead byte of a three- or four-byte letter.) The folded contexts: 0 a line start, 1 after a space or an opening
/// bracket or quote, 2 after other punctuation, 3 in a number, 4 after a comma, colon or
/// semicolon or a period ending a word, 5 in or after an upper-case word, 6 in a lower-case
/// word, 7 after a period following punctuation, 8 after punctuation at a line or word start,
/// 9 after a space following a letter or digit, 10 after an upper-case letter following
/// another or a digit, 11 after a complete UTF-8 letter (or a control byte), 12 where a UTF-8
/// continuation byte comes next.
pub(crate) static COMPLEX: [u8; 64] = [
    11, 11, 12, 12, // not ASCII, or a control byte
    0, 0, 0, 0, // a newline, tab or carriage return
    1, 1, 9, 9, // a space
    2, 2, 2, 2, // ! # $ & * + - / ? @ \ ^ _ ` | ~
    1, 1, 1, 1, // " '
    8, 3, 3, 3, // %
    1, 1, 1, 1, // ( < [ {
    2, 2, 2, 2, // ) > ] }
    8, 4, 4, 4, // , : ;
    8, 7, 4, 4, // .
    8, 0, 0, 0, // =
    3, 3, 3, 3, // a digit
    5, 5, 10, 5, // an upper-case vowel
    5, 5, 10, 5, // an upper-case consonant
    6, 6, 6, 6, // a lower-case vowel
    6, 6, 6, 6, // a lower-case consonant
];

/// How a meta-block's literals are coded: `count` contexts, folded from the 64 by `map`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Model {
    pub(crate) count: usize,
    pub(crate) map: &'static [u8; 64],
}

impl Model {
    /// One context: a code per block type, as quality 4 writes.
    pub(crate) const ONE: Self = Self {
        count: 1,
        map: &[0; 64],
    };

    /// The folded context of a literal after `p1`, which came after `p2`.
    #[inline(always)]
    pub(crate) fn context(&self, p1: u8, p2: u8) -> u8 {
        self.map[context::literal(UTF8, p1, p2)]
    }
}

/// Inputs this large may use the 13 contexts.
const COMPLEX_HINT: usize = 1 << 20;
/// The data is sampled in strides of this many bytes...
const STRIDE: usize = 64;
/// ...one every this many.
const STRIDE_EVERY: usize = 4096;
/// Bits per literal the contexts must save to be used.
const MIN_GAIN: f64 = 0.2;
/// With 13 contexts, the bits per byte of the top 5 bits above which the data is taken for
/// barely compressible, and the contexts not worth their cost.
const MAX_COMPLEX_ENTROPY: f64 = 3.0;

/// The model for the literals of `data`, a meta-block of a stream of about `size_hint` bytes
/// (0 if not known), as the reference decides from quality 5 (`ChooseContextMap`,
/// `ShouldUseComplexStaticContextMap`). The three-context map of qualities 7 and up isn't
/// here: below 7 the reference never picks it.
pub(crate) fn decide(data: &[u8], size_hint: usize) -> Model {
    if data.len() < STRIDE {
        return Model::ONE;
    }
    if size_hint >= COMPLEX_HINT && complex_pays(complex_entropy(data)) {
        return Model {
            count: 13,
            map: &COMPLEX,
        };
    }
    if simple_pays(simple_gain(data)) {
        Model {
            count: 2,
            map: &SIMPLE,
        }
    } else {
        Model::ONE
    }
}

/// The strides sampled: `STRIDE` bytes every `STRIDE_EVERY`, from the start.
fn strides(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    (0..)
        .map(|k| k * STRIDE_EVERY)
        .take_while(move |&s| s + STRIDE <= data.len())
        .map(move |s| &data[s..s + STRIDE])
}

/// Shannon's bits for `counts` (no floor of a bit per symbol, unlike the block splitter's:
/// what the context saves is coded in the byte's other bits).
fn entropy(counts: &[u32]) -> f64 {
    let total: u32 = counts.iter().sum();
    let log2 = |c: u32| if c == 0 { 0.0 } else { f64::from(c).log2() };
    f64::from(total) * log2(total) - counts.iter().map(|&c| f64::from(c) * log2(c)).sum::<f64>()
}

/// The bits per byte of the top 5 bits of the sampled bytes, alone and given their context
/// in the 13 (each stride's first two bytes are only context).
fn complex_entropy(data: &[u8]) -> (f64, f64) {
    let mut combined = [0u32; 32];
    let mut by_context = [[0u32; 32]; 13];
    let mut total = 0u32;
    for stride in strides(data) {
        let (mut p2, mut p1) = (stride[0], stride[1]);
        for &b in &stride[2..] {
            let c = COMPLEX[context::literal(UTF8, p1, p2)];
            total += 1;
            combined[usize::from(b >> 3)] += 1;
            by_context[usize::from(c)][usize::from(b >> 3)] += 1;
            (p2, p1) = (p1, b);
        }
    }
    let plain = entropy(&combined) / f64::from(total);
    let with = by_context.iter().map(|h| entropy(h)).sum::<f64>() / f64::from(total);
    (plain, with)
}

/// Whether the 13 contexts pay, given [`complex_entropy`]: they save enough, and the data
/// isn't too random for them.
fn complex_pays((plain, with): (f64, f64)) -> bool {
    with <= MAX_COMPLEX_ENTROPY && plain - with >= MIN_GAIN
}

/// The bits per byte that two contexts save on the top 2 bits of the sampled bytes. Each
/// byte is counted in a pair with the byte before, by class: 0 ASCII, 1 a continuation byte,
/// 2 a lead byte. Two contexts split the pairs by index modulo 6, as the reference does:
/// those after a continuation byte apart from those after ASCII or a lead byte.
fn simple_gain(data: &[u8]) -> f64 {
    let class = |b: u8| [0, 0, 1, 2][usize::from(b >> 6)];
    let mut pairs = [0u32; 9];
    for stride in strides(data) {
        let mut prev = class(stride[0]) * 3;
        for &b in &stride[1..] {
            pairs[prev + class(b)] += 1;
            prev = class(b) * 3;
        }
    }
    let mut alone = [0u32; 3];
    let mut after = [0u32; 6];
    for (i, &n) in pairs.iter().enumerate() {
        alone[i % 3] += n;
        after[i % 6] += n;
    }
    let total: u32 = alone.iter().sum();
    let plain = entropy(&alone) / f64::from(total);
    let split = (entropy(&after[..3]) + entropy(&after[3..])) / f64::from(total);
    plain - split
}

/// Whether two contexts pay, given [`simple_gain`].
fn simple_pays(gain: f64) -> bool {
    gain >= MIN_GAIN
}

#[cfg(test)]
mod tests {
    use super::{COMPLEX, Model, SIMPLE, decide};
    use crate::context;

    /// The folded context of the byte after `p2`, `p1`.
    fn complex(p1: u8, p2: u8) -> u8 {
        COMPLEX[context::literal(super::UTF8, p1, p2)]
    }

    #[test]
    fn the_simple_map_marks_what_follows_a_lead_byte() {
        // "é" is 0xc3 0xa9: the continuation byte follows a lead byte.
        assert_eq!(SIMPLE[context::literal(super::UTF8, 0xc3, b' ')], 1);
        assert_eq!(SIMPLE[context::literal(super::UTF8, 0xc3, b'a')], 1);
        assert_eq!(SIMPLE[context::literal(super::UTF8, 0xa9, 0xc3)], 0);
        assert_eq!(SIMPLE[context::literal(super::UTF8, b'a', b'b')], 0);
        assert_eq!(SIMPLE.iter().map(|&c| u32::from(c)).sum::<u32>(), 2);
    }

    #[test]
    fn every_complex_entry_is_its_class() {
        // A byte of each `p1` row, and of each `p2` class (control, punctuation, digit or
        // upper case, lower case), with what the context means.
        let p2s = *b" ,Zz";
        let rows: [(u8, [u8; 4], &str); 16] = [
            (
                0x01,
                [11, 11, 12, 12],
                "control: inside a UTF-8 sequence (see below)",
            ),
            (b'\n', [0; 4], "a line start"),
            (b' ', [1, 1, 9, 9], "after a space: 9 if it follows a word"),
            (b'!', [2; 4], "after other punctuation"),
            (b'"', [1; 4], "after a quote: a word starts"),
            (
                b'%',
                [8, 3, 3, 3],
                "after %: in a number, unless at a word start",
            ),
            (b'(', [1; 4], "after an opening bracket: a word starts"),
            (b')', [2; 4], "after a closing bracket"),
            (b',', [8, 4, 4, 4], "after , : ;"),
            (
                b'.',
                [8, 7, 4, 4],
                "after a period: 7 after punctuation, 4 ending a word",
            ),
            (b'=', [8, 0, 0, 0], "after = : a value starts"),
            (b'7', [3; 4], "in a number"),
            (
                b'E',
                [5, 5, 10, 5],
                "after an upper-case vowel: 10 in an upper-case word",
            ),
            (
                b'K',
                [5, 5, 10, 5],
                "after an upper-case consonant: 10 in an upper-case word",
            ),
            (b'e', [6; 4], "in a lower-case word, after a vowel"),
            (b'k', [6; 4], "in a lower-case word, after a consonant"),
        ];
        for (row, (p1, want, why)) in rows.iter().enumerate() {
            for (col, &p2) in p2s.iter().enumerate() {
                if row == 0 {
                    continue;
                }
                assert_eq!(complex(*p1, p2), want[col], "{why}: p1 {p1:#x} p2 {p2:#x}");
                assert_eq!(context::literal(super::UTF8, *p1, p2), row * 4 + col);
            }
        }
        // Row 0 ORs the two classes: `LUT0` of a continuation byte is 0 or 1, of a lead byte
        // 2 or 3; `LUT1` of a lead byte of three or four bytes is 2. So 12 is where a
        // continuation byte comes next, and 11 where a letter is complete.
        assert_eq!(complex(0xa9, 0xc3), 11, "é ends");
        assert_eq!(complex(0x82, 0xe2), 12, "the middle of '€' (e2 82 ac)");
        assert_eq!(complex(0xac, 0x82), 11, "'€' ends");
        assert_eq!(complex(0xc3, 0xa9), 12, "a lead byte after a letter");
        assert_eq!(complex(0xc3, b' '), 12, "a lead byte after a space");
        // After an ASCII letter the classes mix: a continuation byte ORed with a lower-case
        // `p2` (3) lands in 12 as well.
        assert_eq!(complex(0xa9, b'a'), 12);
        assert_eq!(complex(0x01, b','), 11, "a control byte after punctuation");
        assert_eq!(COMPLEX.iter().copied().max(), Some(12));
    }

    fn text(len: usize, words: &[&str]) -> Vec<u8> {
        let mut x = 7u32;
        let mut out = Vec::new();
        while out.len() < len {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            out.extend_from_slice(words[x as usize % words.len()].as_bytes());
            out.push(b' ');
        }
        out.truncate(len);
        out
    }

    #[test]
    fn random_bytes_and_ascii_text_get_one_context() {
        let mut x = 1u64;
        let random: Vec<u8> = (0..100_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect();
        assert_eq!(decide(&random, 0), Model::ONE);
        assert_eq!(decide(&random, 1 << 20), Model::ONE, "nothing to predict");
        let ascii = text(100_000, &["the", "brotli", "context", "of", "words"]);
        assert_eq!(decide(&ascii, 0), Model::ONE);
        assert_eq!(decide(&ascii[..63], 1 << 20), Model::ONE, "under 64 bytes");
    }

    #[test]
    fn text_of_two_byte_letters_gets_two_contexts() {
        let greek = text(100_000, &["καλημέρα", "κόσμε", "λόγος", "ήλιος"]);
        assert_eq!(decide(&greek, 0).count, 2);
        // Mixed with ASCII words it still pays.
        let mixed = text(100_000, &["καλημέρα", "the", "κόσμε", "of"]);
        assert_eq!(decide(&mixed, 0).count, 2);
    }

    /// 64 bytes of Greek, Russian, Chinese, ASCII and punctuation, every 4096 bytes, with
    /// ASCII between.
    fn sampled() -> (Vec<u8>, Vec<u8>) {
        let stride =
            "Καλή μέρα, κόσμε! Привет, мир. 你好 (OK) 2026; e.g. x".as_bytes()[..64].to_vec();
        let filler: Vec<u8> = b"0123456789abcdefghij"
            .iter()
            .copied()
            .cycle()
            .take(4096 - 64)
            .collect();
        let data = [&stride[..], &filler[..]].concat().repeat(3);
        (stride, data)
    }

    #[test]
    fn only_the_strides_are_measured() {
        // The values the reference's formulas give for this input (computed apart from this
        // code): only the 64 bytes at every 4096 are sampled, so the filler counts for
        // nothing.
        let (stride, data) = sampled();
        for d in [&stride, &data] {
            let gain = super::simple_gain(d);
            assert!((gain - 0.321_718_522_742_890).abs() < 1e-12, "{gain}");
            let (plain, with) = super::complex_entropy(d);
            assert!((plain - 3.332_283_311_800_921).abs() < 1e-12, "{plain}");
            assert!((with - 1.994_481_336_235_82).abs() < 1e-12, "{with}");
        }
        // 64 bytes are enough to decide on; 63 aren't.
        assert_eq!(decide(&stride, 0).count, 2);
        assert_eq!(decide(&stride[..63], 0), Model::ONE);
    }

    #[test]
    fn contexts_must_save_a_fifth_of_a_bit() {
        use super::{complex_pays, simple_pays};
        assert!(simple_pays(0.2));
        assert!(!simple_pays(0.15));
        assert!(complex_pays((3.2, 3.0)));
        assert!(!complex_pays((3.15, 3.0)), "too little saved");
        assert!(
            !complex_pays((3.5, 3.000_001)),
            "too random for 13 contexts"
        );
        assert!(complex_pays((2.0, 1.0)));
    }

    #[test]
    fn long_ascii_text_gets_thirteen_contexts() {
        let english = text(
            2 << 20,
            &[
                "The",
                "quick, brown",
                "fox.",
                "Jumps (over)",
                "the lazy",
                "dog;",
                "42%",
            ],
        );
        assert_eq!(decide(&english, 2 << 20).count, 13);
        assert_eq!(decide(&english, 1 << 20).count, 13);
        assert_eq!(
            decide(&english, (1 << 20) - 1),
            Model::ONE,
            "only from 1 MiB"
        );
    }
}
