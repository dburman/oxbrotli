//! Writing prefix codes into the stream (RFC 7932 §3.4, §3.5).

use crate::bitwriter::BitWriter;
use crate::encode::huffman::{Code, canonical, limited_lengths};

/// The order code-length code lengths are stored in (§3.5).
const ORDER: [usize; 18] = [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// The fixed code for code-length code lengths 0..=5 (§3.5): `(bits, count)`, written as
/// they appear, least significant bit first.
const FIXED: [(u32, u32); 6] = [
    (0b00, 2),
    (0b0111, 4),
    (0b011, 3),
    (0b10, 2),
    (0b01, 2),
    (0b1111, 4),
];

/// Bits needed for any symbol of an alphabet of `size` (§3.4).
fn alphabet_bits(size: usize) -> u32 {
    usize::BITS - (size - 1).leading_zeros()
}

/// Writes `code`, over an alphabet of `alphabet_size` symbols, as a simple prefix code when
/// it uses four symbols or fewer, else as a complex one. A code that uses no symbol is
/// written as the one-symbol code for symbol 0.
pub(crate) fn store(w: &mut BitWriter<'_>, code: &Code, alphabet_size: usize) {
    if let Some(few) = &code.few {
        store_simple(w, code, few, alphabet_size);
    } else {
        store_complex(w, &code.lengths);
    }
}

fn store_simple(w: &mut BitWriter<'_>, code: &Code, few: &[u16], alphabet_size: usize) {
    let bits = alphabet_bits(alphabet_size);
    let symbols: &[u16] = if few.is_empty() { &[0] } else { few };
    w.put(2, 1);
    w.put(2, symbols.len() as u32 - 1);
    // Listed by code length, then symbol: the order whose lengths §3.4 fixes (1, 2, 2 for
    // three symbols; 1, 2, 3, 3 or 2, 2, 2, 2 for four).
    let mut listed: Vec<u16> = symbols.to_vec();
    listed.sort_by_key(|&s| (code.lengths[s as usize], s));
    for &s in &listed {
        w.put(bits, u32::from(s));
    }
    if listed.len() == 4 {
        w.put(1, u32::from(code.lengths[listed[0] as usize] == 1));
    }
}

/// One code-length symbol (0..=17) and its extra bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LengthSymbol {
    pub(crate) symbol: u8,
    pub(crate) extra: u8,
}

/// The repeat codes for a run of `n >= 3` lengths: code 16 (`base` 4) or 17 (`base` 8). The
/// decoder turns a chain of extras e1, e2, ... into the count (((e1 + 1) * base + e2 + 1) *
/// base + ...) + 2, so the chain is the digits 1..=base of `n - 2` in bijective base `base`,
/// most significant first, each less one.
fn repeat_chain(symbol: u8, base: usize, n: usize, out: &mut Vec<LengthSymbol>) {
    debug_assert!(n >= 3);
    let start = out.len();
    let mut d = n - 2;
    while d > 0 {
        let q = (d - 1) / base;
        let digit = d - q * base;
        out.push(LengthSymbol {
            symbol,
            extra: (digit - 1) as u8,
        });
        d = q;
    }
    out[start..].reverse();
}

/// The code-length symbols for `lengths`, up to the last non-zero one (§3.5): runs of zeros
/// as code 17, runs of a repeated length as code 16, where they pay, as the reference decides
/// (`c/enc/entropy_encode.c`, `BrotliWriteHuffmanTree`): not for alphabets of 50 symbols or
/// fewer, and for zeros and other lengths each only if the runs worth coding (3 zeros or more,
/// 4 of another length or more) average more than two symbols.
pub(crate) fn run_lengths(lengths: &[u8], out: &mut Vec<LengthSymbol>) {
    let end = lengths.iter().rposition(|&l| l != 0).map_or(0, |i| i + 1);
    let (zeros, others) = if lengths.len() > RLE_ALPHABET {
        runs_worth_coding(&lengths[..end])
    } else {
        (false, false)
    };
    write_runs(&lengths[..end], zeros, others, out);
}

/// Alphabets this size or smaller are written without run-length codes.
const RLE_ALPHABET: usize = 50;

/// Whether to run-length code the zeros, and the other lengths, of `lengths`.
fn runs_worth_coding(lengths: &[u8]) -> (bool, bool) {
    // (symbols in runs, runs + 1), for zeros and for other lengths.
    let (mut zeros, mut others) = ((0, 1), (0, 1));
    let mut i = 0;
    while i < lengths.len() {
        let v = lengths[i];
        let run = lengths[i..].iter().take_while(|&&l| l == v).count();
        if v == 0 && run >= 3 {
            zeros = (zeros.0 + run, zeros.1 + 1);
        }
        if v != 0 && run >= 4 {
            others = (others.0 + run, others.1 + 1);
        }
        i += run;
    }
    (zeros.0 > 2 * zeros.1, others.0 > 2 * others.1)
}

