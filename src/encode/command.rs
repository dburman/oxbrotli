//! Insert-and-copy commands (RFC 7932 §5) and distances (§4): their codes, and the encoder's
//! copy of the decoder's distance ring.

/// A distance code and its extra bits (§4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DistanceCode {
    pub(crate) code: u8,
    pub(crate) extra_bits: u8,
    pub(crate) extra: u32,
}

impl DistanceCode {
    /// Distance code 0: the last distance again.
    pub(crate) const LAST: Self = Self {
        code: 0,
        extra_bits: 0,
        extra: 0,
    };

    /// The code for `distance` written out (not from the ring), with `NPOSTFIX = 0` and
    /// `NDIRECT = 0`. It runs §4's formula backwards:
    ///
    /// `distance = ((2 + (hcode & 1)) << ndistbits) - 4 + extra + 1`
    ///
    /// with `hcode = code - 16` and `ndistbits = 1 + (hcode >> 1)`.
    pub(crate) fn explicit(distance: u32) -> Self {
        debug_assert!(
            (1..=MAX_DISTANCE).contains(&distance),
            "distance {distance}"
        );
        let x = distance + 3;
        let ndistbits = x.ilog2() - 1;
        let odd = (x >> ndistbits) & 1;
        let hcode = 2 * (ndistbits - 1) + odd;
        Self {
            code: (16 + hcode) as u8,
            extra_bits: ndistbits as u8,
            extra: x - ((2 + odd) << ndistbits),
        }
    }
}

/// The largest distance code 63 can say: `(3 << 24) - 4 + (2^24 - 1) + 1`.
pub(crate) const MAX_DISTANCE: u32 = (1 << 26) - 4;

/// One insert-and-copy command (§5): `insert` literals, then `copy` bytes from `distance`
/// back. A `copy` of 0 is no copy: only the last command of a meta-block, ending after its
/// inserts, may have it (the decoder then reads no distance). For a dictionary word, `copy` is
/// the word's length and `cut` the bytes its transform drops from the end, so the copy writes
/// `copy - cut` bytes; otherwise `cut` is 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Command {
    pub(crate) insert: u32,
    pub(crate) copy: u32,
    pub(crate) distance: DistanceCode,
    pub(crate) cut: u8,
}

impl Command {
    /// The bytes the copy writes.
    pub(crate) fn copied(&self) -> usize {
        (self.copy - u32::from(self.cut)) as usize
    }
}

/// The length code for `len` from a table of the first length of each code: what
/// `insert_code` and `copy_code` work out.
#[cfg(test)]
fn length_code(base: &[u32; 24], len: u32) -> usize {
    base.partition_point(|&b| b <= len) - 1
}

/// The insert length code (§5) for `len`. Past the first six, codes come in pairs whose extra
/// bits grow by one a pair: from 6, a pair covers `2 << pair` lengths per code, so `len - 2`
/// has its top bit at `pair + 2`, and the bit below picks the code of the pair. From 130 each
/// code has a bit more than the last, then three wide ones.
pub(crate) fn insert_code(len: u32) -> usize {
    match len {
        0..6 => len as usize,
        6..130 => {
            let extra = (len - 2).ilog2() - 1;
            (2 * extra + ((len - 2) >> extra) + 2) as usize
        }
        130..2114 => ((len - 66).ilog2() + 10) as usize,
        2114..6210 => 21,
        6210..22594 => 22,
        _ => 23,
    }
}

/// The copy length code (§5) for `len`, from 2: as `insert_code`, with the pairs from 10 and
/// the codes of a bit more each from 134.
pub(crate) fn copy_code(len: u32) -> usize {
    match len {
        0..10 => len.saturating_sub(2) as usize,
        10..134 => {
            let extra = (len - 6).ilog2() - 1;
            (2 * extra + ((len - 6) >> extra) + 4) as usize
        }
        134..2118 => ((len - 70).ilog2() + 12) as usize,
        _ => 23,
    }
}

/// The cell (§5) of insert codes `8 * i..` and copy codes `8 * c..` among cells 2..=10, the
/// ones that read a distance.
const EXPLICIT_CELL: [[u16; 3]; 3] = [[2, 3, 6], [4, 5, 8], [7, 9, 10]];

