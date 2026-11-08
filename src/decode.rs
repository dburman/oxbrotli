//! The decoder: the stream header, meta-blocks and commands (RFC 7932 §9, §10).

use crate::Error;
use crate::bits::BitReader;
use crate::context;
use crate::dictionary;
use crate::prefix;
use crate::tables::{
    BLOCK_COUNT_BASE, BLOCK_COUNT_EXTRA, COPY_BASE, COPY_EXTRA, INSERT_BASE, INSERT_EXTRA,
    command_codes,
};

/// The block count of a category with one block type: more than any meta-block holds.
const ONE_TYPE_COUNT: u32 = 1 << 24;

/// The distance ring at the start of a stream (§4): last, second-to-last, and so on.
const INITIAL_DISTANCES: [u32; 4] = [4, 11, 15, 16];

/// One block category (literals, insert-and-copy lengths or distances) of a meta-block (§6).
struct Category {
    types: u32,
    /// Offsets in the code table of the block type and block count codes.
    type_code: usize,
    count_code: usize,
    /// Symbols left in the current block.
    count: u32,
    current: u32,
    previous: u32,
}

impl Category {
    /// Reads the category's part of the meta-block header: NBLTYPES, and with two types or
    /// more, the block type and count codes and the first block count.
    fn read(
        r: &mut BitReader<'_>,
        table: &mut Vec<u32>,
        lengths: &mut Vec<u8>,
    ) -> Result<Self, Error> {
        let types = read_count_256(r)?;
        let mut c = Self {
            types,
            type_code: 0,
            count_code: 0,
            count: ONE_TYPE_COUNT,
            current: 0,
            previous: 1,
        };
        if types >= 2 {
            c.type_code = table.len();
            prefix::read(r, types + 2, table, lengths)?;
            c.count_code = table.len();
            prefix::read(r, 26, table, lengths)?;
            c.count = read_block_count(table, c.count_code, r)?;
        }
        Ok(c)
    }

    /// Counts one symbol of the category, switching blocks first if the current one is used
    /// up (§6).
    #[inline(always)]
    fn next(&mut self, table: &[u32], r: &mut BitReader<'_>) -> Result<(), Error> {
        if self.count == 0 {
            self.switch(table, r)?;
        }
        self.count -= 1;
        Ok(())
    }

    #[inline(never)]
    fn switch(&mut self, table: &[u32], r: &mut BitReader<'_>) -> Result<(), Error> {
        if self.types < 2 {
            self.count = ONE_TYPE_COUNT;
            return Ok(());
        }
        let new = match prefix::decode(table, self.type_code, r)? {
            0 => self.previous,
            1 => (self.current + 1) % self.types,
            symbol => symbol - 2,
        };
        self.previous = self.current;
        self.current = new;
        self.count = read_block_count(table, self.count_code, r)?;
        Ok(())
    }
}

/// A block count: its code, then extra bits (§6).
fn read_block_count(table: &[u32], code: usize, r: &mut BitReader<'_>) -> Result<u32, Error> {
    let c = prefix::decode(table, code, r)? as usize;
    Ok(BLOCK_COUNT_BASE[c] + r.bits(u32::from(BLOCK_COUNT_EXTRA[c]))?)
}

/// NBLTYPES, NTREES: a value in 1..=256 in the variable-length code of §9.2.
fn read_count_256(r: &mut BitReader<'_>) -> Result<u32, Error> {
    if !r.bit()? {
        return Ok(1);
    }
    let n = r.bits(3)?;
    if n == 0 {
        return Ok(2);
    }
    Ok((1 << n) + r.bits(n)? + 1)
}

/// WBITS (§9.1).
fn read_window_bits(r: &mut BitReader<'_>) -> Result<u32, Error> {
    if !r.bit()? {
        return Ok(16);
    }
    let n = r.bits(3)?;
    if n != 0 {
        return Ok(17 + n);
    }
    match r.bits(3)? {
        0 => Ok(17),
        1 => Err(Error::Corrupt("large-window Brotli is not supported")),
        m => Ok(8 + m),
    }
}

/// Reusable decoding state: the tables of the current meta-block.
#[derive(Default)]
pub(crate) struct Decoder {
    /// Every prefix code of the meta-block, one after another.
    table: Vec<u32>,
    /// Where each literal, insert-and-copy and distance code starts in `table`.
    literal_codes: Vec<u32>,
    command_codes: Vec<u32>,
    distance_codes: Vec<u32>,
    /// Context maps and literal context modes.
    literal_map: Vec<u8>,
    distance_map: Vec<u8>,
    modes: Vec<u8>,
    /// Scratch space for code lengths.
    lengths: Vec<u8>,
}

impl Decoder {
    /// Decodes one Brotli stream at the start of `src`, appending at most `limit` bytes to
    /// `out`. Returns how many bytes of `src` the stream took.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`], [`Error::Corrupt`] or [`Error::TooLarge`]. `out` may hold part of
    /// the output when it fails.
    pub(crate) fn decode(
        &mut self,
        src: &[u8],
        out: &mut Vec<u8>,
        limit: usize,
    ) -> Result<usize, Error> {
        let mut r = BitReader::new(src);
        let mut s = Stream {
            window: (1 << read_window_bits(&mut r)?) - 16,
            base: out.len(),
            limit,
            distances: INITIAL_DISTANCES,
        };
        loop {
            let last = r.bit()?;
            if last && r.bit()? {
                break;
            }
            let nibbles = match r.bits(2)? {
                3 => 0,
                n => n + 4,
            };
            if nibbles == 0 {
                skip_metadata(&mut r)?;
                if last {
                    break;
                }
                continue;
            }
            let mlen = r.bits(4 * nibbles)? as usize + 1;
            if nibbles > 4 && (mlen - 1) >> (4 * (nibbles - 1)) == 0 {
                return Err(Error::Corrupt(
                    "meta-block length has a needless zero nibble",
                ));
            }
            if out.len() - s.base + mlen > s.limit {
                return Err(Error::TooLarge);
            }
            reserve_within(out, mlen, s.base.saturating_add(s.limit));
            if !last && r.bit()? {
                if r.align_to_byte() != 0 {
                    return Err(Error::Corrupt("non-zero padding before uncompressed data"));
                }
                out.extend_from_slice(r.bytes(mlen)?);
                continue;
            }
            self.meta_block(&mut r, &mut s, out, mlen)?;
            if last {
                break;
            }
        }
        if r.align_to_byte() != 0 {
            return Err(Error::Corrupt("non-zero bits after the last meta-block"));
        }
        Ok(r.byte_position())
    }

