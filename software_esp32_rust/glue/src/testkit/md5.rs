//! MD5 (RFC 1321) for the [`Md5`] port, so digests in tests are the real ones the dashboard
//! computes.

use crate::port::Md5;

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

fn k(i: usize) -> u32 {
    ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32
}

/// Streaming MD5.
#[derive(Clone)]
pub(crate) struct FakeMd5 {
    state: [u32; 4],
    buf: Vec<u8>,
    len: u64,
    /// Bytes added since the last reset (for assertions).
    pub(crate) added: u64,
}

impl Default for FakeMd5 {
    fn default() -> Self {
        let mut m = FakeMd5 {
            state: [0; 4],
            buf: Vec::new(),
            len: 0,
            added: 0,
        };
        m.reset();
        m
    }
}

impl FakeMd5 {
    fn block(&mut self, chunk: &[u8]) {
        let m: Vec<u32> = chunk
            .chunks(4)
            .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
            .collect();
        let [mut a, mut b, mut c, mut d] = self.state;
        for (i, &s) in S.iter().enumerate() {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let t = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(k(i))
                    .wrapping_add(m[g])
                    .rotate_left(s),
            );
            a = t;
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }

    /// Hex digest of `data`.
    pub(crate) fn hex(data: &[u8]) -> String {
        let mut m = FakeMd5::default();
        m.update(data);
        m.digest().iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl Md5 for FakeMd5 {
    fn reset(&mut self) {
        self.state = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476];
        self.buf.clear();
        self.len = 0;
        self.added = 0;
    }
    fn update(&mut self, data: &[u8]) {
        self.len += data.len() as u64;
        self.added += data.len() as u64;
        self.buf.extend_from_slice(data);
        while self.buf.len() >= 64 {
            let chunk: Vec<u8> = self.buf.drain(..64).collect();
            self.block(&chunk);
        }
    }
    fn digest(&mut self) -> [u8; 16] {
        let bits = self.len.wrapping_mul(8);
        let mut tail = std::mem::take(&mut self.buf);
        tail.push(0x80);
        while tail.len() % 64 != 56 {
            tail.push(0);
        }
        tail.extend_from_slice(&bits.to_le_bytes());
        for chunk in tail.chunks(64) {
            self.block(chunk);
        }
        let mut out = [0u8; 16];
        for (o, s) in out.chunks_mut(4).zip(self.state) {
            o.copy_from_slice(&s.to_le_bytes());
        }
        self.reset();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_1321_test_suite() {
        assert_eq!(FakeMd5::hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(FakeMd5::hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            FakeMd5::hex(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            ),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
        let mut m = FakeMd5::default();
        m.update(b"message ");
        m.update(b"digest");
        let d: String = m.digest().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(d, "f96b697d7cb7938d525a2f31aaf161d0");
    }
}
