//! Deterministic generators of the fuzz and random tests, bit-exact with the generators of the
//! C++ tests, so a ported test walks the same inputs as its original:
//!
//! | C++ test                                  | Rust test                                    |
//! |-------------------------------------------|----------------------------------------------|
//! | `std::mt19937 rng(S);` `rng()`            | `Rng::new(S)`, `rng.next_u32()`              |
//! | `rng() % n`                               | `rng.below(n)`                               |
//! | `srand(S);` `rand()`                      | `CRand::new(S)`, `rng.rand()` (glibc)        |
//! | `seed = seed * A + C;` (inline LCG)       | `Lcg::new(S, A, C)`, `lcg.next_state()`      |

/// `std::mt19937` (32-bit Mersenne Twister, the C++11 parameters and seeding).
pub struct Rng {
    mt: [u32; 624],
    index: usize,
}

impl Rng {
    /// `std::mt19937 rng(seed)`.
    pub fn new(seed: u32) -> Self {
        let mut mt = [0u32; 624];
        mt[0] = seed;
        for i in 1..624 {
            let prev = mt[i - 1];
            mt[i] = 1_812_433_253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
        Self { mt, index: 624 }
    }

    /// `rng()`.
    pub fn next_u32(&mut self) -> u32 {
        if self.index >= 624 {
            self.twist();
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9D2C_5680;
        y ^= (y << 15) & 0xEFC6_0000;
        y ^ (y >> 18)
    }

    /// `rng() % n`.
    pub fn below(&mut self, n: u32) -> u32 {
        self.next_u32() % n
    }

    fn twist(&mut self) {
        for i in 0..624 {
            let y = (self.mt[i] & 0x8000_0000) | (self.mt[(i + 1) % 624] & 0x7FFF_FFFF);
            let mut v = self.mt[(i + 397) % 624] ^ (y >> 1);
            if y & 1 != 0 {
                v ^= 0x9908_B0DF;
            }
            self.mt[i] = v;
        }
        self.index = 0;
    }
}

/// glibc `srand(seed)` / `rand()`: the TYPE_3 additive feedback generator (31 words, separation
/// 3, 310 discarded outputs), results 0..=RAND_MAX (2^31 - 1).
pub struct CRand {
    state: [i32; 31],
    front: usize,
    rear: usize,
}

impl CRand {
    /// `srand(seed)`; 0 seeds like 1, as in glibc.
    pub fn new(seed: u32) -> Self {
        let seed = if seed == 0 { 1 } else { seed };
        let mut state = [0i32; 31];
        let mut word = seed as i32; // glibc: int32_t word = seed
        state[0] = word;
        for slot in state.iter_mut().skip(1) {
            // state[i] = (16807 * state[i - 1]) % 2147483647 without overflowing 31 bits, in
            // `long` like glibc and stored to int32
            let hi = i64::from(word) / 127_773;
            let lo = i64::from(word) % 127_773;
            word = (16_807 * lo - 2_836 * hi) as i32;
            if word < 0 {
                word += 2_147_483_647;
            }
            *slot = word;
        }
        let mut r = Self {
            state,
            front: 3,
            rear: 0,
        };
        for _ in 0..310 {
            r.rand();
        }
        r
    }

    /// `rand()`.
    pub fn rand(&mut self) -> i32 {
        let val = (self.state[self.front] as u32).wrapping_add(self.state[self.rear] as u32);
        self.state[self.front] = val as i32;
        self.front += 1;
        if self.front >= 31 {
            self.front = 0;
            self.rear += 1;
        } else {
            self.rear += 1;
            if self.rear >= 31 {
                self.rear = 0;
            }
        }
        (val >> 1) as i32
    }
}

/// The inline linear congruential generators of the C++ tests: `seed = seed * mul + inc`
/// (mod 2^32); the test applies its own shift to the state, as the C++ does.
pub struct Lcg {
    state: u32,
    mul: u32,
    inc: u32,
}

impl Lcg {
    pub fn new(seed: u32, mul: u32, inc: u32) -> Self {
        Self {
            state: seed,
            mul,
            inc,
        }
    }

    /// `seed = seed * 1664525u + 1013904223u` (Numerical Recipes).
    pub fn numerical_recipes(seed: u32) -> Self {
        Self::new(seed, 1_664_525, 1_013_904_223)
    }

    /// `seed = seed * 1103515245u + 12345u` (the C standard's example `rand`).
    pub fn ansi_c(seed: u32) -> Self {
        Self::new(seed, 1_103_515_245, 12_345)
    }

    /// Advances and returns the new state (the C++ `seed` after the step).
    pub fn next_state(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(self.mul).wrapping_add(self.inc);
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference values: g++ 12 / glibc 2.36 (Debian bookworm), printed by std::mt19937 and
    // srand()/rand() for the same seeds.

    fn first<const N: usize>(mut f: impl FnMut() -> u32) -> [u32; N] {
        core::array::from_fn(|_| f())
    }

    #[test]
    fn mt19937_matches_the_cpp_library() {
        let mut a = Rng::new(1234);
        assert_eq!(
            first::<5>(|| a.next_u32()),
            [
                822_569_775,
                2_137_449_171,
                2_671_936_806,
                3_512_589_365,
                1_880_026_316
            ]
        );
        let mut b = Rng::new(777);
        assert_eq!(
            first::<5>(|| b.next_u32()),
            [
                655_685_735,
                2_776_480_559,
                1_298_611_771,
                862_112_678,
                266_444_375
            ]
        );
        let mut z = Rng::new(0);
        assert_eq!(
            first::<3>(|| z.next_u32()),
            [2_357_136_044, 2_546_248_239, 3_071_714_933]
        );
        // C++11 [rand.predef]: the 10000th output of a default-seeded mt19937 is 4123659995.
        let mut d = Rng::new(5489);
        for _ in 0..9999 {
            d.next_u32();
        }
        assert_eq!(d.next_u32(), 4_123_659_995);
        let mut m = Rng::new(1234);
        assert_eq!(m.below(1000), 822_569_775 % 1000);
        assert_eq!(m.below(7), 2_137_449_171 % 7);
    }

    #[test]
    fn crand_matches_glibc() {
        let rows: [(u32, [i32; 3]); 6] = [
            (12345, [383_100_999, 858_300_821, 357_768_173]),
            (4242, [475_847_265, 515_213_143, 1_724_485_243]),
            (777, [947_371_799, 2_013_380_011, 1_359_686_060]),
            (1, [1_804_289_383, 846_930_886, 1_681_692_777]),
            (u32::MAX, [254_925_627, 1_205_188_300, 366_127_624]),
            (0x8000_0000, [1_336_741_213, 1_210_407_648, 1_447_044_896]),
        ];
        for (seed, want) in rows {
            let mut r = CRand::new(seed);
            assert_eq!([r.rand(), r.rand(), r.rand()], want, "seed {seed}");
        }
        let mut zero = CRand::new(0); // srand(0) seeds like srand(1)
        assert_eq!(zero.rand(), 1_804_289_383);
        let mut long = CRand::new(12345);
        let mut last = 0;
        for _ in 0..5 {
            last = long.rand();
        }
        assert_eq!(last, 133_005_921);
    }

    #[test]
    fn lcg_steps_like_the_inline_cpp_generators() {
        let mut nr = Lcg::numerical_recipes(0x123_4567);
        assert_eq!(
            nr.next_state(),
            0x123_4567u32
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223)
        );
        let mut ansi = Lcg::ansi_c(12345);
        let s1 = ansi.next_state();
        assert_eq!(s1, 12345u32.wrapping_mul(1_103_515_245).wrapping_add(12345));
        assert_eq!(
            ansi.next_state(),
            s1.wrapping_mul(1_103_515_245).wrapping_add(12345)
        );
        let mut raw = Lcg::new(1, 3, 4);
        assert_eq!(raw.next_state(), 7);
        assert_eq!(raw.next_state(), 25);
    }
}