    /// Reads a compressed meta-block's header (§9.2) and data (§9.3).
    fn meta_block(
        &mut self,
        r: &mut BitReader<'_>,
        s: &mut Stream,
        out: &mut Vec<u8>,
        mlen: usize,
    ) -> Result<(), Error> {
        self.table.clear();
        let mut lit = Category::read(r, &mut self.table, &mut self.lengths)?;
        let mut cmd = Category::read(r, &mut self.table, &mut self.lengths)?;
        let mut dist = Category::read(r, &mut self.table, &mut self.lengths)?;

        let npostfix = r.bits(2)?;
        let ndirect = r.bits(4)? << npostfix;
        self.modes.clear();
        for _ in 0..lit.types {
            self.modes.push(r.bits(2)? as u8);
        }
        let literal_trees = read_count_256(r)?;
        self.read_context_map(r, 64 * lit.types as usize, literal_trees, false)?;
        let distance_trees = read_count_256(r)?;
        self.read_context_map(r, 4 * dist.types as usize, distance_trees, true)?;

        let codes = [
            (literal_trees, 256),
            (cmd.types, 704),
            (distance_trees, 16 + ndirect + (48 << npostfix)),
        ];
        let mut starts = [
            std::mem::take(&mut self.literal_codes),
            std::mem::take(&mut self.command_codes),
            std::mem::take(&mut self.distance_codes),
        ];
        for ((count, alphabet), starts) in codes.into_iter().zip(&mut starts) {
            starts.clear();
            for _ in 0..count {
                starts.push(self.table.len() as u32);
                prefix::read(r, alphabet, &mut self.table, &mut self.lengths)?;
            }
        }
        let [literal_codes, command_codes, distance_codes] = starts;
        let result = self.commands(
            r,
            s,
            out,
            mlen,
            [&mut lit, &mut cmd, &mut dist],
            (npostfix, ndirect),
            [&literal_codes, &command_codes, &distance_codes],
        );
        self.literal_codes = literal_codes;
        self.command_codes = command_codes;
        self.distance_codes = distance_codes;
        result
    }

    /// The commands of a meta-block (§9.3), until `mlen` bytes are out.
    #[allow(
        clippy::too_many_arguments,
        reason = "the decoding loop's state, passed once"
    )]
    fn commands(
        &self,
        r: &mut BitReader<'_>,
        s: &mut Stream,
        out: &mut Vec<u8>,
        mlen: usize,
        [lit, cmd, dist]: [&mut Category; 3],
        (npostfix, ndirect): (u32, u32),
        [literal_codes, cmd_codes, distance_codes]: [&[u32]; 3],
    ) -> Result<(), Error> {
        let table = &self.table[..];
        let end = out.len() + mlen;
        loop {
            cmd.next(table, r)?;
            let code = prefix::decode(table, cmd_codes[cmd.current as usize] as usize, r)?;
            let (insert_code, copy_code) = command_codes(code);
            let insert = INSERT_BASE[insert_code] + r.bits(u32::from(INSERT_EXTRA[insert_code]))?;
            let copy = COPY_BASE[copy_code] + r.bits(u32::from(COPY_EXTRA[copy_code]))?;

            if insert as usize > end - out.len() {
                return Err(Error::Corrupt(
                    "literals run past the end of the meta-block",
                ));
            }
            for _ in 0..insert {
                lit.next(table, r)?;
                let (p1, p2) = s.last_two(out);
                let id = context::literal(self.modes[lit.current as usize], p1, p2);
                let tree = self.literal_map[64 * lit.current as usize + id];
                let byte = prefix::decode(table, literal_codes[tree as usize] as usize, r)?;
                out.push(byte as u8);
            }
            if out.len() == end {
                return Ok(());
            }

            // The distance: implicitly the last one, or a distance code.
            let (distance, push) = if code < 128 {
                (s.distances[0], false)
            } else {
                dist.next(table, r)?;
                let tree = self.distance_map[4 * dist.current as usize + context::distance(copy)];
                let dcode = prefix::decode(table, distance_codes[tree as usize] as usize, r)?;
                s.resolve_distance(dcode, npostfix, ndirect, r)?
            };
            let max = s.window.min(out.len() - s.base);
            if distance as usize > max {
                // A static dictionary word (§8): not remembered in the ring.
                let room = end - out.len();
                dictionary::append_word(out, copy as usize, distance as usize - max - 1, room)?;
                if out.len() == end {
                    return Ok(());
                }
                continue;
            }
            if copy as usize > end - out.len() {
                return Err(Error::Corrupt("a copy runs past the end of the meta-block"));
            }
            if push {
                s.distances = [distance, s.distances[0], s.distances[1], s.distances[2]];
            }
            copy_back(out, distance as usize, copy as usize);
            if out.len() == end {
                return Ok(());
            }
        }
    }

    /// Reads a context map of `size` entries over `trees` codes (§7.3) into the literal or
    /// distance map.
    fn read_context_map(
        &mut self,
        r: &mut BitReader<'_>,
        size: usize,
        trees: u32,
        distances: bool,
    ) -> Result<(), Error> {
        let mut map = std::mem::take(if distances {
            &mut self.distance_map
        } else {
            &mut self.literal_map
        });
        map.clear();
        map.resize(size, 0);
        let result = self.read_context_map_into(r, &mut map, trees);
        if distances {
            self.distance_map = map;
        } else {
            self.literal_map = map;
        }
        result
    }

    fn read_context_map_into(
        &mut self,
        r: &mut BitReader<'_>,
        map: &mut [u8],
        trees: u32,
    ) -> Result<(), Error> {
        if trees < 2 {
            return Ok(());
        }
        let rlemax = if r.bit()? { r.bits(4)? + 1 } else { 0 };
        // The map's code is only needed while reading it.
        let start = self.table.len();
        prefix::read(r, trees + rlemax, &mut self.table, &mut self.lengths)?;
        let mut i = 0;
        while i < map.len() {
            let symbol = prefix::decode(&self.table, start, r)?;
            if symbol == 0 {
                map[i] = 0;
                i += 1;
            } else if symbol <= rlemax {
                let run = (1usize << symbol) + r.bits(symbol)? as usize;
                if run > map.len() - i {
                    self.table.truncate(start);
                    return Err(Error::Corrupt("context map zeros run past its end"));
                }
                map[i..i + run].fill(0);
                i += run;
            } else {
                map[i] = (symbol - rlemax) as u8;
                i += 1;
            }
        }
        self.table.truncate(start);
        if r.bit()? {
            inverse_move_to_front(map);
        }
        Ok(())
    }
}

