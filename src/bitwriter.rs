//! Writing the bit stream: least significant bit first (RFC 7932 §2).

/// Writes bits into a byte buffer, least significant bit first, through a 64-bit
/// accumulator.
pub(crate) struct BitWriter<'a> {
    out: &'a mut Vec<u8>,
    acc: u64,
    count: u32,
}

/// Bits written but not yet a whole byte, kept between calls: see [`BitWriter::suspend`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Pending {
    bits: u8,
    count: u8,
}

/// A point to roll back to: see [`BitWriter::mark`].
#[derive(Clone, Copy)]
pub(crate) struct Mark {
    len: usize,
    acc: u64,
    count: u32,
}

impl<'a> BitWriter<'a> {
    /// A writer appending to `out`, which must end on a byte boundary of the stream.
    pub(crate) fn new(out: &'a mut Vec<u8>) -> Self {
        Self {
            out,
            acc: 0,
            count: 0,
        }
    }

    /// A writer appending to `out` after the bits `pending` that an earlier writer left.
    pub(crate) fn resume(out: &'a mut Vec<u8>, pending: Pending) -> Self {
        Self {
            out,
            acc: u64::from(pending.bits),
            count: u32::from(pending.count),
        }
    }

    /// Puts the whole bytes written into the buffer and returns the bits left over, fewer than
    /// eight, for [`resume`](Self::resume).
    pub(crate) fn suspend(mut self) -> Pending {
        while self.count >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.count -= 8;
        }
        Pending {
            bits: self.acc as u8,
            count: self.count as u8,
        }
    }

    /// Appends the low `n` bits of `v` (`n <= 32`; the other bits of `v` must be zero).
    #[inline(always)]
    pub(crate) fn put(&mut self, n: u32, v: u32) {
        debug_assert!(
            n <= 32 && (n == 32 || v >> n == 0),
            "{v} doesn't fit in {n} bits"
        );
        self.acc |= u64::from(v) << self.count;
        self.count += n;
        if self.count >= 32 {
            self.out.extend_from_slice(&(self.acc as u32).to_le_bytes());
            self.acc >>= 32;
            self.count -= 32;
        }
    }

    /// Bits written since `mark`.
    pub(crate) fn bits_since(&self, mark: Mark) -> usize {
        (self.out.len() - mark.len) * 8 + self.count as usize - mark.count as usize
    }

    /// Bits written into the current byte: 0 on a byte boundary.
    pub(crate) fn bit_offset(&self) -> usize {
        self.count as usize % 8
    }

    /// Pads with zero bits to the next byte boundary.
    pub(crate) fn align(&mut self) {
        let pad = (8 - self.count % 8) % 8;
        self.put(pad, 0);
        while self.count >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.count -= 8;
        }
    }

    /// Appends whole bytes; the writer must be on a byte boundary.
    pub(crate) fn bytes(&mut self, data: &[u8]) {
        self.align();
        debug_assert_eq!(self.count, 0);
        self.out.extend_from_slice(data);
    }

    /// The current position, to [`rollback`](Self::rollback) to.
    pub(crate) fn mark(&self) -> Mark {
        Mark {
            len: self.out.len(),
            acc: self.acc,
            count: self.count,
        }
    }

    /// Forgets everything written since `mark`.
    pub(crate) fn rollback(&mut self, mark: Mark) {
        self.out.truncate(mark.len);
        self.acc = mark.acc;
        self.count = mark.count;
    }

    /// Pads to a byte boundary and flushes the last bits into the buffer.
    pub(crate) fn finish(mut self) {
        self.align();
    }
}

/// `v` with its low `n` bits reversed: a prefix code (most significant bit first, RFC 7932
/// §3.2) in the order [`BitWriter::put`] writes bits. 0 for `n = 0`, without a branch: the
/// shift by 32 is checked, not taken.
#[inline]
pub(crate) fn reverse(v: u32, n: u32) -> u32 {
    v.reverse_bits().checked_shr(32 - n).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{BitWriter, reverse};
    use crate::bits::BitReader;

    #[test]
    fn a_suspended_writer_resumes_where_it_stopped() {
        // The same bits written in one go and in pieces, stopping after each.
        let values: Vec<(u32, u32)> = (0..300u32)
            .map(|i| (i % 13 + 1, i % (1 << (i % 13 + 1))))
            .collect();
        let mut whole = Vec::new();
        let mut w = BitWriter::new(&mut whole);
        for &(n, v) in &values {
            w.put(n, v);
        }
        w.finish();
        let mut pieces = Vec::new();
        let mut pending = super::Pending::default();
        for chunk in values.chunks(7) {
            let mut w = BitWriter::resume(&mut pieces, pending);
            for &(n, v) in chunk {
                w.put(n, v);
            }
            pending = w.suspend();
            assert!(pending.count < 8);
        }
        BitWriter::resume(&mut pieces, pending).finish();
        assert_eq!(pieces, whole);
    }

    #[test]
    fn what_is_written_reads_back() {
        let values: Vec<(u32, u32)> = (0..500)
            .map(|i: u32| {
                let n = i % 33;
                (
                    n,
                    if n == 32 {
                        i.wrapping_mul(2_654_435_761)
                    } else {
                        (i * 7919) & ((1u64 << n) - 1) as u32
                    },
                )
            })
            .collect();
        let mut buf = Vec::new();
        let mut w = BitWriter::new(&mut buf);
        for &(n, v) in &values {
            w.put(n, v);
        }
        w.finish();
        let mut r = BitReader::new(&buf);
        for &(n, v) in &values {
            if n <= 24 {
                assert_eq!(r.bits(n).unwrap(), v, "{n} bits");
            } else {
                let low = r.bits(16).unwrap();
                let high = r.bits(n - 16).unwrap();
                assert_eq!(low | (high << 16), v, "{n} bits");
            }
        }
    }

    #[test]
    fn aligning_and_whole_bytes() {
        let mut buf = vec![0xee];
        let mut w = BitWriter::new(&mut buf);
        w.put(3, 0b101);
        w.bytes(b"ab");
        w.put(1, 1);
        w.finish();
        assert_eq!(buf, [0xee, 0b101, b'a', b'b', 1]);
    }

    #[test]
    fn rolling_back_forgets_what_was_written() {
        let mut buf = Vec::new();
        let mut w = BitWriter::new(&mut buf);
        let start = w.mark();
        w.put(5, 0b10110);
        let mark = w.mark();
        assert_eq!(w.bit_offset(), 5);
        for _ in 0..100 {
            w.put(13, 0x1abc);
        }
        assert_eq!(w.bits_since(mark), 1300);
        w.rollback(mark);
        assert_eq!(w.bits_since(start), 5);
        w.put(3, 0b111);
        w.finish();
        assert_eq!(buf, [0b1111_0110]);
    }

    #[test]
    fn reversing_codes() {
        assert_eq!(reverse(0b1, 1), 0b1);
        assert_eq!(reverse(0b110, 3), 0b011);
        assert_eq!(reverse(0b100_0000_0000_0001, 15), 0b100_0000_0000_0001);
        assert_eq!(reverse(0b10, 4), 0b0100);
        assert_eq!(reverse(0, 0), 0);
    }
}
