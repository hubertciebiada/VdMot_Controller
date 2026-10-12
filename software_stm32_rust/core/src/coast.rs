//! The coast of a valve motor: the pulses it turns after its enable went off. The revolution
//! pulses come from the motor current, which stops with the enable, so a step that stops on its
//! pulse count travels further than it counted. A closing step away from the open end stop and
//! the move back to it measure it: the move back counts the step and its coast (a play of the
//! gears counts in both moves, each starts with a reversal). A partial closing move is shortened
//! by the learned coast. Hardware-free.
//!
//! Only the closing coast is learned. A closing step has the spring of the normally closed insert
//! behind it and coasts 50..80 pulses; an opening step works against the spring, which stops the
//! motor within a few pulses (about 2..14 in the field data of 2.2.4). A step away from the closed
//! end stop cannot measure it: the move back counts the pulses the motor turns against the hard
//! stop of the actuator until the end-stop threshold trips (2.2.4: 44..98 pulses or more than
//! COAST_MAX_COUNTS on 13 valves).
//!
//! The probe of a calibration sets the coast. A move pair of normal moves refines it by a quarter
//! when it agrees with it; two pairs in a row that agree with each other but not with it replace
//! it by their mean, and set it before one is learned: a single false pair changes nothing, and a
//! coast the normal moves do not confirm does not stay until the next calibration. Every sample
//! comes from a step next to the open end stop, where the spring of the insert is compressed most;
//! the coast of a step in the middle of the stroke is not measured.

use crate::move_classifier::{DIR_CLOSE, DIR_OPEN};

/// Pulses of the closing step away from the open end stop that a calibration makes (about 4 % of
/// a stroke of 4500..5400 pulses).
pub const COAST_PROBE_COUNTS: u16 = 200;
/// The shortest step of a normal move that gives a sample with the move back: a shorter step
/// can stop before the motor reaches its speed.
pub const COAST_SAMPLE_MIN_COUNTS: u32 = 100;
/// A larger difference between the move back and the step is no coast: one of them did not end
/// where the sample assumes (the actuators coast 50..80 pulses when closing).
pub const COAST_MAX_COUNTS: u16 = 100;
/// The smallest difference of a move pair's sample from the coast it refines that still agrees
/// with it (half of the coast, but at least this).
pub const COAST_AGREE_MIN: u16 = 8;

/// The coast of a step that counted `step` pulses away from an end stop, from the `back` pulses
/// of the move back to it; None when the move back counted fewer pulses than the step or more
/// than COAST_MAX_COUNTS beyond them.
pub fn coast_sample(step: u32, back: u32) -> Option<u16> {
    let coast = back.checked_sub(step)?;
    u16::try_from(coast).ok().filter(|&c| c <= COAST_MAX_COUNTS)
}

/// The pulse target of a partial move of `nominal` pulses: shortened by `coast`, but by at most
/// half of it (a step of less than twice the coast does not get exact).
pub fn shortened_target(nominal: u16, coast: u16) -> u16 {
    nominal - coast.min(nominal / 2)
}

/// A sample agrees with `reference` when it differs by at most half of it (at least
/// COAST_AGREE_MIN).
fn agrees(reference: u16, sample: u16) -> bool {
    reference.abs_diff(sample) <= (reference / 2).max(COAST_AGREE_MIN)
}

/// Two samples agree when they differ by at most half of the smaller one (in either order).
fn pair_agrees(a: u16, b: u16) -> bool {
    agrees(a.min(b), a.max(b))
}

/// The learned closing coast.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoastEstimate {
    counts: u16,
    learned: bool,
    /// the sample of a move pair that waits for a second one that agrees with it
    candidate: Option<u16>,
}

impl CoastEstimate {
    /// The learned coast in pulses, 0 before it is learned.
    pub fn counts(&self) -> u16 {
        self.counts
    }

    /// The sample of a calibration probe (at most COAST_MAX_COUNTS): it is the coast.
    pub fn set(&mut self, sample: u16) {
        self.counts = sample.min(COAST_MAX_COUNTS);
        self.learned = true;
        self.candidate = None;
    }

    /// The sample of a move pair (at most COAST_MAX_COUNTS): it moves a learned coast a quarter
    /// towards it when it agrees with it. Otherwise it waits; the next sample, when it does not
    /// agree with the learned coast but with the waiting one, makes their mean the coast.
    pub fn refine(&mut self, sample: u16) {
        let sample = sample.min(COAST_MAX_COUNTS);
        if self.learned && agrees(self.counts, sample) {
            // rounded
            self.counts = ((3 * u32::from(self.counts) + u32::from(sample) + 2) / 4) as u16;
            self.candidate = None;
        } else if let Some(first) = self.candidate.filter(|&first| pair_agrees(first, sample)) {
            self.set((u32::from(first) + u32::from(sample)).div_ceil(2) as u16);
        } else {
            self.candidate = Some(sample);
        }
    }
}

/// Where a valve stands for the samples of its normal moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Probe {
    /// not known to be at the open end stop
    #[default]
    None,
    /// at the open end stop
    AtOpenEnd,
    /// a closing step away from the open end stop ended on its pulse count after `counted`
    /// pulses
    Away { counted: u32 },
}

/// How a normal move ended ([`ValveCoast::on_move`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveEnd {
    /// DIR_OPEN / DIR_CLOSE
    pub dir: u8,
    /// the motor stopped on the pulse count
    pub count_reached: bool,
    /// the motor stopped at the end-stop threshold of the end stop in `dir`, not early: a move
    /// to the end stop, or an open end move
    pub at_end: bool,
    pub counted: u32,
}

/// The learned closing coast of one valve and the closing step away from the open end stop that
/// waits for its move back. RAM only: a start of the STM begins without a coast.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValveCoast {
    close: CoastEstimate,
    probe: Probe,
}

impl ValveCoast {
    /// The learned coast of a step in `dir` in pulses: 0 for an opening step (not learned) and
    /// for a closing step before it is learned.
    pub fn counts(&self, dir: u8) -> u16 {
        if dir == DIR_CLOSE {
            self.close.counts()
        } else {
            0
        }
    }

    /// The sample of the calibration probe.
    pub fn set(&mut self, sample: u16) {
        self.close.set(sample);
    }

    /// The valve moved in a way that gives no sample (a move of another kind, a move of a failed
    /// or blocked valve, a stop on the safety limit or without current).
    pub fn forget(&mut self) {
        self.probe = Probe::None;
    }

    /// The end of a normal move. A closing step that leaves the open end stop and ends on its
    /// pulse count, followed by a move back that ends at the open end stop, gives a sample;
    /// every other move ends the pair.
    pub fn on_move(&mut self, m: &MoveEnd) {
        self.probe = if m.at_end && m.dir == DIR_OPEN {
            if let Probe::Away { counted } = self.probe {
                if let Some(sample) = coast_sample(counted, m.counted) {
                    self.close.refine(sample);
                }
            }
            Probe::AtOpenEnd
        } else if m.dir == DIR_CLOSE
            && m.count_reached
            && m.counted >= COAST_SAMPLE_MIN_COUNTS
            && self.probe == Probe::AtOpenEnd
        {
            Probe::Away { counted: m.counted }
        } else {
            Probe::None
        };
    }
}

#[cfg(test)]
mod tests;