/// State that lasts the whole stream.
struct Stream {
    /// The largest backward distance (§9.1).
    window: usize,
    /// Where this stream's output starts in `out`.
    base: usize,
    limit: usize,
    /// The last four distances, last first (§4).
    distances: [u32; 4],
}

impl Stream {
    /// The last byte written and the one before it, zero before the stream starts (§7.1).
    #[inline(always)]
    fn last_two(&self, out: &[u8]) -> (u8, u8) {
        let n = out.len() - self.base;
        match n {
            0 => (0, 0),
            1 => (out[out.len() - 1], 0),
            _ => (out[out.len() - 1], out[out.len() - 2]),
        }
    }

    /// The distance for distance code `dcode`, and whether it goes into the ring (§4).
    fn resolve_distance(
        &self,
        dcode: u32,
        npostfix: u32,
        ndirect: u32,
        r: &mut BitReader<'_>,
    ) -> Result<(u32, bool), Error> {
        if dcode < 16 {
            // (ring index, adjustment) for the 16 short codes.
            const SHORT: [(usize, i32); 16] = [
                (0, 0),
                (1, 0),
                (2, 0),
                (3, 0),
                (0, -1),
                (0, 1),
                (0, -2),
                (0, 2),
                (0, -3),
                (0, 3),
                (1, -1),
                (1, 1),
                (1, -2),
                (1, 2),
                (1, -3),
                (1, 3),
            ];
            let (i, adjust) = SHORT[dcode as usize];
            let d = i64::from(self.distances[i]) + i64::from(adjust);
            if d <= 0 {
                return Err(Error::Corrupt(
                    "a short distance code gives a distance below 1",
                ));
            }
            return Ok((d as u32, dcode != 0));
        }
        if dcode < 16 + ndirect {
            return Ok((dcode - 15, true));
        }
        let x = dcode - ndirect - 16;
        let bits = 1 + (x >> (npostfix + 1));
        let hcode = x >> npostfix;
        let lcode = x & ((1 << npostfix) - 1);
        let offset = ((2 + (hcode & 1)) << bits) - 4;
        let extra = r.bits(bits)?;
        Ok((((offset + extra) << npostfix) + lcode + ndirect + 1, true))
    }
}

/// Skips a metadata meta-block (§9.2), after its MNIBBLES.
fn skip_metadata(r: &mut BitReader<'_>) -> Result<(), Error> {
    if r.bit()? {
        return Err(Error::Corrupt("reserved bit set in a metadata block"));
    }
    let skip_bytes = r.bits(2)?;
    let mut skip = 0usize;
    for i in 0..skip_bytes {
        let byte = r.bits(8)?;
        if i + 1 == skip_bytes && skip_bytes > 1 && byte == 0 {
            return Err(Error::Corrupt("metadata length has a needless zero byte"));
        }
        skip |= (byte as usize) << (8 * i);
    }
    if skip_bytes > 0 {
        skip += 1;
    }
    if r.align_to_byte() != 0 {
        return Err(Error::Corrupt("non-zero padding before metadata"));
    }
    r.bytes(skip)?;
    Ok(())
}

/// Makes room for `more` bytes in `out`, growing it geometrically as `Vec` does, but never
/// past a capacity of `cap`: the output limit bounds the memory taken, not just the output.
fn reserve_within(out: &mut Vec<u8>, more: usize, cap: usize) {
    let needed = out.len() + more;
    if needed <= out.capacity() {
        return;
    }
    let target = needed
        .max(out.capacity().saturating_mul(2))
        .min(cap.max(needed));
    out.reserve_exact(target - out.len());
}

/// Bytes a copy moves at a time.
const CHUNK: usize = 16;