/// Writes `lengths` as code-length symbols, with code 17 for runs of zeros if `zeros` and code
/// 16 for runs of another length if `others`.
fn write_runs(lengths: &[u8], zeros: bool, others: bool, out: &mut Vec<LengthSymbol>) {
    out.clear();
    // Code 16 repeats the previous non-zero length written, 8 at the start.
    let mut previous = 8u8;
    let mut i = 0;
    while i < lengths.len() {
        let v = lengths[i];
        let run = if (v == 0 && zeros) || (v != 0 && others) {
            lengths[i..].iter().take_while(|&&l| l == v).count()
        } else {
            1
        };
        let literal = LengthSymbol {
            symbol: v,
            extra: 0,
        };
        let mut rest = run;
        if v == 0 {
            // 11 zeros as one zero and one code 17 (10), not two codes 17, as the reference.
            if rest == 11 {
                out.push(literal);
                rest -= 1;
            }
            if rest >= 3 {
                repeat_chain(17, 8, rest, out);
            } else {
                out.extend(std::iter::repeat_n(literal, rest));
            }
        } else {
            if v != previous {
                out.push(literal);
                previous = v;
                rest -= 1;
            }
            // 7 repeats as the length and one code 16 (6), not two codes 16.
            if rest == 7 {
                out.push(literal);
                rest -= 1;
            }
            if rest >= 3 {
                repeat_chain(16, 4, rest, out);
            } else {
                out.extend(std::iter::repeat_n(literal, rest));
            }
        }
        i += run;
    }
}

