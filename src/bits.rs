//! Reading the bit stream: least significant bit first (RFC 7932 §2).

use crate::Error;

/// Reads bits from a byte slice, least significant bit of each byte first.
///
/// Up to 64 bits are kept in `buf`; `count` of them are valid, the rest are zero. Reads of up
/// to 24 bits at a time refill first, so they never run out mid-value unless the input does.
pub(crate) struct BitReader<'a> {
    src: &'a [u8],
    /// The next byte of `src` not yet in `buf`.
    pos: usize,
    buf: u64,
    count: u32,
}

impl<'a> BitReader<'a> {
    /// A reader at the start of `src`.
    pub(crate) fn new(src: &'a [u8]) -> Self {
        Self {
            src,
            pos: 0,
            buf: 0,
            count: 0,
        }
    }

    /// Loads whole bytes until at least 56 bits are buffered or the input ends.
    #[inline(always)]
    fn refill(&mut self) {
        if self.count > 56 {
            return;
        }
        if let Some(chunk) = self.src.get(self.pos..self.pos + 8) {
            // Eight bytes at once; keep only the whole bytes that fit.
            let mut word = [0u8; 8];
            word.copy_from_slice(chunk);
            let word = u64::from_le_bytes(word);
            let take = (63 - self.count) / 8;
            self.buf |= (word & ((1u64 << (take * 8)) - 1)) << self.count;
            self.pos += take as usize;
            self.count += take * 8;
        } else {
            while self.count <= 56 {
                let Some(&b) = self.src.get(self.pos) else {
                    break;
                };
                self.buf |= u64::from(b) << self.count;
                self.pos += 1;
                self.count += 8;
            }
        }
    }

    /// The next `n` bits (`n <= 24`) without consuming them; bits past the end read as zero.
    #[inline(always)]
    pub(crate) fn peek(&mut self, n: u32) -> u32 {
        debug_assert!(n <= 24);
        if self.count < n {
            self.refill();
        }
        (self.buf & ((1u64 << n) - 1)) as u32
    }

    /// Consumes `n` bits that [`peek`](Self::peek) looked at.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if fewer than `n` bits are left.
    #[inline(always)]
    pub(crate) fn consume(&mut self, n: u32) -> Result<(), Error> {
        if n > self.count {
            return Err(Error::Truncated);
        }
        self.buf >>= n;
        self.count -= n;
        Ok(())
    }

    /// Reads `n` bits (`n <= 24`), the first one read in the lowest bit.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if fewer than `n` bits are left.
    #[inline(always)]
    pub(crate) fn bits(&mut self, n: u32) -> Result<u32, Error> {
        let v = self.peek(n);
        self.consume(n)?;
        Ok(v)
    }

    /// Reads one bit.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] at the end of the input.
    #[inline(always)]
    pub(crate) fn bit(&mut self) -> Result<bool, Error> {
        Ok(self.bits(1)? == 1)
    }

    /// Skips to the next byte boundary and returns the skipped bits, which callers check
    /// are zero where the format says so.
    pub(crate) fn align_to_byte(&mut self) -> u32 {
        let n = self.count % 8;
        let v = (self.buf & ((1u64 << n) - 1)) as u32;
        self.buf >>= n;
        self.count -= n;
        v
    }

    /// Whether the reader is on a byte boundary.
    pub(crate) fn is_aligned(&self) -> bool {
        self.count.is_multiple_of(8)
    }

    /// The offset in the input of the next unread byte. Only meaningful when aligned.
    pub(crate) fn byte_position(&self) -> usize {
        self.pos - (self.count / 8) as usize
    }

    /// Takes the next `n` whole bytes. The reader must be aligned (see
    /// [`align_to_byte`](Self::align_to_byte)).
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if fewer than `n` bytes are left.
    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        debug_assert!(self.is_aligned());
        let start = self.byte_position();
        let out = self
            .src
            .get(start..start.checked_add(n).ok_or(Error::Truncated)?)
            .ok_or(Error::Truncated)?;
        self.pos = start + n;
        self.buf = 0;
        self.count = 0;
        Ok(out)
    }

    /// Whether every bit of the input has been read.
    #[cfg(test)]
    pub(crate) fn at_end(&self) -> bool {
        self.count == 0 && self.pos == self.src.len()
    }

    /// Bits not yet read.
    #[cfg(test)]
    pub(crate) fn remaining_bits(&self) -> usize {
        self.count as usize + (self.src.len() - self.pos) * 8
    }
}

/// A bit writer for tests: builds streams by hand, least significant bit first.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestBits {
    pub(crate) bytes: Vec<u8>,
    acc: u64,
    count: u32,
}