/// Appends `len` bytes copied from `distance` bytes back; the source may overlap the
/// destination (§10).
///
/// Copies are mostly short, so a call to `memmove` per copy costs more than the bytes. The
/// copy goes in fixed 16-byte chunks (two moves of eight bytes, no call). Where the capacity
/// reserved has room for a chunk past the end, the last chunk may overshoot and is cut off;
/// otherwise the last bytes go one by one. Nothing is allocated past the reservation, so the
/// output limit still bounds the memory. A distance under 16 repeats a pattern: after its
/// first few bytes, the same bytes are found at the next multiple of the distance of 16 or
/// more, and the chunks copy from there.
#[inline(always)]
fn copy_back(out: &mut Vec<u8>, distance: usize, len: usize) {
    let target = out.len() + len;
    let overshoot = out.capacity() - out.len() >= len + CHUNK;
    let mut distance = distance;
    if distance < CHUNK {
        let wide = distance * CHUNK.div_ceil(distance);
        for _ in 0..(wide - distance).min(len) {
            let b = out[out.len() - distance];
            out.push(b);
        }
        distance = wide;
    }
    if out.len() == target {
        return;
    }
    // The lead is done, so this many bytes back are the pattern's (or the copy's own, for a
    // distance of 16 or more), at least a chunk before the end.
    let mut from = out.len() - distance;
    while out.len() + CHUNK <= target || (overshoot && out.len() < target) {
        let mut chunk = [0u8; CHUNK];
        chunk.copy_from_slice(&out[from..from + CHUNK]);
        out.extend_from_slice(&chunk);
        from += CHUNK;
    }
    if overshoot {
        out.truncate(target);
    } else {
        while out.len() < target {
            let b = out[out.len() - distance];
            out.push(b);
        }
    }
}

/// The inverse move-to-front transform of §7.3.
fn inverse_move_to_front(v: &mut [u8]) {
    let mut mtf: [u8; 256] = std::array::from_fn(|i| i as u8);
    for x in v {
        let index = *x as usize;
        let value = mtf[index];
        *x = value;
        mtf.copy_within(0..index, 1);
        mtf[0] = value;
    }
}

#[cfg(test)]
mod tests {
    use super::{Decoder, copy_back, inverse_move_to_front, read_count_256, read_window_bits};
    use crate::Error;
    use crate::bits::{BitReader, TestBits};

    #[test]
    fn copies_match_a_byte_by_byte_copy() {
        // Every short distance and length, with no spare capacity, a little and plenty.
        let history: Vec<u8> = (0..100u32).map(|i| (i * 37 % 251) as u8).collect();
        for distance in 1..=40 {
            for len in 0..=70 {
                let mut expected = history.clone();
                for _ in 0..len {
                    expected.push(expected[expected.len() - distance]);
                }
                for spare in [0, len, len + 15, len + 16, len + 100] {
                    let mut out = Vec::with_capacity(history.len() + spare);
                    out.extend_from_slice(&history);
                    let capacity = out.capacity();
                    copy_back(&mut out, distance, len);
                    assert_eq!(
                        out, expected,
                        "distance {distance}, length {len}, spare {spare}"
                    );
                    if spare >= len {
                        assert_eq!(out.capacity(), capacity, "grew: {distance}, {len}, {spare}");
                    }
                }
            }
        }
    }

