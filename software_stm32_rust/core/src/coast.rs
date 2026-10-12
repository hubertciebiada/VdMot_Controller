//! The coast of a valve motor: the pulses it turns after its enable went off. The revolution
//! pulses come from the motor current, which stops with the enable, so a step that stops on its
//! pulse count travels further than it counted. A step away from an end stop and the move back
//! to that end stop measure it: the move back counts the step and its coast. A partial move is
//! shortened by the coast learned for its direction. Hardware-free.
//!
//! Every sample comes from a step next to an end stop, where the load of the insert differs
//! most from the middle of the stroke; the coast of a step in the middle is not measured.

use crate::move_classifier::{DIR_CLOSE, DIR_OPEN};

/// Pulses of the step away from an end stop that a calibration makes in each direction (about
/// 4 % of a stroke of 4500..5400 pulses).
pub const COAST_PROBE_COUNTS: u16 = 200;
/// The shortest step of a normal move that gives a sample with the move back: a shorter step
/// can stop before the motor reaches its speed.
pub const COAST_SAMPLE_MIN_COUNTS: u32 = 100;
/// A larger difference between the move back and the step is no coast: one of them did not end
/// where the sample assumes.
pub const COAST_MAX_COUNTS: u16 = 200;

/// The other direction.
pub fn opposite(dir: u8) -> u8 {
    if dir == DIR_OPEN {
        DIR_CLOSE
    } else {
        DIR_OPEN
    }
}

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

/// The learned coast of one direction: the first sample, then a running mean that gives each
/// further sample a quarter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoastEstimate {
    counts: u16,
    learned: bool,
}

impl CoastEstimate {
    /// The learned coast in pulses, 0 before the first sample.
    pub fn counts(&self) -> u16 {
        self.counts
    }

    pub fn add(&mut self, sample: u16) {
        self.counts = if self.learned {
            // rounded; the mean of two u16 values fits a u16
            ((3 * u32::from(self.counts) + u32::from(sample) + 2) / 4) as u16
        } else {
            sample
        };
        self.learned = true;
    }
}

/// Where a valve stands for the samples of its normal moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Probe {
    /// not known to be at an end stop
    #[default]
    None,
    /// at the end stop of this direction (DIR_OPEN: the open end stop)
    AtEnd(u8),
    /// a step away from the end stop of `end` ended on its pulse count after `counted` pulses
    Away { end: u8, counted: u32 },
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

/// The learned coast of one valve and the step away from an end stop that waits for its move
/// back. RAM only: a start of the STM begins without a coast.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValveCoast {
    open: CoastEstimate,
    close: CoastEstimate,
    probe: Probe,
}

impl ValveCoast {
    /// The learned coast of a step in `dir` in pulses, 0 before the first sample.
    pub fn counts(&self, dir: u8) -> u16 {
        if dir == DIR_OPEN {
            self.open.counts()
        } else {
            self.close.counts()
        }
    }

    /// A sample of the coast of a step in `dir`.
    pub fn add(&mut self, dir: u8, sample: u16) {
        if dir == DIR_OPEN {
            self.open.add(sample);
        } else {
            self.close.add(sample);
        }
    }

    /// The valve stands at the end stop of `end` (a calibration closed it).
    pub fn at_end(&mut self, end: u8) {
        self.probe = Probe::AtEnd(end);
    }

    /// The valve moved in a way that gives no sample (a move of another kind, a move of a failed
    /// or blocked valve, a stop on the safety limit or without current).
    pub fn forget(&mut self) {
        self.probe = Probe::None;
    }

    /// The end of a normal move. A step that leaves an end stop and ends on its pulse count,
    /// followed by a move back that ends at that end stop, gives a sample of the step's
    /// direction; every other move ends the pair.
    pub fn on_move(&mut self, m: &MoveEnd) {
        self.probe = if m.at_end {
            if let Probe::Away { end, counted } = self.probe {
                if end == m.dir {
                    if let Some(sample) = coast_sample(counted, m.counted) {
                        self.add(opposite(end), sample);
                    }
                }
            }
            Probe::AtEnd(m.dir)
        } else if m.count_reached
            && m.counted >= COAST_SAMPLE_MIN_COUNTS
            && self.probe == Probe::AtEnd(opposite(m.dir))
        {
            Probe::Away {
                end: opposite(m.dir),
                counted: m.counted,
            }
        } else {
            Probe::None
        };
    }
}

#[cfg(test)]
mod tests;
