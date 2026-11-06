//! Prefix codes (RFC 7932 §3): reading their compact form and decoding symbols with them.
//!
//! A code becomes a two-level lookup table: the first [`ROOT_BITS`] bits of the input index a
//! root table; codes longer than that continue in a second-level table the root entry points
//! to. Tables are appended to a shared `Vec<u32>` so a meta-block's many codes live in one
//! allocation that is reused between meta-blocks.
//!
//! An entry packs a symbol (or a second-level offset) in its low 16 bits and a length in the
//! next 8. In the root table a length above [`ROOT_BITS`] marks a link: the second-level table
//! starts at the offset and is indexed by the next `length - ROOT_BITS` bits.

use crate::Error;
use crate::bits::BitReader;

/// Bits looked up in the root table.
pub(crate) const ROOT_BITS: u32 = 8;

/// The longest code for any alphabet (§3.5: code lengths are 0..=15).
pub(crate) const MAX_LENGTH: u32 = 15;

/// The order code-length code lengths are stored in (§3.5).
const CODE_LENGTH_ORDER: [usize; 18] =
    [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// The fixed code for code-length code lengths (§3.5), by the next 4 bits of input:
/// `(length in bits, value)`.
const CODE_LENGTH_CODE: [(u8, u8); 16] = [
    (2, 0),
    (2, 4),
    (2, 3),
    (3, 2),
    (2, 0),
    (2, 4),
    (2, 3),
    (4, 1),
    (2, 0),
    (2, 4),
    (2, 3),
    (3, 2),
    (2, 0),
    (2, 4),
    (2, 3),
    (4, 5),
];

#[inline(always)]
fn entry(length: u32, value: u32) -> u32 {
    (length << 16) | value
}

/// Decodes one symbol with the table starting at `table[start]`.
///
/// # Errors
///
/// [`Error::Truncated`] if the input ends inside the code.
#[inline(always)]
pub(crate) fn decode(table: &[u32], start: usize, r: &mut BitReader<'_>) -> Result<u32, Error> {
    let bits = r.peek(MAX_LENGTH);
    let e = table[start + (bits & ((1 << ROOT_BITS) - 1)) as usize];
    let length = e >> 16;
    if length <= ROOT_BITS {
        r.consume(length)?;
        return Ok(e & 0xffff);
    }
    let sub = length - ROOT_BITS;
    let index = (bits >> ROOT_BITS) & ((1 << sub) - 1);
    let e = table[start + (e & 0xffff) as usize + index as usize];
    r.consume(e >> 16)?;
    Ok(e & 0xffff)
}

/// The number of bits that can hold every symbol of an alphabet of `size` (§3.4).
fn alphabet_bits(size: u32) -> u32 {
    32 - (size - 1).leading_zeros()
}

/// Reads one prefix code over `alphabet_size` symbols (§3.4, §3.5) and appends its table to
/// `table`. `lengths` is scratch space, reused between calls.
///
/// # Errors
///
/// [`Error::Corrupt`] for an invalid code, [`Error::Truncated`] if the input ends.
pub(crate) fn read(
    r: &mut BitReader<'_>,
    alphabet_size: u32,
    table: &mut Vec<u32>,
    lengths: &mut Vec<u8>,
) -> Result<(), Error> {
    debug_assert!((2..=1 << 16).contains(&alphabet_size));
    let hskip = r.bits(2)?;
    if hskip == 1 {
        return read_simple(r, alphabet_size, table);
    }
    read_complex(r, hskip, alphabet_size, lengths)?;
    build(lengths, table)
}

/// A simple prefix code (§3.4): one to four symbols listed explicitly.
fn read_simple(
    r: &mut BitReader<'_>,
    alphabet_size: u32,
    table: &mut Vec<u32>,
) -> Result<(), Error> {
    let nsym = r.bits(2)? as usize + 1;
    let width = alphabet_bits(alphabet_size);
    let mut symbols = [0u32; 4];
    for i in 0..nsym {
        let s = r.bits(width)?;
        if s >= alphabet_size {
            return Err(Error::Corrupt(
                "simple prefix code symbol is outside the alphabet",
            ));
        }
        if symbols[..i].contains(&s) {
            return Err(Error::Corrupt("simple prefix code repeats a symbol"));
        }
        symbols[i] = s;
    }
    // Lengths in the order the symbols were listed.
    let lengths: &[u8] = match nsym {
        1 => &[0],
        2 => &[1, 1],
        3 => &[1, 2, 2],
        _ if r.bit()? => &[1, 2, 3, 3],
        _ => &[2, 2, 2, 2],
    };
    if nsym == 1 {
        let start = table.len();
        table.resize(start + (1 << ROOT_BITS), entry(0, symbols[0]));
        return Ok(());
    }
    // Canonical codes go to symbols in increasing order within each length.
    let mut pairs: Vec<(u8, u32)> = lengths.iter().copied().zip(symbols).collect();
    pairs.sort_unstable();
    build_sorted(&pairs, table);
    Ok(())
}

/// A complex prefix code (§3.5): code lengths for every symbol, themselves coded with a
/// prefix code. Leaves the lengths in `lengths`, one per symbol of the alphabet.
fn read_complex(
    r: &mut BitReader<'_>,
    hskip: u32,
    alphabet_size: u32,
    lengths: &mut Vec<u8>,
) -> Result<(), Error> {
    // The code-length code: lengths 0..=5 for the 18 code-length symbols, stopping once the
    // code is complete. Kraft sums are counted in units of 2^-5.
    let mut cl_lengths = [0u8; 18];
    let mut space: i32 = 32;
    let mut used = 0;
    for &symbol in &CODE_LENGTH_ORDER[hskip as usize..] {
        let (bits, value) = CODE_LENGTH_CODE[r.peek(4) as usize];
        r.consume(u32::from(bits))?;
        cl_lengths[symbol] = value;
        if value != 0 {
            space -= 32 >> value;
            used += 1;
            if space <= 0 {
                break;
            }
        }
    }
    if !(used == 1 || space == 0) {
        return Err(Error::Corrupt(
            "code length code is incomplete or oversubscribed",
        ));
    }
    let mut cl_table = Vec::with_capacity(1 << ROOT_BITS);
    if used == 1 {
        let only = cl_lengths.iter().position(|&l| l != 0).unwrap_or(0) as u32;
        cl_table.resize(1 << ROOT_BITS, entry(0, only));
    } else {
        build(&cl_lengths, &mut cl_table)?;
    }

    // The symbol code lengths, until the code is complete (Kraft sum in units of 2^-15).
    lengths.clear();
    lengths.resize(alphabet_size as usize, 0);
    let mut symbol = 0usize;
    let mut space: i32 = 1 << MAX_LENGTH;
    let mut prev_length: u8 = 8;
    // The current run of repeat codes: what it repeats, and how many so far.
    let mut repeat_length: u8 = 0;
    let mut repeat: usize = 0;
    while symbol < lengths.len() && space > 0 {
        let code = decode(&cl_table, 0, r)? as u8;
        if code < 16 {
            repeat = 0;
            lengths[symbol] = code;
            symbol += 1;
            if code != 0 {
                prev_length = code;
                space -= (1 << MAX_LENGTH) >> code;
            }
            continue;
        }
        let (extra, length) = if code == 16 { (2, prev_length) } else { (3, 0) };
        if repeat_length != length {
            repeat = 0;
            repeat_length = length;
        }
        let old = repeat;
        if repeat > 0 {
            repeat = (repeat - 2) << extra;
        }
        repeat += r.bits(extra)? as usize + 3;
        let delta = repeat - old;
        if symbol + delta > lengths.len() {
            return Err(Error::Corrupt(
                "code lengths repeat past the end of the alphabet",
            ));
        }
        lengths[symbol..symbol + delta].fill(length);
        symbol += delta;
        if length != 0 {
            space -= (delta as i32) << (MAX_LENGTH - u32::from(length));
        }
    }
    if space != 0 {
        return Err(Error::Corrupt(
            "prefix code is incomplete or oversubscribed",
        ));
    }
    Ok(())
}

/// Builds the table for canonical code `lengths` (one per symbol, 0 for unused) and appends
/// it to `table`. The lengths must form a complete code.
fn build(lengths: &[u8], table: &mut Vec<u32>) -> Result<(), Error> {
    let mut pairs: Vec<(u8, u32)> = lengths
        .iter()
        .enumerate()
        .filter(|&(_, &l)| l != 0)
        .map(|(s, &l)| (l, s as u32))
        .collect();
    if pairs.len() < 2 {
        return Err(Error::Corrupt("prefix code has fewer than two symbols"));
    }
    pairs.sort_unstable();
    build_sorted(&pairs, table);
    Ok(())
}

/// Reverses the low `n` bits of `v`.
fn reverse(v: u32, n: u32) -> u32 {
    v.reverse_bits() >> (32 - n)
}

/// Builds the table for a complete canonical code given as `(length, symbol)` pairs sorted by
/// length, then symbol (§3.2), and appends it to `table`.
fn build_sorted(pairs: &[(u8, u32)], table: &mut Vec<u32>) {
    let start = table.len();
    let root_size = 1usize << ROOT_BITS;
    table.resize(start + root_size, 0);
    let mut code: u32 = 0;
    let mut prev_length = pairs[0].0;
    let mut i = 0;
    // Codes that fit in the root table.
    while i < pairs.len() && u32::from(pairs[i].0) <= ROOT_BITS {
        let (length, symbol) = pairs[i];
        code <<= length - prev_length;
        prev_length = length;
        let length = u32::from(length);
        let rev = reverse(code, length) as usize;
        let e = entry(length, symbol);
        for k in (rev..root_size).step_by(1 << length) {
            table[start + k] = e;
        }
        code += 1;
        i += 1;
    }
    // Longer codes, grouped by their first ROOT_BITS bits. Codes are canonical, so a group's
    // codes are consecutive and its longest code is its last.
    while i < pairs.len() {
        let length = u32::from(pairs[i].0);
        code <<= length - u32::from(prev_length);
        let prefix = code >> (length - ROOT_BITS);
        // The longest code sharing this prefix sets the second-level size.
        let mut j = i;
        let mut c = code;
        let mut l = length;
        let mut max_length = length;
        while j < pairs.len() {
            let lj = u32::from(pairs[j].0);
            c <<= lj - l;
            l = lj;
            if c >> (lj - ROOT_BITS) != prefix {
                break;
            }
            max_length = lj;
            c += 1;
            j += 1;
        }
        let sub_bits = max_length - ROOT_BITS;
        let sub_start = table.len() - start;
        table.resize(table.len() + (1 << sub_bits), 0);
        table[start + reverse(prefix, ROOT_BITS) as usize] =
            entry(ROOT_BITS + sub_bits, sub_start as u32);
        // Fill the group.
        let mut l = length;
        while i < j {
            let (length, symbol) = pairs[i];
            let length = u32::from(length);
            code <<= length - l;
            l = length;
            let rest = length - ROOT_BITS;
            let low = reverse(code & ((1 << rest) - 1), rest) as usize;
            let e = entry(length, symbol);
            for k in (low..1 << sub_bits).step_by(1 << rest) {
                table[start + sub_start + k] = e;
            }
            code += 1;
            i += 1;
        }
        prev_length = l as u8;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{MAX_LENGTH, decode, read};
    use crate::Error;
    use crate::bits::{BitReader, TestBits};

    /// The storage order of code-length code lengths (§3.5).
    const ORDER: [usize; 18] = [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];

    /// The fixed code for code-length code lengths 0..=5 (§3.5), as `(bits, count)` written
    /// least significant bit first: the RFC lists them "parsed from right to left".
    const FIXED: [(u32, u32); 6] = [
        (0b00, 2),
        (0b0111, 4),
        (0b011, 3),
        (0b10, 2),
        (0b01, 2),
        (0b1111, 4),
    ];

    /// Canonical codes (most significant bit first) for `lengths` (§3.2).
    pub(crate) fn canonical(lengths: &[u8]) -> Vec<u32> {
        let mut count = [0u32; 16];
        for &l in lengths {
            count[l as usize] += 1;
        }
        count[0] = 0;
        let mut next = [0u32; 16];
        let mut code = 0;
        for bits in 1..16 {
            code = (code + count[bits - 1]) << 1;
            next[bits] = code;
        }
        lengths
            .iter()
            .map(|&l| {
                let c = next[l as usize];
                if l != 0 {
                    next[l as usize] += 1;
                }
                c
            })
            .collect()
    }

    /// Writes HSKIP 0 and the code-length code lengths `cl` (indexed by code-length symbol),
    /// stopping where the decoder stops: once the code is complete.
    pub(crate) fn write_cl_code(w: &mut TestBits, cl: &[u8; 18]) {
        w.put(2, 0);
        let mut space = 32;
        for &s in &ORDER {
            let (bits, n) = FIXED[cl[s] as usize];
            w.put(n, bits);
            if cl[s] != 0 {
                space -= 32 >> cl[s];
                if space <= 0 {
                    break;
                }
            }
        }
    }

    /// Writes a complex prefix code for `lengths` with no repeat codes: the code-length code
    /// gives each of the symbols 0..=15 a 4-bit code (so symbol `s` is written as `s`), and
    /// the lengths follow until the code is complete.
    pub(crate) fn write_complex(w: &mut TestBits, lengths: &[u8]) {
        let mut cl = [4u8; 18];
        cl[16] = 0;
        cl[17] = 0;
        write_cl_code(w, &cl);
        let mut space: i32 = 1 << MAX_LENGTH;
        for &l in lengths {
            if space <= 0 {
                break;
            }
            w.code(u32::from(l), 4);
            if l != 0 {
                space -= (1 << MAX_LENGTH) >> l;
            }
        }
    }

    /// Writes `symbols` with the canonical code for `lengths`.
    pub(crate) fn write_symbols(w: &mut TestBits, lengths: &[u8], symbols: &[u32]) {
        let codes = canonical(lengths);
        for &s in symbols {
            w.code(codes[s as usize], u32::from(lengths[s as usize]));
        }
    }

    /// Reads a code over `alphabet` symbols from `bytes`, then decodes `n` symbols.
    fn read_and_decode(bytes: &[u8], alphabet: u32, n: usize) -> Result<Vec<u32>, Error> {
        let mut r = BitReader::new(bytes);
        let (mut table, mut scratch) = (Vec::new(), Vec::new());
        read(&mut r, alphabet, &mut table, &mut scratch)?;
        (0..n).map(|_| decode(&table, 0, &mut r)).collect()
    }

    #[test]
    fn canonical_codes_match_the_rfc_example() {
        // §3.2: lengths (3, 3, 3, 3, 3, 2, 4, 4) give 010 011 100 101 110 00 1110 1111.
        assert_eq!(
            canonical(&[3, 3, 3, 3, 3, 2, 4, 4]),
            [0b010, 0b011, 0b100, 0b101, 0b110, 0b00, 0b1110, 0b1111]
        );
    }

    #[test]
    fn simple_codes_of_one_to_four_symbols() {
        /// NSYM - 1, the symbols as listed, the tree-select bit, lengths in listed order.
        type Case = (u32, &'static [u32], Option<u32>, &'static [u8]);
        let cases: &[Case] = &[
            (1, &[9, 3], None, &[1, 1]),
            (2, &[7, 250, 1], None, &[1, 2, 2]),
            (3, &[4, 3, 2, 1], Some(0), &[2, 2, 2, 2]),
            (3, &[40, 30, 20, 10], Some(1), &[1, 2, 3, 3]),
        ];
        for &(nsym1, symbols, tree, listed) in cases {
            let mut w = TestBits::default();
            w.put(2, 1).put(2, nsym1);
            for &s in symbols {
                w.put(8, s);
            }
            if let Some(t) = tree {
                w.put(1, t);
            }
            let mut lengths = vec![0u8; 256];
            for (&s, &l) in symbols.iter().zip(listed) {
                lengths[s as usize] = l;
            }
            let message: Vec<u32> = symbols.iter().rev().chain(symbols).copied().collect();
            write_symbols(&mut w, &lengths, &message);
            assert_eq!(
                read_and_decode(&w.finish(), 256, message.len()).unwrap(),
                message,
                "symbols {symbols:?}"
            );
        }
    }

    #[test]
    fn a_one_symbol_code_reads_no_bits() {
        let mut w = TestBits::default();
        w.put(2, 1).put(2, 0).put(8, 77).put(8, 0xa5);
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes);
        let (mut table, mut scratch) = (Vec::new(), Vec::new());
        read(&mut r, 256, &mut table, &mut scratch).unwrap();
        for _ in 0..5 {
            assert_eq!(decode(&table, 0, &mut r).unwrap(), 77);
        }
        assert_eq!(
            r.bits(8).unwrap(),
            0xa5,
            "the next field is where it should be"
        );
    }

    #[test]
    fn simple_code_symbols_are_checked() {
        let mut w = TestBits::default();
        w.put(2, 1).put(2, 2).put(8, 5).put(8, 6).put(8, 5);
        assert_eq!(
            read_and_decode(&w.finish(), 256, 0),
            Err(Error::Corrupt("simple prefix code repeats a symbol"))
        );
        // An alphabet of 704 has 10-bit symbols, and 704 itself is outside it.
        let mut w = TestBits::default();
        w.put(2, 1).put(2, 1).put(10, 703).put(10, 704);
        assert_eq!(
            read_and_decode(&w.finish(), 704, 0),
            Err(Error::Corrupt(
                "simple prefix code symbol is outside the alphabet"
            ))
        );
        let mut w = TestBits::default();
        w.put(2, 1).put(2, 1).put(10, 703).put(10, 0);
        let mut lengths = vec![0u8; 704];
        lengths[0] = 1;
        lengths[703] = 1;
        write_symbols(&mut w, &lengths, &[703, 0, 0]);
        assert_eq!(read_and_decode(&w.finish(), 704, 3).unwrap(), [703, 0, 0]);
        // An alphabet of 26 (block counts) has 5-bit symbols.
        let mut w = TestBits::default();
        w.put(2, 1).put(2, 0).put(5, 25);
        assert_eq!(read_and_decode(&w.finish(), 26, 1).unwrap(), [25]);
    }

    #[test]
    fn a_complex_code_of_256_equal_lengths() {
        let lengths = vec![8u8; 256];
        let mut w = TestBits::default();
        write_complex(&mut w, &lengths);
        let message: Vec<u32> = (0..256).rev().chain(0..256).collect();
        write_symbols(&mut w, &lengths, &message);
        assert_eq!(
            read_and_decode(&w.finish(), 256, message.len()).unwrap(),
            message
        );
    }

    #[test]
    fn codes_up_to_fifteen_bits_use_the_second_level() {
        // Lengths 1, 2, ..., 14, 15, 15: complete, and seven of them longer than the root.
        let mut lengths: Vec<u8> = (1..=15).collect();
        lengths.push(15);
        let mut w = TestBits::default();
        write_complex(&mut w, &lengths);
        let message: Vec<u32> = (0..16)
            .rev()
            .chain(0..16)
            .chain([15, 15, 14, 9, 8, 0])
            .collect();
        write_symbols(&mut w, &lengths, &message);
        assert_eq!(
            read_and_decode(&w.finish(), 16, message.len()).unwrap(),
            message
        );
    }

    #[test]
    fn a_lone_sixteen_repeats_the_initial_length_eight() {
        // §3.5: when the code-length code has a single symbol (16), it takes no bits, and the
        // previous length starts at 8. 256 lengths of 8 come from four chained repeats:
        // 3+2 = 5; 4*(5-2)+3+2 = 17; 4*(17-2)+3+2 = 65; 4*(65-2)+3+1 = 256.
        let mut cl = [0u8; 18];
        cl[16] = 4; // any non-zero length makes it the single symbol
        let mut w = TestBits::default();
        w.put(2, 0);
        for &s in &ORDER {
            let (bits, n) = FIXED[cl[s] as usize];
            w.put(n, bits);
        }
        for extra in [2, 2, 2, 1] {
            w.put(2, extra);
        }
        let lengths = vec![8u8; 256];
        let message = [0, 255, 128, 7];
        write_symbols(&mut w, &lengths, &message);
        assert_eq!(
            read_and_decode(&w.finish(), 256, message.len()).unwrap(),
            message
        );
    }

    #[test]
    fn chained_sixteens_multiply_the_count() {
        // 7, then 16(+0) = 3, 16(+2) = 4*1+5 = 9, 16(+2) = 4*7+5 = 33, 16(+0) = 4*31+3 = 127:
        // 128 lengths of 7, a complete code over 128 symbols.
        let mut cl = [0u8; 18];
        cl[7] = 1;
        cl[16] = 1;
        let codes = canonical(&cl);
        let mut w = TestBits::default();
        write_cl_code(&mut w, &cl);
        w.code(codes[7], 1);
        for extra in [0, 2, 2, 0] {
            w.code(codes[16], 1).put(2, extra);
        }
        let lengths = vec![7u8; 128];
        let message = [0, 127, 64, 1];
        write_symbols(&mut w, &lengths, &message);
        assert_eq!(
            read_and_decode(&w.finish(), 128, message.len()).unwrap(),
            message
        );
    }

    #[test]
    fn chained_seventeens_multiply_the_count() {
        // 1, 17(+7) = 10 zeros, 17(+0) = 8*(10-2)+3 = 67 zeros, 1: symbols 0 and 68.
        let mut cl = [0u8; 18];
        cl[1] = 1;
        cl[17] = 1;
        let codes = canonical(&cl);
        let mut w = TestBits::default();
        write_cl_code(&mut w, &cl);
        w.code(codes[1], 1);
        w.code(codes[17], 1).put(3, 7);
        w.code(codes[17], 1).put(3, 0);
        w.code(codes[1], 1);
        let mut lengths = vec![0u8; 69];
        lengths[0] = 1;
        lengths[68] = 1;
        let message = [68, 0, 68];
        write_symbols(&mut w, &lengths, &message);
        assert_eq!(
            read_and_decode(&w.finish(), 69, message.len()).unwrap(),
            message
        );
    }

    #[test]
    fn a_sixteen_after_a_seventeen_starts_a_new_count() {
        // 3, 17(+0) = 3 zeros, 16(+0) = 3 more 3s (not chained to the 17), 2, 2:
        // lengths [3, 0, 0, 0, 3, 3, 3, 2, 2], complete (4/8 + 2/4).
        let mut cl = [0u8; 18];
        for s in [2, 3, 16, 17] {
            cl[s] = 2;
        }
        let codes = canonical(&cl);
        let mut w = TestBits::default();
        write_cl_code(&mut w, &cl);
        w.code(codes[3], 2);
        w.code(codes[17], 2).put(3, 0);
        w.code(codes[16], 2).put(2, 0);
        w.code(codes[2], 2);
        w.code(codes[2], 2);
        let lengths = [3, 0, 0, 0, 3, 3, 3, 2, 2];
        let message = [8, 0, 4, 5, 6, 7];
        write_symbols(&mut w, &lengths, &message);
        assert_eq!(
            read_and_decode(&w.finish(), 9, message.len()).unwrap(),
            message
        );
    }

    #[test]
    fn incomplete_and_oversubscribed_codes_are_rejected() {
        for lengths in [
            // 3/4, then a 1 overshoots. (Reading stops once a code is complete, so only a
            // last step can oversubscribe it: 1, 2, 2, 2 would be the complete 1, 2, 2.)
            &[2, 2, 2, 1, 0, 0, 0, 0][..],
            &[1, 2, 0, 0, 0, 0, 0, 0], // 3/4: incomplete
            &[0, 0, 1, 0, 0, 0, 0, 0], // one symbol: a complex code needs two
        ] {
            let mut w = TestBits::default();
            write_complex(&mut w, lengths);
            assert_eq!(
                read_and_decode(&w.finish(), 8, 0),
                Err(Error::Corrupt(
                    "prefix code is incomplete or oversubscribed"
                )),
                "{lengths:?}"
            );
        }
    }

    #[test]
    fn an_incomplete_code_length_code_is_rejected() {
        // Two code-length symbols of length 2 and nothing else: half the space.
        let mut cl = [0u8; 18];
        cl[1] = 2;
        cl[2] = 2;
        let mut w = TestBits::default();
        write_cl_code(&mut w, &cl);
        assert_eq!(
            read_and_decode(&w.finish(), 256, 0),
            Err(Error::Corrupt(
                "code length code is incomplete or oversubscribed"
            ))
        );
        // Lengths 2, 2, 2 (3/4), then a 1 overshoots.
        let mut w = TestBits::default();
        w.put(2, 0);
        for length in [2, 2, 2, 1] {
            let (bits, n) = FIXED[length];
            w.put(n, bits);
        }
        assert_eq!(
            read_and_decode(&w.finish(), 256, 0),
            Err(Error::Corrupt(
                "code length code is incomplete or oversubscribed"
            ))
        );
    }

    #[test]
    fn repeats_past_the_alphabet_are_rejected() {
        // 17(+7) is 10 zeros, into a 5-symbol alphabet.
        let mut cl = [0u8; 18];
        cl[17] = 1;
        cl[1] = 1;
        let codes = canonical(&cl);
        let mut w = TestBits::default();
        write_cl_code(&mut w, &cl);
        w.code(codes[17], 1).put(3, 7);
        assert_eq!(
            read_and_decode(&w.finish(), 5, 0),
            Err(Error::Corrupt(
                "code lengths repeat past the end of the alphabet"
            ))
        );
    }

    #[test]
    fn a_cut_off_code_is_truncation() {
        let lengths = vec![8u8; 256];
        let mut w = TestBits::default();
        write_complex(&mut w, &lengths);
        let bytes = w.finish();
        for len in 0..bytes.len() - 1 {
            assert_eq!(
                read_and_decode(&bytes[..len], 256, 0),
                Err(Error::Truncated),
                "{len} of {} bytes",
                bytes.len()
            );
        }
    }

    /// Random complete code lengths over `alphabet` symbols, at most 15 bits: split random
    /// leaves of a binary tree, then give the leaf depths to random symbols.
    fn random_lengths(rng: &mut u64, alphabet: usize) -> Vec<u8> {
        let mut next = || {
            *rng ^= *rng << 13;
            *rng ^= *rng >> 7;
            *rng ^= *rng << 17;
            *rng
        };
        let leaves = 2 + (next() as usize % (alphabet - 1));
        let mut depths = vec![1u8, 1];
        while depths.len() < leaves {
            let i = next() as usize % depths.len();
            if depths[i] < 15 {
                depths[i] += 1;
                depths.push(depths[i]);
            } else if depths.iter().all(|&d| d == 15) {
                break;
            }
        }
        let mut lengths = vec![0u8; alphabet];
        let mut free: Vec<usize> = (0..alphabet).collect();
        for d in depths {
            let k = next() as usize % free.len();
            lengths[free.swap_remove(k)] = d;
        }
        lengths
    }

    #[test]
    fn random_complete_codes_round_trip() {
        let mut rng = 0x2545_f491_4f6c_dd1d_u64;
        for round in 0..1000 {
            let alphabet = [2, 3, 18, 26, 64, 256, 704][round % 7];
            let lengths = random_lengths(&mut rng, alphabet);
            let used: Vec<u32> = (0..alphabet as u32)
                .filter(|&s| lengths[s as usize] != 0)
                .collect();
            let message: Vec<u32> = (0..200)
                .map(|i| used[(i * 7919 + round) % used.len()])
                .collect();
            let mut w = TestBits::default();
            write_complex(&mut w, &lengths);
            write_symbols(&mut w, &lengths, &message);
            assert_eq!(
                read_and_decode(&w.finish(), alphabet as u32, message.len()).unwrap(),
                message,
                "round {round}: lengths {lengths:?}"
            );
        }
    }
}