    fn decode(bytes: &[u8]) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        let used = Decoder::default().decode(bytes, &mut out, usize::MAX)?;
        assert_eq!(used, bytes.len(), "the whole input is the stream");
        Ok(out)
    }

    #[test]
    fn window_bits_follow_the_rfc_table() {
        // (bit pattern as printed in §9.1, read right to left, value)
        for (pattern, value) in [
            ("0100001", 10),
            ("0110001", 11),
            ("1000001", 12),
            ("1010001", 13),
            ("1100001", 14),
            ("1110001", 15),
            ("0", 16),
            ("0000001", 17),
            ("0011", 18),
            ("0101", 19),
            ("0111", 20),
            ("1001", 21),
            ("1011", 22),
            ("1101", 23),
            ("1111", 24),
        ] {
            let mut w = TestBits::default();
            for c in pattern.chars().rev() {
                w.put(1, u32::from(c == '1'));
            }
            let bytes = w.finish();
            assert_eq!(
                read_window_bits(&mut BitReader::new(&bytes)),
                Ok(value),
                "{pattern}"
            );
        }
        let mut w = TestBits::default();
        for c in "0010001".chars().rev() {
            w.put(1, u32::from(c == '1'));
        }
        assert_eq!(
            read_window_bits(&mut BitReader::new(&w.finish())),
            Err(Error::Corrupt("large-window Brotli is not supported"))
        );
    }

    /// Maps as the encoder writes them, read back: NTREES, then the map.
    fn read_back(map: &[u8], trees: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut w = crate::bitwriter::BitWriter::new(&mut out);
        crate::encode::write_context_map(&mut w, map, trees);
        w.finish();
        let mut r = BitReader::new(&out);
        assert_eq!(read_count_256(&mut r), Ok(trees as u32));
        let mut got = vec![0; map.len()];
        Decoder::default()
            .read_context_map_into(&mut r, &mut got, trees as u32)
            .unwrap();
        got
    }

    #[test]
    fn context_maps_take_the_largest_run_prefix_up_to_6() {
        // The prefix is the longest zero run's (after move-to-front) bit length less one, at
        // most 6; none for runs of one.
        for (zeros, prefix) in [(0, 0), (1, 0), (2, 1), (5, 2), (63, 5), (64, 6), (500, 6)] {
            // 0 then 1 repeated: move-to-front makes the repeats zeros. Then 2, 3, ... to end.
            let map: Vec<u8> = [0]
                .into_iter()
                .chain(std::iter::repeat_n(1, zeros + 1))
                .chain(2..10)
                .collect();
            let mut out = Vec::new();
            let mut w = crate::bitwriter::BitWriter::new(&mut out);
            crate::encode::write_context_map(&mut w, &map, 10);
            w.finish();
            let mut r = BitReader::new(&out);
            assert_eq!(read_count_256(&mut r), Ok(10));
            let got = if r.bit().unwrap() {
                r.bits(4).unwrap() + 1
            } else {
                0
            };
            assert_eq!(got, prefix, "a run of {zeros}");
            assert_eq!(read_back(&map, 10), map);
        }
    }

    #[test]
    fn context_maps_the_encoder_writes_read_back() {
        use crate::encode::context::{COMPLEX, SIMPLE};
        // The static maps, for every number of block types they can have.
        for (static_map, n) in [(&SIMPLE, 2), (&COMPLEX, 13)] {
            for types in 1..=256 / n {
                let map: Vec<u8> = (0..types)
                    .flat_map(|t| static_map.iter().map(move |&c| (t * n) as u8 + c))
                    .collect();
                assert_eq!(
                    read_back(&map, types * n),
                    map,
                    "{n} contexts, {types} types"
                );
            }
        }
        // Random maps of 1 to 256 trees, each tree used, some with long runs of one tree.
        let mut x = 12_345u32;
        let mut next = |m: usize| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as usize % m
        };
        for trees in 1..=256usize {
            let len = 64 * (1 + next(8)).max(trees.div_ceil(64));
            let mut map: Vec<u8> = (0..len).map(|i| (i % trees) as u8).collect();
            for i in (1..len).rev() {
                map.swap(i, next(i + 1));
            }
            if trees % 3 == 0 {
                // A run as long as a meta-block's map can have: zeros after move-to-front,
                // more than one longest-run symbol says.
                let at = next(len);
                let run = next(len - at) + 1;
                let v = map[at];
                map[at..at + run].fill(v);
            }
            assert_eq!(read_back(&map, trees), map, "{trees} trees");
        }
        // Zero runs around what one run-length symbol says at the largest prefix, 6: up to
        // 127, and 128 on takes two.
        for zeros in [62, 63, 64, 126, 127, 128, 129, 254, 255, 256, 257, 1000] {
            let map: Vec<u8> = std::iter::once(0).chain(vec![1; zeros + 1]).collect();
            assert_eq!(read_back(&map, 2), map, "a run of {zeros}");
        }
        // The longest run: one tree's 64 * 256 entries but the first.
        let mut map = vec![1u8; 64 * 256];
        map[0] = 0;
        assert_eq!(read_back(&map, 2), map);
    }

    #[test]
    fn counts_up_to_256_follow_the_rfc_table() {
        // (pattern as printed in §9.2 with x bits filled, value); 0110111 is 12.
        for (pattern, value) in [
            ("0", 1),
            ("0001", 2),
            ("00011", 3),
            ("10011", 4),
            ("0110111", 12),
            ("1111111101", 128),
            ("11111111111", 256),
            ("00000001111", 129),
        ] {
            let mut w = TestBits::default();
            for c in pattern.chars().rev() {
                w.put(1, u32::from(c == '1'));
            }
            let bytes = w.finish();
            assert_eq!(
                read_count_256(&mut BitReader::new(&bytes)),
                Ok(value),
                "{pattern}"
            );
        }
    }

    #[test]
    fn inverse_move_to_front_matches_the_rfc_function() {
        let mut v = [0, 1, 1, 0, 2, 0, 3, 1];
        inverse_move_to_front(&mut v);
        // Each index picks a value from the list, which then moves to the front:
        // 0 -> 0 [0 1 2 3]; 1 -> 1 [1 0 2 3]; 1 -> 0 [0 1 2 3]; 0 -> 0;
        // 2 -> 2 [2 0 1 3]; 0 -> 2; 3 -> 3 [3 2 0 1]; 1 -> 2 [2 3 0 1].
        assert_eq!(v, [0, 1, 0, 0, 2, 2, 3, 2]);
    }

    /// WBITS 16 (one 0 bit).
    fn header(w: &mut TestBits) {
        w.put(1, 0);
    }

    #[test]
    fn the_empty_stream() {
        let mut w = TestBits::default();
        header(&mut w);
        w.put(1, 1).put(1, 1); // ISLAST, ISLASTEMPTY
        assert_eq!(decode(&w.finish()).unwrap(), b"");
    }

    #[test]
    fn an_uncompressed_meta_block() {
        let mut w = TestBits::default();
        header(&mut w);
        // ISLAST 0, MNIBBLES 4 (00), MLEN-1 = 4, ISUNCOMPRESSED 1, pad, bytes.
        w.put(1, 0).put(2, 0).put(16, 4).put(1, 1).align();
        for &b in b"hello" {
            w.put(8, u32::from(b));
        }
        w.put(1, 1).put(1, 1);
        assert_eq!(decode(&w.finish()).unwrap(), b"hello");
    }

    #[test]
    fn padding_before_uncompressed_data_must_be_zero() {
        let mut w = TestBits::default();
        header(&mut w);
        w.put(1, 0).put(2, 0).put(16, 0).put(1, 1);
        w.put(1, 1).align(); // a set padding bit
        w.put(8, u32::from(b'x')).put(1, 1).put(1, 1);
        assert_eq!(
            decode(&w.finish()),
            Err(Error::Corrupt("non-zero padding before uncompressed data"))
        );
    }

    #[test]
    fn metadata_is_skipped() {
        let mut w = TestBits::default();
        header(&mut w);
        // ISLAST 0, MNIBBLES 0 (11), reserved 0, MSKIPBYTES 1, MSKIPLEN-1 = 2, pad, 3 bytes.
        w.put(1, 0).put(2, 3).put(1, 0).put(2, 1).put(8, 2).align();
        w.put(8, 0xaa).put(8, 0xbb).put(8, 0xcc);
        // Then one uncompressed byte and the end.
        w.put(1, 0)
            .put(2, 0)
            .put(16, 0)
            .put(1, 1)
            .align()
            .put(8, u32::from(b'z'));
        w.put(1, 1).put(1, 1);
        assert_eq!(decode(&w.finish()).unwrap(), b"z");
    }

    #[test]
    fn metadata_rules() {
        // A set reserved bit.
        let mut w = TestBits::default();
        header(&mut w);
        w.put(1, 0).put(2, 3).put(1, 1).put(2, 0);
        assert_eq!(
            decode(&w.finish()),
            Err(Error::Corrupt("reserved bit set in a metadata block"))
        );
        // MSKIPBYTES 2 with a zero last byte.
        let mut w = TestBits::default();
        header(&mut w);
        w.put(1, 0)
            .put(2, 3)
            .put(1, 0)
            .put(2, 2)
            .put(8, 5)
            .put(8, 0);
        assert_eq!(
            decode(&w.finish()),
            Err(Error::Corrupt("metadata length has a needless zero byte"))
        );
        // A last metadata block ends the stream.
        let mut w = TestBits::default();
        header(&mut w);
        w.put(1, 1).put(1, 0).put(2, 3).put(1, 0).put(2, 0).align();
        assert_eq!(decode(&w.finish()).unwrap(), b"");
    }

    #[test]
    fn a_needless_zero_nibble_is_corrupt() {
        let mut w = TestBits::default();
        header(&mut w);
        // MNIBBLES 5 (01) with MLEN-1 = 0x0ffff: its top nibble is zero.
        w.put(1, 0).put(2, 1).put(20, 0xffff);
        assert_eq!(
            decode(&w.finish()),
            Err(Error::Corrupt(
                "meta-block length has a needless zero nibble"
            ))
        );
    }

    #[test]
    fn bits_after_the_last_meta_block_must_be_zero() {
        let mut w = TestBits::default();
        header(&mut w);
        w.put(1, 1).put(1, 1).put(1, 1);
        assert_eq!(
            decode(&w.finish()),
            Err(Error::Corrupt("non-zero bits after the last meta-block"))
        );
    }

    #[test]
    fn the_limit_is_checked_before_the_data() {
        let mut w = TestBits::default();
        header(&mut w);
        w.put(1, 0).put(2, 0).put(16, 9).put(1, 1).align();
        let mut out = Vec::new();
        assert_eq!(
            Decoder::default().decode(&w.finish(), &mut out, 9),
            Err(Error::TooLarge)
        );
        assert!(out.capacity() < 10, "nothing was reserved for the data");
    }

    /// The start of a stream with one compressed meta-block of `mlen` bytes: WBITS 16, ISLAST,
    /// one block type per category, NPOSTFIX 0, NDIRECT 0, context mode LSB6, and one literal
    /// and one distance tree. The literal, command and distance codes follow.
    fn compressed_header(w: &mut TestBits, mlen: u32) {
        header(w);
        w.put(1, 1).put(1, 0).put(2, 0).put(16, mlen - 1);
        w.put(1, 0).put(1, 0).put(1, 0); // NBLTYPESL, NBLTYPESI, NBLTYPESD: 1
        w.put(2, 0).put(4, 0); // NPOSTFIX, NDIRECT
        w.put(2, 0); // context mode
        w.put(1, 0).put(1, 0); // NTREESL, NTREESD: 1
    }

    /// A simple prefix code over `symbols` (1 or 2) of `bits`-bit symbols.
    fn simple(w: &mut TestBits, bits: u32, symbols: &[u32]) {
        w.put(2, 1).put(2, symbols.len() as u32 - 1);
        for &s in symbols {
            w.put(bits, s);
        }
    }

    // Insert-and-copy codes: cell 2 (128..192) is insert codes 0..8 and copy codes 0..8 with
    // an explicit distance.
    const INSERT_3_COPY_2: u32 = 128 + (3 << 3);
    const INSERT_0_COPY_2: u32 = 128;

    #[test]
    fn a_compressed_meta_block_with_a_copy() {
        // Insert "aaa", then copy 2 bytes from distance code 4 (last distance - 1 = 3).
        let mut w = TestBits::default();
        compressed_header(&mut w, 5);
        simple(&mut w, 8, &[u32::from(b'a')]);
        simple(&mut w, 10, &[INSERT_3_COPY_2]);
        simple(&mut w, 6, &[4]);
        // The command, literals and distance take no bits: every code has one symbol.
        assert_eq!(decode(&w.finish()).unwrap(), b"aaaaa");
    }

    #[test]
    fn literals_past_the_meta_block_are_corrupt() {
        let mut w = TestBits::default();
        compressed_header(&mut w, 2);
        simple(&mut w, 8, &[u32::from(b'a')]);
        simple(&mut w, 10, &[INSERT_3_COPY_2]);
        simple(&mut w, 6, &[4]);
        assert_eq!(
            decode(&w.finish()),
            Err(Error::Corrupt(
                "literals run past the end of the meta-block"
            ))
        );
    }

    #[test]
    fn a_copy_past_the_meta_block_is_corrupt() {
        let mut w = TestBits::default();
        compressed_header(&mut w, 4);
        simple(&mut w, 8, &[u32::from(b'a')]);
        simple(&mut w, 10, &[INSERT_3_COPY_2]);
        simple(&mut w, 6, &[4]);
        assert_eq!(
            decode(&w.finish()),
            Err(Error::Corrupt("a copy runs past the end of the meta-block"))
        );
    }

    #[test]
    fn a_short_code_below_distance_one_is_corrupt() {
        // Command 1: insert "aaa", copy 2 from distance code 16 with extra bit 0: distance 1,
        // which becomes the last distance. Command 2: copy 2 from code 8, last distance - 3.
        let mut w = TestBits::default();
        compressed_header(&mut w, 10);
        simple(&mut w, 8, &[u32::from(b'a')]);
        simple(&mut w, 10, &[INSERT_3_COPY_2, INSERT_0_COPY_2]);
        simple(&mut w, 6, &[16, 8]);
        // Two-symbol codes: the smaller symbol is code 0, the larger code 1.
        w.put(1, 1).put(1, 1).put(1, 0); // command 152, distance 16, extra bit
        w.put(1, 0).put(1, 0); // command 128, distance 8
        assert_eq!(
            decode(&w.finish()),
            Err(Error::Corrupt(
                "a short distance code gives a distance below 1"
            ))
        );
    }

    #[test]
    fn distance_code_zero_is_not_remembered() {
        // Literals x (code 0) and y (code 1). Command 1: "xyy", copy 2 at code 4 (last
        // distance - 1 = 3, remembered). Command 2: "xyx", copy 2 at code 0 (the last distance,
        // 3, not remembered again). Command 3: copy 2 at code 1, the second-to-last: 4 gives
        // "yx"; had code 0 been remembered, it would be 3 and give "xx".
        let mut w = TestBits::default();
        compressed_header(&mut w, 12);
        simple(&mut w, 8, &[u32::from(b'x'), u32::from(b'y')]);
        simple(&mut w, 10, &[INSERT_0_COPY_2, INSERT_3_COPY_2]);
        // Distance symbols 4, 0, 1: NSYM 3, lengths 1, 2, 2 in listed order, so 4 is code 0
        // and 0, 1 are codes 10, 11.
        w.put(2, 1).put(2, 2).put(6, 4).put(6, 0).put(6, 1);
        w.put(1, 1).put(1, 0).put(1, 1).put(1, 1).code(0, 1);
        w.put(1, 1).put(1, 0).put(1, 1).put(1, 0).code(0b10, 2);
        w.put(1, 0).code(0b11, 2);
        assert_eq!(decode(&w.finish()).unwrap(), b"xyyxyxyxxyyx");
    }

    #[test]
    fn literal_block_switches() {
        // Three literal block types, each mapped to its own tree: 'a', 'b' and 'c'. Blocks of
        // one literal; the switches are +1, +1, +1 (wrapping to 0), previous, and type 0.
        let mut w = TestBits::default();
        header(&mut w);
        w.put(1, 1).put(1, 0).put(2, 0).put(16, 6 - 1);
        // NBLTYPESL 3 (bit 1, 001, x = 0).
        w.put(1, 1).put(3, 1).put(1, 0);
        // Block type code over 5 symbols: 0 (previous), 1 (+1), 2 and 4 (types 0 and 2), all
        // length 2, so codes 00, 01, 10, 11.
        w.put(2, 1)
            .put(2, 3)
            .put(3, 0)
            .put(3, 1)
            .put(3, 2)
            .put(3, 4)
            .put(1, 0);
        // Block count code: only code 0 (counts 1..=4 from 2 extra bits); the first count.
        simple(&mut w, 5, &[0]);
        w.put(2, 0);
        w.put(1, 0).put(1, 0); // NBLTYPESI, NBLTYPESD: 1
        w.put(2, 0).put(4, 0); // NPOSTFIX, NDIRECT
        w.put(2, 0).put(2, 0).put(2, 0); // context modes
        // NTREESL 3; the map: RLEMAX 0, a code over 0, 1, 2 (lengths 1, 2, 2: 0, 10, 11),
        // then 64 entries per type, no IMTF.
        w.put(1, 1).put(3, 1).put(1, 0);
        w.put(1, 0);
        w.put(2, 1).put(2, 2).put(2, 0).put(2, 1).put(2, 2);
        for (code, len) in [(0, 1), (0b10, 2), (0b11, 2)] {
            for _ in 0..64 {
                w.code(code, len);
            }
        }
        w.put(1, 0);
        w.put(1, 0); // NTREESD 1
        for literal in *b"abc" {
            simple(&mut w, 8, &[u32::from(literal)]);
        }
        // One command: insert code 6 (6 + 1 extra bit), copy code 0, implicit distance: the
        // meta-block ends after the literals, so the copy is ignored.
        simple(&mut w, 10, &[6 << 3]);
        simple(&mut w, 6, &[0]);
        w.put(1, 0); // insert extra bit: 6 literals
        // Literal 1 is in the first block; each next one switches (type code, count extra).
        for code in [0b01, 0b01, 0b01, 0b00, 0b10] {
            w.code(code, 2).put(2, 0);
        }
        // Types: 0, 1, 2, 0 (wrapped), 2 (previous), 0 (symbol 2).
        assert_eq!(decode(&w.finish()).unwrap(), b"abcaca");
    }

    /// Insert nothing, copy 4 bytes, explicit distance (cell 2: copy code 2 is length 4).
    const INSERT_0_COPY_4: u32 = 128 + 2;

    /// A stream with one command that copies 4 bytes from distance code `dcode` with
    /// `extra_bits` extra bits `extra`, in a meta-block of `mlen` bytes.
    fn one_copy(mlen: u32, dcode: u32, extra_bits: u32, extra: u32) -> Vec<u8> {
        let mut w = TestBits::default();
        compressed_header(&mut w, mlen);
        simple(&mut w, 8, &[u32::from(b'a')]);
        simple(&mut w, 10, &[INSERT_0_COPY_4]);
        simple(&mut w, 6, &[dcode]);
        w.put(extra_bits, extra);
        w.finish()
    }

    #[test]
    fn a_dictionary_word() {
        // At the start nothing is in the window, so distance 1 (code 16, one extra bit 0) is
        // word 0 of length 4, transform 0: "time".
        assert_eq!(decode(&one_copy(4, 16, 1, 0)).unwrap(), b"time");
    }

    #[test]
    fn a_transform_past_120_is_corrupt() {
        // Word id 121 << 10 (length 4 has 2^10 words) is distance 123905: code 45 (x = 29,
        // 15 extra bits, offset 3 * 2^15 - 4) with extra bits 123904 - 98300.
        assert_eq!(
            decode(&one_copy(4, 45, 15, 123_904 - 98_300)),
            Err(Error::Corrupt(
                "a dictionary reference with a transform past 120"
            ))
        );
        // One less is transform 120 (" ", FermentFirst, "='") of the last word of length 4,
        // d9 88 d8 b4: FermentFirst on a 2-byte character flips 0x20 in its second byte.
        assert_eq!(
            decode(&one_copy(7, 45, 15, 123_903 - 98_300)).unwrap(),
            b" \xd9\xa8\xd8\xb4='"
        );
    }

    #[test]
    fn a_dictionary_word_past_the_meta_block_is_corrupt() {
        assert_eq!(
            decode(&one_copy(3, 16, 1, 0)),
            Err(Error::Corrupt(
                "a dictionary word runs past the end of the meta-block"
            ))
        );
    }

    #[test]
    fn a_context_map_run_past_its_end_is_corrupt() {
        let mut w = TestBits::default();
        header(&mut w);
        w.put(1, 1).put(1, 0).put(2, 0).put(16, 0);
        w.put(1, 0)
            .put(1, 0)
            .put(1, 0)
            .put(2, 0)
            .put(4, 0)
            .put(2, 0);
        // NTREESL 2 (bits 1, 000), RLEMAX 6 (1, then 6 - 1 in 4 bits), a code whose only
        // symbol is 6: a run of 64 + 6 extra bits zeros, into a 64-entry map.
        w.put(1, 1).put(3, 0).put(1, 1).put(4, 5);
        simple(&mut w, 3, &[6]);
        w.put(6, 1);
        assert_eq!(
            decode(&w.finish()),
            Err(Error::Corrupt("context map zeros run past its end"))
        );
    }

    #[test]
    fn the_buffer_never_grows_past_the_limit() {
        // Three uncompressed meta-blocks of 300,000 bytes against a 1 MiB limit: doubling
        // the buffer for the third would take 1.2 MB.
        let mut w = TestBits::default();
        header(&mut w);
        for _ in 0..3 {
            w.put(1, 0).put(2, 1).put(20, 300_000 - 1).put(1, 1).align();
            for _ in 0..300_000 {
                w.put(8, u32::from(b'q'));
            }
        }
        w.put(1, 1).put(1, 1);
        let mut out = Vec::new();
        Decoder::default()
            .decode(&w.finish(), &mut out, 1 << 20)
            .unwrap();
        assert_eq!(out.len(), 900_000);
        assert!(out.capacity() <= 1 << 20, "{} bytes taken", out.capacity());
    }

    #[test]
    fn the_code_table_is_reused_between_meta_blocks() {
        // The same compressed meta-block once and 100 times: the table holds one meta-block's
        // codes either way.
        let stream = |blocks: usize| {
            let mut w = TestBits::default();
            header(&mut w);
            for i in 0..blocks {
                let last = u32::from(i + 1 == blocks);
                w.put(1, last);
                if last == 1 {
                    w.put(1, 0);
                }
                w.put(2, 0).put(16, 5 - 1);
                if last == 0 {
                    w.put(1, 0); // ISUNCOMPRESSED
                }
                w.put(1, 0)
                    .put(1, 0)
                    .put(1, 0)
                    .put(2, 0)
                    .put(4, 0)
                    .put(2, 0);
                w.put(1, 0).put(1, 0);
                simple(&mut w, 8, &[u32::from(b'a')]);
                simple(&mut w, 10, &[INSERT_3_COPY_2]);
                // Distance code 16 with extra bit 0 is distance 1 every time (code 4 would
                // count down: the ring lasts the whole stream).
                simple(&mut w, 6, &[16]);
                w.put(1, 0);
            }
            w.finish()
        };
        let table_after = |blocks: usize| {
            let mut d = Decoder::default();
            let mut out = Vec::new();
            d.decode(&stream(blocks), &mut out, usize::MAX).unwrap();
            assert_eq!(out, b"aaaaa".repeat(blocks));
            d.table.len()
        };
        assert_eq!(table_after(100), table_after(1));
    }

    #[test]
    fn a_truncated_stream_is_truncation() {
        let mut w = TestBits::default();
        header(&mut w);
        w.put(1, 0).put(2, 0).put(16, 4).put(1, 1).align();
        for &b in b"hello" {
            w.put(8, u32::from(b));
        }
        w.put(1, 1).put(1, 1);
        let bytes = w.finish();
        for len in 0..bytes.len() {
            assert_eq!(decode(&bytes[..len]), Err(Error::Truncated), "{len} bytes");
        }
    }
}