/// The insert-and-copy code for insert code `ic` and copy code `cc`, in cell 0 or 1 (the
/// last distance, none read) when `last_distance` and the codes allow it.
fn command_code(ic: usize, cc: usize, last_distance: bool) -> u16 {
    let (i, c) = (ic >> 3, cc >> 3);
    let cell = if last_distance && i == 0 && c < 2 {
        c as u16
    } else {
        EXPLICIT_CELL[i][c]
    };
    cell * 64 + ((ic as u16 & 7) << 3) + (cc as u16 & 7)
}

/// A command as it is written: its code and length codes.
pub(crate) struct Coded {
    pub(crate) code: u16,
    pub(crate) insert_code: u8,
    pub(crate) copy_code: u8,
}

impl Coded {
    pub(crate) fn new(c: &Command) -> Self {
        let insert_code = insert_code(c.insert);
        // No copy: copy code 0, whose length the decoder never gets to.
        let copy_code = if c.copy == 0 { 0 } else { copy_code(c.copy) };
        let last = c.copy == 0 || c.distance.code == 0;
        Self {
            code: command_code(insert_code, copy_code, last),
            insert_code: insert_code as u8,
            copy_code: copy_code as u8,
        }
    }

    /// Whether a distance code follows the literals.
    pub(crate) fn reads_distance(&self, c: &Command) -> bool {
        c.copy != 0 && self.code >= 128
    }
}

/// The decoder's ring of the last four distances (§4), as the encoder tracks it: most recent
/// first, starting from 4, 11, 15, 16. Distance code 0 and dictionary references leave it as it
/// is; every other distance goes on the front.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ring(pub(crate) [u32; 4]);

impl Ring {
    pub(crate) const START: Self = Self([4, 11, 15, 16]);

    /// The most recent distance.
    pub(crate) fn last(&self) -> u32 {
        self.0[0]
    }

    /// The short code (0..=15, §4) that says `distance`, if one does. Tried in the order the
    /// reference tries them (`c/enc/backward_references.c`, `ComputeDistanceCode`): the last
    /// two distances, then the last one give or take up to 3 (codes 4 to 9), then the one
    /// before it give or take up to 3 (10 to 15), then the third and fourth.
    pub(crate) fn short_code(&self, distance: u32) -> Option<u8> {
        let [first, second, third, fourth] = self.0;
        // Codes 4..=9 are last -1, +1, -2, +2, -3, +3; 10..=15 the same around the second.
        let near = |base: u32, codes: u8| {
            let delta = i64::from(distance) - i64::from(base);
            match delta {
                -3..=-1 => Some(codes + 2 * (-delta - 1) as u8),
                1..=3 => Some(codes + 2 * (delta - 1) as u8 + 1),
                _ => None,
            }
        };
        if distance == first {
            Some(0)
        } else if distance == second {
            Some(1)
        } else if let Some(code) = near(first, 4).or_else(|| near(second, 10)) {
            Some(code)
        } else if distance == third {
            Some(2)
        } else if distance == fourth {
            Some(3)
        } else {
            None
        }
    }

    /// The distance short code `code` (1..=15) stands for: what the decoder computes.
    #[cfg(test)]
    pub(crate) fn resolve(&self, code: u8) -> i64 {
        let [first, second, third, fourth] = self.0.map(i64::from);
        match code {
            0 => first,
            1 => second,
            2 => third,
            3 => fourth,
            4..=9 => first + [-1, 1, -2, 2, -3, 3][code as usize - 4],
            _ => second + [-1, 1, -2, 2, -3, 3][code as usize - 10],
        }
    }

    /// Puts `distance` on the front.
    pub(crate) fn push(&mut self, distance: u32) {
        self.0 = [distance, self.0[0], self.0[1], self.0[2]];
    }

