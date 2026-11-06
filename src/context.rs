//! Context IDs for literals and distances (RFC 7932 §7.1, §7.2).

use crate::context_lut::{LUT0, LUT1, LUT2};

/// The literal context ID (0..64) in context `mode` (0 LSB6, 1 MSB6, 2 UTF8, 3 Signed), from
/// the last byte `p1` and the one before it, `p2`.
#[inline(always)]
pub(crate) fn literal(mode: u8, p1: u8, p2: u8) -> usize {
    match mode & 3 {
        0 => (p1 & 0x3f) as usize,
        1 => (p1 >> 2) as usize,
        2 => (LUT0[p1 as usize] | LUT1[p2 as usize]) as usize,
        _ => ((LUT2[p1 as usize] << 3) | LUT2[p2 as usize]) as usize,
    }
}

/// The distance context ID (0..4) for a copy of `copy_len` bytes (§7.2).
#[inline(always)]
pub(crate) fn distance(copy_len: u32) -> usize {
    match copy_len {
        0..=2 => 0,
        3 => 1,
        4 => 2,
        _ => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::{distance, literal};
    use crate::context_lut::{LUT0, LUT1, LUT2};

    /// CRC-32 (the zlib polynomial, as RFC 7932 Appendix C uses).
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn the_tables_match_the_rfc_checksums() {
        assert_eq!(crc32(&LUT0), 0x8e91_efb7, "Lut0");
        assert_eq!(crc32(&LUT1), 0xd01a_32f4, "Lut1");
        assert_eq!(crc32(&LUT2), 0x0dd7_a0d6, "Lut2");
    }

    #[test]
    fn context_modes() {
        // (mode, p1, p2, context ID)
        for (mode, p1, p2, id) in [
            (0, 0xff, 0x00, 63),
            (0, 0x41, 0xff, 1),
            (1, 0xff, 0x00, 63),
            (1, 0x41, 0xff, 16),
            // UTF8: Lut0[' '] = 8, Lut1['a'] = 3; Lut0['e'] = 56 (a vowel), Lut1[' '] = 0.
            (2, b' ', b'a', 11),
            (2, b'e', b' ', 56),
            // Signed: Lut2[0x00] = 0, Lut2[0xff] = 7.
            (3, 0xff, 0x00, 56),
            (3, 0x00, 0xff, 7),
            (3, 0xff, 0xff, 63),
        ] {
            assert_eq!(
                literal(mode, p1, p2),
                id,
                "mode {mode}, p1 {p1:#x}, p2 {p2:#x}"
            );
        }
    }

    #[test]
    fn literal_contexts_stay_below_64() {
        for mode in 0..4 {
            for p1 in 0..=255 {
                for p2 in 0..=255 {
                    assert!(literal(mode, p1, p2) < 64, "mode {mode}, {p1}, {p2}");
                }
            }
        }
    }

    #[test]
    fn distance_contexts_by_copy_length() {
        for (len, id) in [(2, 0), (3, 1), (4, 2), (5, 3), (100, 3), (16_779_333, 3)] {
            assert_eq!(distance(len), id, "copy length {len}");
        }
    }
}