fn store_complex(w: &mut BitWriter<'_>, lengths: &[u8]) {
    let mut symbols = Vec::new();
    run_lengths(lengths, &mut symbols);
    let mut counts = [0u32; 18];
    for s in &symbols {
        counts[s.symbol as usize] += 1;
    }
    let used = counts.iter().filter(|&&c| c != 0).count();
    let mut cl_lengths = [0u8; 18];
    if used == 1 {
        // One code-length symbol: any non-zero length; it then takes no bits (§3.5).
        let only = counts.iter().position(|&c| c != 0).unwrap_or(0);
        cl_lengths[only] = 1;
    } else {
        limited_lengths(&counts, 5, &mut cl_lengths);
    }
    let mut cl_codes = Vec::new();
    canonical(&cl_lengths, &mut cl_codes);

    // HSKIP: leading zero lengths in storage order that needn't be written (0, 2 or 3).
    let hskip = if cl_lengths[1] != 0 || cl_lengths[2] != 0 {
        0
    } else if cl_lengths[3] != 0 {
        2
    } else {
        3
    };
    w.put(2, hskip);
    // Up to the last non-zero length in storage order, where the decoder finds the code
    // complete and stops; a one-symbol code is read to the end of the list.
    let last = if used == 1 {
        ORDER.len()
    } else {
        ORDER
            .iter()
            .rposition(|&s| cl_lengths[s] != 0)
            .map_or(0, |i| i + 1)
    };
    for &s in &ORDER[hskip as usize..last] {
        let (bits, n) = FIXED[cl_lengths[s] as usize];
        w.put(n, bits);
    }
    // A one-symbol code takes no bits per symbol, whatever length was written for it.
    let width = |i: usize| {
        if used == 1 {
            0
        } else {
            u32::from(cl_lengths[i])
        }
    };
    for s in &symbols {
        let i = s.symbol as usize;
        w.put(width(i), u32::from(cl_codes[i]) & ((1 << width(i)) - 1));
        match s.symbol {
            16 => w.put(2, u32::from(s.extra)),
            17 => w.put(3, u32::from(s.extra)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LengthSymbol, run_lengths, runs_worth_coding, store, write_runs};
    use crate::bits::BitReader;
    use crate::bitwriter::BitWriter;
    use crate::encode::huffman::Code;
    use crate::prefix;

    /// Writes `code`, then `message` with it; reads the code back with the decoder and
    /// decodes the message.
    fn round_trip(code: &Code, alphabet: usize, message: &[u16]) -> Vec<u32> {
        let mut buf = Vec::new();
        let mut w = BitWriter::new(&mut buf);
        store(&mut w, code, alphabet);
        for &s in message {
            w.put(
                u32::from(code.lengths[s as usize]),
                u32::from(code.codes[s as usize]),
            );
        }
        w.put(8, 0xa5);
        w.finish();
        let mut r = BitReader::new(&buf);
        let (mut table, mut scratch) = (Vec::new(), Vec::new());
        prefix::read(&mut r, alphabet as u32, &mut table, &mut scratch).unwrap();
        let out = message
            .iter()
            .map(|_| prefix::decode(&table, 0, &mut r).unwrap())
            .collect();
        assert_eq!(
            r.bits(8).unwrap(),
            0xa5,
            "the reader is where the writer stopped"
        );
        out
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

    #[test]
    fn codes_of_every_size_round_trip() {
        let mut rng = Rng(0x2545_f491_4f6c_dd1d);
        for round in 0..2000 {
            let alphabet = [2, 3, 4, 5, 18, 26, 64, 256, 258, 704][round % 10];
            // From one used symbol up to all of them, with skewed and flat counts.
            let used = 1 + (rng.next() as usize % alphabet);
            let mut counts = vec![0u32; alphabet];
            for _ in 0..used {
                let s = rng.next() as usize % alphabet;
                counts[s] = 1 + (rng.next() % [3, 1000, 1 << 20][round % 3]) as u32;
            }
            let code = Code::from_counts(&counts, 15);
            let message: Vec<u16> = (0..alphabet as u16)
                .filter(|&s| counts[s as usize] != 0)
                .cycle()
                .take(300)
                .collect();
            let got = round_trip(&code, alphabet, &message);
            let want: Vec<u32> = message.iter().map(|&s| u32::from(s)).collect();
            assert_eq!(
                got, want,
                "round {round}: alphabet {alphabet}, counts {counts:?}"
            );
        }
    }

    #[test]
    fn an_unused_code_is_one_symbol() {
        let code = Code::from_counts(&[0; 64], 15);
        assert_eq!(round_trip(&code, 64, &[]), Vec::<u32>::new());
    }

    #[test]
    fn equal_lengths_use_one_repeat_symbol() {
        // 256 lengths of 8: all code 16, starting from the implicit previous length 8.
        let mut out = Vec::new();
        run_lengths(&[8; 256], &mut out);
        assert!(out.iter().all(|s| s.symbol == 16), "{out:?}");
        let code = Code::from_counts(&[1; 256], 15);
        let message: Vec<u16> = (0..256).collect();
        let want: Vec<u32> = (0..256).collect();
        assert_eq!(round_trip(&code, 256, &message), want);
    }

    /// The lengths a decoder rebuilds from code-length symbols (§3.5).
    fn expand(symbols: &[LengthSymbol]) -> Vec<u8> {
        let mut out = Vec::new();
        let (mut previous, mut repeat, mut repeat_length) = (8u8, 0usize, 0u8);
        for s in symbols {
            if s.symbol < 16 {
                out.push(s.symbol);
                repeat = 0;
                if s.symbol != 0 {
                    previous = s.symbol;
                }
                continue;
            }
            let (bits, length) = if s.symbol == 16 {
                (2, previous)
            } else {
                (3, 0)
            };
            if repeat_length != length {
                repeat = 0;
                repeat_length = length;
            }
            let old = repeat;
            if repeat > 0 {
                repeat = (repeat - 2) << bits;
            }
            repeat += usize::from(s.extra) + 3;
            out.extend(std::iter::repeat_n(length, repeat - old));
        }
        out
    }

    #[test]
    fn runs_of_every_length_expand_back() {
        for run in 0..300 {
            for (before, value) in [
                (None, 0u8),
                (None, 8),
                (Some(5u8), 5),
                (Some(3), 7),
                (Some(2), 0),
            ] {
                let mut lengths: Vec<u8> = before.into_iter().collect();
                lengths.extend(std::iter::repeat_n(value, run));
                lengths.push(1); // a non-zero last length, so nothing is trimmed
                let mut symbols = Vec::new();
                run_lengths(&lengths, &mut symbols);
                assert_eq!(
                    expand(&symbols),
                    lengths,
                    "run of {run} x {value} after {before:?}"
                );
                // With run-length codes on, whatever the alphabet's size.
                for (zeros, others) in [(true, true), (true, false), (false, true)] {
                    write_runs(&lengths, zeros, others, &mut symbols);
                    assert_eq!(
                        expand(&symbols),
                        lengths,
                        "run of {run} x {value} after {before:?}, {zeros} {others}"
                    );
                }
            }
        }
    }

    #[test]
    fn run_length_codes_are_used_where_they_pay() {
        let mut out = Vec::new();
        // A small alphabet: none, however long the run.
        let small: Vec<u8> = [vec![0; 40], vec![3, 3]].concat();
        run_lengths(&small, &mut out);
        assert!(out.iter().all(|s| s.symbol < 16), "{out:?}");
        // A large one with long runs: both.
        let large: Vec<u8> = [vec![0; 40], vec![5; 20], vec![2]].concat();
        run_lengths(&large, &mut out);
        assert!(out.iter().any(|s| s.symbol == 17) && out.iter().any(|s| s.symbol == 16));
        assert_eq!(expand(&out), large);
        // Runs that barely qualify, averaging two symbols or less: not worth it.
        assert_eq!(runs_worth_coding(&[0, 0, 0, 1, 2, 1, 2]), (false, false));
        assert_eq!(
            runs_worth_coding(&[0, 0, 0, 0, 0, 1, 1, 1, 1, 1]),
            (true, true)
        );
        // 7 repeats and 11 zeros start with a plain symbol.
        write_runs(&[4; 8], true, true, &mut out);
        assert_eq!(out.iter().map(|s| s.symbol).collect::<Vec<_>>(), [4, 4, 16]);
        write_runs(&[[0; 11].as_slice(), &[1]].concat(), true, true, &mut out);
        assert_eq!(out.iter().map(|s| s.symbol).collect::<Vec<_>>(), [0, 17, 1]);
    }

    #[test]
    fn trailing_zeros_are_not_written() {
        let mut out = Vec::new();
        run_lengths(&[3, 0, 3, 0, 0, 0, 0, 0], &mut out);
        assert_eq!(expand(&out), [3, 0, 3]);
    }
}
