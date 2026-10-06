//! Current profile of one motor move: at most PROFILE_SAMPLES points
//! (pulse count, |current| in 0.1 mA) at equal count spacing. Hardware-free,
//! fixed size, no allocation; safe to copy by assignment.

pub const PROFILE_SAMPLES: u8 = 32;

const SAMPLES: usize = PROFILE_SAMPLES as usize;
const MAX_SPACING: u16 = 0x8000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProfileSample {
    pub count: u16,
    /// 0.1 mA, absolute value
    pub current: u16,
}

fn saturate16(v: u32) -> u16 {
    u16::try_from(v).unwrap_or(u16::MAX)
}

fn magnitude16(v: i32) -> u16 {
    saturate16(v.unsigned_abs())
}

/// Samples are taken at the first poll at or after each multiple of the
/// spacing, which starts at one pulse. When the buffer is full the spacing
/// doubles and only the first sample of each new grid cell is kept, so the
/// samples stay on an equally spaced grid whatever the length of the move.
#[derive(Clone, Copy, Debug)]
pub struct ProfileRecorder {
    samples: [ProfileSample; SAMPLES],
    size: u8,
    /// at least 1
    spacing: u16,
    next: u32,
}

impl Default for ProfileRecorder {
    /// empty, spacing 1 (the C++ member initializers)
    fn default() -> Self {
        ProfileRecorder {
            samples: [ProfileSample::default(); SAMPLES],
            size: 0,
            spacing: 1,
            next: 0,
        }
    }
}

impl ProfileRecorder {
    pub fn reset(&mut self) {
        self.size = 0;
        self.spacing = 1;
        self.next = 0;
    }

    fn append(&mut self, count: u16, current: u16) {
        if let Some(s) = self.samples.get_mut(usize::from(self.size)) {
            *s = ProfileSample { count, current };
            self.size += 1;
        }
        self.next = self.next_grid_point(count);
    }

    fn next_grid_point(&self, count: u16) -> u32 {
        let spacing = u32::from(self.spacing);
        (u32::from(count) / spacing + 1) * spacing
    }

    fn compact(&mut self) {
        // Every sample is the first poll at or after a grid point. With a doubled
        // spacing the first sample in each new grid cell is exactly what sampling
        // with that spacing from the start would have recorded.
        while usize::from(self.size) == SAMPLES && self.spacing < MAX_SPACING {
            self.spacing *= 2;
            let mut kept = 0usize;
            for i in 0..SAMPLES {
                let s = self.samples[i];
                if kept == 0
                    || s.count / self.spacing != self.samples[kept - 1].count / self.spacing
                {
                    self.samples[kept] = s;
                    kept += 1;
                }
            }
            self.size = kept as u8;
        }
        // with 16-bit counts the largest spacing leaves at most two cells, so size < PROFILE_SAMPLES here
        self.next = match usize::from(self.size).checked_sub(1) {
            Some(last) => self.next_grid_point(self.samples[last].count),
            None => 0,
        };
    }

    /// Offers the state at one poll; recorded if the count reached the next
    /// sampling point. Counts must not decrease within a move (a smaller count
    /// is ignored).
    pub fn add(&mut self, count: u32, current: i32) {
        let c = saturate16(count);
        if u32::from(c) < self.next {
            return;
        }
        if self.size == PROFILE_SAMPLES {
            self.compact();
            if u32::from(c) < self.next {
                return;
            }
        }
        self.append(c, magnitude16(current));
    }

    /// Records the stop point of the move (unless it is already the last
    /// sample), so the profile always ends with the current at the stop.
    pub fn finish(&mut self, count: u32, current: i32) {
        let c = saturate16(count);
        if let Some(last) = usize::from(self.size).checked_sub(1) {
            let last = &mut self.samples[last];
            if c < last.count {
                return;
            }
            if c == last.count {
                last.current = magnitude16(current);
                return;
            }
        }
        if self.size == PROFILE_SAMPLES {
            self.compact();
        }
        self.append(c, magnitude16(current));
    }

    pub fn size(&self) -> u8 {
        self.size
    }

    pub fn spacing(&self) -> u16 {
        self.spacing
    }

    /// i < size(); out-of-range indexes return {0, 0}.
    pub fn at(&self, i: u8) -> ProfileSample {
        if i >= self.size {
            return ProfileSample::default();
        }
        self.samples[usize::from(i)]
    }
}
