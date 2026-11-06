//! The fixed tables of RFC 7932 §5 (insert and copy lengths) and §6 (block counts).

/// Insert length codes 0..=23 (§5): the first length of each code.
pub(crate) const INSERT_BASE: [u32; 24] = [
    0, 1, 2, 3, 4, 5, 6, 8, 10, 14, 18, 26, 34, 50, 66, 98, 130, 194, 322, 578, 1090, 2114, 6210,
    22594,
];

/// Insert length codes 0..=23 (§5): extra bits after each code.
pub(crate) const INSERT_EXTRA: [u8; 24] = [
    0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 7, 8, 9, 10, 12, 14, 24,
];

/// Copy length codes 0..=23 (§5): the first length of each code.
pub(crate) const COPY_BASE: [u32; 24] = [
    2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 14, 18, 22, 30, 38, 54, 70, 102, 134, 198, 326, 582, 1094, 2118,
];

/// Copy length codes 0..=23 (§5): extra bits after each code.
pub(crate) const COPY_EXTRA: [u8; 24] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 7, 8, 9, 10, 24,
];

/// The 11 cells of the insert-and-copy alphabet (§5), by `code >> 6`: the first insert length
/// code and the first copy length code of the cell. Cells 0 and 1 (codes 0..=127) also mean
/// "the distance is the last distance".
pub(crate) const COMMAND_CELLS: [(u8, u8); 11] = [
    (0, 0),
    (0, 8),
    (0, 0),
    (0, 8),
    (8, 0),
    (8, 8),
    (0, 16),
    (16, 0),
    (8, 16),
    (16, 8),
    (16, 16),
];

/// Block count codes 0..=25 (§6): the first count of each code.
pub(crate) const BLOCK_COUNT_BASE: [u32; 26] = [
    1, 5, 9, 13, 17, 25, 33, 41, 49, 65, 81, 97, 113, 145, 177, 209, 241, 305, 369, 497, 753, 1265,
    2289, 4337, 8433, 16625,
];

/// Block count codes 0..=25 (§6): extra bits after each code.
pub(crate) const BLOCK_COUNT_EXTRA: [u8; 26] = [
    2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 6, 6, 7, 8, 9, 10, 11, 12, 13, 24,
];

/// The insert length code and copy length code of insert-and-copy code `code` (0..704).
pub(crate) fn command_codes(code: u32) -> (usize, usize) {
    let (insert, copy) = COMMAND_CELLS[(code >> 6) as usize];
    (
        insert as usize + ((code >> 3) & 7) as usize,
        copy as usize + (code & 7) as usize,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        BLOCK_COUNT_BASE, BLOCK_COUNT_EXTRA, COPY_BASE, COPY_EXTRA, INSERT_BASE, INSERT_EXTRA,
        command_codes,
    };

    /// Each code's range ends where the next begins, and the last one ends where the RFC
    /// says.
    fn contiguous(base: &[u32], extra: &[u8], last: u32, name: &str) {
        for i in 1..base.len() {
            assert_eq!(
                base[i - 1] + (1 << extra[i - 1]),
                base[i],
                "{name}: code {} ends where code {i} begins",
                i - 1
            );
        }
        let n = base.len() - 1;
        assert_eq!(
            base[n] + (1 << extra[n]) - 1,
            last,
            "{name}: the last length"
        );
    }

    #[test]
    fn length_ranges_match_the_rfc() {
        contiguous(&INSERT_BASE, &INSERT_EXTRA, 16_799_809, "insert");
        contiguous(&COPY_BASE, &COPY_EXTRA, 16_779_333, "copy");
        contiguous(
            &BLOCK_COUNT_BASE,
            &BLOCK_COUNT_EXTRA,
            16_793_840,
            "block count",
        );
    }

    #[test]
    fn command_cells_match_the_rfc_table() {
        // (code, insert length code, copy length code), from the §5 grid.
        for (code, insert, copy) in [
            (0, 0, 0),
            (7, 0, 7),
            (63, 7, 7),
            (64, 0, 8),
            (127, 7, 15),
            (128, 0, 0),
            (191, 7, 7),
            (192, 0, 8),
            (256, 8, 0),
            (320, 8, 8),
            (384, 0, 16),
            (447, 7, 23),
            (448, 16, 0),
            (512, 8, 16),
            (576, 16, 8),
            (640, 16, 16),
            (703, 23, 23),
        ] {
            assert_eq!(command_codes(code), (insert, copy), "code {code}");
        }
    }
}