#[cfg(test)]
impl TestBits {
    /// Appends the low `n` bits of `v`.
    pub(crate) fn put(&mut self, n: u32, v: u32) -> &mut Self {
        assert!(
            n <= 32 && (n == 32 || v >> n == 0),
            "{v} doesn't fit in {n} bits"
        );
        self.acc |= u64::from(v) << self.count;
        self.count += n;
        while self.count >= 8 {
            self.bytes.push(self.acc as u8);
            self.acc >>= 8;
            self.count -= 8;
        }
        self
    }

    /// Appends a prefix code `code` of `len` bits, written most significant bit first as RFC
    /// 7932 §3.2 gives codes.
    pub(crate) fn code(&mut self, code: u32, len: u32) -> &mut Self {
        for i in (0..len).rev() {
            self.put(1, (code >> i) & 1);
        }
        self
    }

    /// Pads with zero bits to a byte boundary.
    pub(crate) fn align(&mut self) -> &mut Self {
        if self.count > 0 {
            self.put(8 - self.count, 0);
        }
        self
    }

    /// The bytes written, padded with zero bits.
    pub(crate) fn finish(&mut self) -> Vec<u8> {
        self.align();
        std::mem::take(&mut self.bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::{BitReader, TestBits};
    use crate::Error;

    #[test]
    fn bits_come_least_significant_first() {
        let mut r = BitReader::new(&[0b1010_0110, 0xff]);
        assert_eq!(r.bits(1).unwrap(), 0);
        assert_eq!(r.bits(2).unwrap(), 0b11);
        assert_eq!(r.bits(3).unwrap(), 0b100);
        // Two bits left in the first byte, then six from the second.
        assert_eq!(r.bits(8).unwrap(), 0b1111_1110);
        assert_eq!(r.bits(2).unwrap(), 0b11);
        assert!(r.at_end());
    }

    #[test]
    fn reads_cross_byte_and_word_boundaries() {
        let mut w = TestBits::default();
        let values: Vec<(u32, u32)> = (0..200).map(|i| (1 + i % 24, i * 7919)).collect();
        for &(n, v) in &values {
            w.put(n, v & ((1 << n) - 1));
        }
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes);
        for &(n, v) in &values {
            assert_eq!(r.bits(n).unwrap(), v & ((1 << n) - 1), "{n} bits of {v}");
        }
    }

    #[test]
    fn zero_bit_reads_read_nothing() {
        let mut r = BitReader::new(&[]);
        assert_eq!(r.bits(0).unwrap(), 0);
        assert!(r.at_end());
    }

    #[test]
    fn twenty_four_bit_reads() {
        let mut r = BitReader::new(&[0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc]);
        assert_eq!(r.bits(24).unwrap(), 0x56_3412);
        assert_eq!(r.bits(24).unwrap(), 0xbc_9a78);
        assert_eq!(r.bits(1), Err(Error::Truncated));
    }

    #[test]
    fn running_out_is_truncation_at_every_length() {
        let src = [1, 2, 3, 4];
        for len in 0..src.len() {
            let mut r = BitReader::new(&src[..len]);
            for _ in 0..len {
                r.bits(8).unwrap();
            }
            assert_eq!(r.bits(1), Err(Error::Truncated), "{len} bytes");
            // A failed read consumes nothing.
            assert_eq!(r.remaining_bits(), 0, "{len} bytes");
        }
        let mut r = BitReader::new(&src);
        r.bits(20).unwrap();
        assert_eq!(r.bits(13), Err(Error::Truncated));
        assert_eq!(r.bits(12).unwrap(), 0x040);
    }

    #[test]
    fn aligning_returns_the_skipped_bits() {
        let mut r = BitReader::new(&[0b1011_0101, 0xaa]);
        assert_eq!(r.bits(3).unwrap(), 0b101);
        assert!(!r.is_aligned());
        assert_eq!(r.align_to_byte(), 0b10110);
        assert!(r.is_aligned());
        assert_eq!(r.align_to_byte(), 0, "already aligned");
        assert_eq!(r.bits(8).unwrap(), 0xaa);
    }

    #[test]
    fn whole_bytes_after_aligning() {
        let src: Vec<u8> = (0..20).collect();
        let mut r = BitReader::new(&src);
        r.bits(4).unwrap();
        r.align_to_byte();
        assert_eq!(r.byte_position(), 1);
        assert_eq!(r.bytes(3).unwrap(), &[1, 2, 3]);
        assert_eq!(r.bits(8).unwrap(), 4);
        assert_eq!(r.bytes(15).unwrap(), &src[5..]);
        assert!(r.at_end());
        assert_eq!(r.bytes(1), Err(Error::Truncated));
        assert_eq!(r.bytes(usize::MAX), Err(Error::Truncated));
    }

    #[test]
    fn peeking_past_the_end_reads_zeros_but_consuming_fails() {
        let mut r = BitReader::new(&[0xff]);
        assert_eq!(r.peek(15), 0xff);
        assert_eq!(r.consume(9), Err(Error::Truncated));
        r.consume(8).unwrap();
        assert!(r.at_end());
    }
}
