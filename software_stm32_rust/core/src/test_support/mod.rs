//! Shared test helpers. The C++ suites of software_stm32/test/native have no shared support
//! files; the random tests use `std::mt19937`, which [`Rng`] implements.

use std::string::String;
use std::vec::Vec;

/// MT19937 (32 bit), the generator of `std::mt19937`: the C++ seeds give the same raw
/// sequence. The ranges are uniform but not those of libstdc++'s distributions, so a fuzz test
/// draws other values than its C++ original with the same seed and iteration count.
pub struct Rng {
    mt: [u32; 624],
    index: usize,
}

impl Rng {
    pub fn new(seed: u32) -> Self {
        let mut mt = [0u32; 624];
        mt[0] = seed;
        for i in 1..624 {
            let prev = mt[i - 1];
            mt[i] = 1_812_433_253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
        Rng { mt, index: 624 }
    }

    pub fn next_u32(&mut self) -> u32 {
        if self.index >= 624 {
            self.twist();
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    fn twist(&mut self) {
        for i in 0..624 {
            let y = (self.mt[i] & 0x8000_0000) | (self.mt[(i + 1) % 624] & 0x7fff_ffff);
            let mut v = self.mt[(i + 397) % 624] ^ (y >> 1);
            if y & 1 != 0 {
                v ^= 0x9908_b0df;
            }
            self.mt[i] = v;
        }
        self.index = 0;
    }

    /// Uniform in `lo..=hi` (`std::uniform_int_distribution<uint32_t>(lo, hi)`).
    pub fn range_u32(&mut self, lo: u32, hi: u32) -> u32 {
        assert!(lo <= hi);
        let span = u64::from(hi - lo) + 1;
        if span == 1 << 32 {
            return self.next_u32();
        }
        // rejection keeps the result unbiased
        let zone = (1u64 << 32) - (1u64 << 32) % span;
        loop {
            let v = u64::from(self.next_u32());
            if v < zone {
                return lo + (v % span) as u32;
            }
        }
    }

    /// Uniform in `lo..=hi` (`std::uniform_int_distribution<int32_t>(lo, hi)`).
    pub fn range_i32(&mut self, lo: i32, hi: i32) -> i32 {
        assert!(lo <= hi);
        let offset = self.range_u32(0, hi.wrapping_sub(lo) as u32);
        lo.wrapping_add(offset as i32)
    }

    /// Uniform in `lo..=hi`.
    pub fn range_usize(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.range_u32(0, u32::try_from(hi - lo).expect("range fits u32")) as usize
    }

    /// One byte 0..=255.
    pub fn byte(&mut self) -> u8 {
        self.range_u32(0, 255) as u8
    }
}

/// The bytes as text for readable assertion messages (lossy for non-UTF-8 bytes).
pub fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// 0..=max_len random bytes (test_fuzz.cpp randomBytes).
pub fn random_bytes(rng: &mut Rng, max_len: usize) -> Vec<u8> {
    let len = rng.range_usize(0, max_len);
    (0..len).map(|_| rng.byte()).collect()
}

/// Mostly protocol-like text with occasional junk (test_fuzz.cpp randomProtocolText).
pub fn random_protocol_text(rng: &mut Rng, max_len: usize) -> Vec<u8> {
    const ALPHABET: &[u8] = b"stgvlpxdn0123456789 -\t\r\n\x01\xff";
    let len = rng.range_usize(0, max_len);
    (0..len)
        .map(|_| ALPHABET[rng.range_usize(0, ALPHABET.len() - 1)])
        .collect()
}

#[test]
fn rng_is_mt19937() {
    // the 10000th output of a default-constructed std::mt19937 (seed 5489), C++11 26.5.5
    let mut rng = Rng::new(5489);
    let mut v = 0;
    for _ in 0..10_000 {
        v = rng.next_u32();
    }
    assert_eq!(v, 4_123_659_995);
}

#[test]
fn rng_ranges_stay_inside() {
    let mut rng = Rng::new(1);
    for _ in 0..10_000 {
        let v = rng.range_u32(3, 9);
        assert!((3..=9).contains(&v));
        let s = rng.range_i32(i32::MIN, i32::MAX);
        let _ = s;
        let n = rng.range_i32(-5, 5);
        assert!((-5..=5).contains(&n));
        let u = rng.range_usize(0, 17);
        assert!(u <= 17);
    }
}