    /// The code for a backward `distance` (within the window), putting it on the ring as the
    /// decoder will.
    pub(crate) fn encode(&mut self, distance: u32) -> DistanceCode {
        match self.short_code(distance) {
            Some(0) => DistanceCode::LAST,
            Some(code) => {
                self.push(distance);
                DistanceCode {
                    code,
                    extra_bits: 0,
                    extra: 0,
                }
            }
            None => {
                self.push(distance);
                DistanceCode::explicit(distance)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DistanceCode, EXPLICIT_CELL, MAX_DISTANCE, Ring, command_code, copy_code, insert_code,
        length_code,
    };
    use crate::tables::{COMMAND_CELLS, COPY_BASE, INSERT_BASE, command_codes};

    #[test]
    fn length_codes_are_the_tables() {
        // Every length to 30,000, each code's first length and the one before it, and the
        // longest the decoder takes.
        let lengths = (0..30_000)
            .chain(
                INSERT_BASE
                    .iter()
                    .chain(&COPY_BASE)
                    .flat_map(|&b| [b.max(1) - 1, b, b + 1]),
            )
            .chain([1 << 20, 16_799_808, 16_799_809, 16_779_332, 16_779_333]);
        for len in lengths {
            assert_eq!(
                insert_code(len),
                length_code(&INSERT_BASE, len),
                "insert {len}"
            );
            if len >= 2 {
                assert_eq!(copy_code(len), length_code(&COPY_BASE, len), "copy {len}");
            }
        }
    }

    #[test]
    fn every_command_code_is_found_again() {
        for code in 0..704u32 {
            let (ic, cc) = command_codes(code);
            assert_eq!(
                u32::from(command_code(ic, cc, code < 128)),
                code,
                "insert code {ic}, copy code {cc}"
            );
        }
        for (i, row) in EXPLICIT_CELL.iter().enumerate() {
            for (c, &cell) in row.iter().enumerate() {
                assert_eq!(
                    COMMAND_CELLS[cell as usize],
                    (8 * i as u8, 8 * c as u8),
                    "cell {cell}"
                );
            }
        }
    }

    /// The distance code 16 + NDIRECT.. decodes back (§4, with NPOSTFIX = NDIRECT = 0).
    fn distance_of(d: DistanceCode) -> u32 {
        let hcode = u32::from(d.code) - 16;
        let ndistbits = 1 + (hcode >> 1);
        assert_eq!(u32::from(d.extra_bits), ndistbits);
        assert!(d.extra < 1 << ndistbits);
        ((2 + (hcode & 1)) << ndistbits) - 4 + d.extra + 1
    }

    #[test]
    fn explicit_distances_decode_back() {
        let mut d = 1;
        while d <= MAX_DISTANCE {
            assert_eq!(distance_of(DistanceCode::explicit(d)), d);
            d += 1 + d / 64;
        }
        assert_eq!(DistanceCode::explicit(1).code, 16);
        assert_eq!(DistanceCode::explicit(MAX_DISTANCE).code, 63);
    }

    #[test]
    fn short_codes_say_the_distance_they_are_chosen_for() {
        let mut ring = Ring([100, 40, 7, 1000]);
        for distance in 1..1200 {
            if let Some(code) = ring.short_code(distance) {
                assert_eq!(ring.resolve(code), i64::from(distance), "code {code}");
            }
        }
        // The order: the last distances first, then near the last, then the older ones.
        assert_eq!(ring.short_code(100), Some(0));
        assert_eq!(ring.short_code(40), Some(1));
        assert_eq!(ring.short_code(99), Some(4));
        assert_eq!(ring.short_code(103), Some(9));
        assert_eq!(ring.short_code(38), Some(12));
        assert_eq!(ring.short_code(7), Some(2));
        assert_eq!(ring.short_code(1000), Some(3));
        assert_eq!(ring.short_code(500), None);
        // Near the last distance beats being the third one.
        ring = Ring([10, 50, 12, 90]);
        assert_eq!(ring.short_code(12), Some(5 + 2));
    }

    #[test]
    fn the_ring_moves_as_the_decoders_does() {
        let mut ring = Ring::START;
        assert_eq!(ring.encode(4), DistanceCode::LAST);
        assert_eq!(ring, Ring::START, "code 0 leaves the ring");
        // A short code other than 0 pushes the distance it stands for, even one on the ring.
        assert_eq!(ring.encode(11).code, 1);
        assert_eq!(ring, Ring([11, 4, 11, 15]));
        assert_eq!(ring.encode(500).code, DistanceCode::explicit(500).code);
        assert_eq!(ring, Ring([500, 11, 4, 11]));
        assert_eq!(ring.encode(501).code, 5);
        assert_eq!(ring, Ring([501, 500, 11, 4]));
    }
}
