//! Valve sim of the glue tests (port of `test/native/glue/sim/valve_sim`): twelve actuators behind
//! the L293/MUX wiring of the board, driving the real motor module through the fake board.
//!
//! Every millisecond of fake time, in this order (the harness `motor::bench` calls the parts):
//! the motor model ([`Rig::on_ms`]: pulses on REVIN through the EXTI handler, which may cut the
//! enable at once), the timer interrupts (TIM1 `timer_handler0` once `valve_setup()` attached it,
//! TIM2 `valve_loop` once `setup_system()` attached it), then `valve_loop()` every 10th ms while
//! no timer calls it, and [`Rig::track`]. Other interleavings of the interrupts are injected by
//! the tests.
//!
//! A valve moves while the valve PSU is on (PSU enable low), its L293 enable is high and the MUX
//! selects it (even valve: the MUX_ON level of the board revision). Currents in 0.1 mA, positive
//! while opening, read by `timer_handler0` through the current and reference inputs (12 bit, the
//! inverse of its conversion).

use std::vec::Vec;

use crate::board::BoardRev;
use crate::hal::{CurrentAdc, Out, Pins};

use super::fake_board::FakeBoard;

const VALVES: usize = 12;
const ENA: [Out; VALVES / 2] = [
    Out::Ena0,
    Out::Ena1,
    Out::Ena2,
    Out::Ena3,
    Out::Ena4,
    Out::Ena5,
];
/// reference / 2 of the current amplifier (ANINREFHALF)
const ADC_MID: i64 = 2048;
/// hardware.h ANINCURRENTGAIN
const CURRENT_GAIN: f64 = 138.0;

#[derive(Clone, Copy, Debug)]
pub struct SimValve {
    /// false: open circuit, no current, no pulses
    pub connected: bool,
    /// short_current_dma once enabled, no pulses
    pub shorted: bool,
    /// counts from the closed end stop
    pub position: i32,
    /// counts between the end stops
    pub stroke: i32,
    /// motor speed
    pub pulses_per_ms: f32,
    /// turning freely
    pub run_current_dma: i32,
    /// at an end stop or at the obstacle
    pub stall_current_dma: i32,
    pub short_current_dma: i32,
    /// current in the first inrush_ms after the enable (0: no inrush)
    pub inrush_peak_dma: i32,
    pub inrush_ms: u32,
    /// pulses the motor turns on after the enable went off
    pub coast_pulses: i32,
    /// pulses the motor turns after a reversal before the valve moves (the play of the gears)
    pub backlash_pulses: i32,
    /// pulses the turning motor still makes its way when the direction flips with the enable on
    /// (a brake), slowing down evenly; then it turns the other way, speeding up over as many
    /// pulses. 0: it turns the other way at once
    pub brake_pulses: i32,
    /// current while braking (the sign of the direction output)
    pub brake_current_dma: i32,
    /// obstacle: opening stalls in [jam_from, jam_to), closing stalls in (jam_from, jam_to]
    pub jam_from: i32,
    pub jam_to: i32,
    // ---- read by the tests
    /// pulses the valve turned (coast included)
    pub pulses: u32,
    /// times its enable went on
    pub enables: u32,
}

impl Default for SimValve {
    fn default() -> Self {
        SimValve {
            connected: true,
            shorted: false,
            position: 1800,
            stroke: 3600,
            pulses_per_ms: 0.2,
            run_current_dma: 250,
            stall_current_dma: 700,
            short_current_dma: 2000,
            inrush_peak_dma: 0,
            inrush_ms: 0,
            coast_pulses: 0,
            backlash_pulses: 0,
            brake_pulses: 0,
            brake_current_dma: 1400,
            jam_from: -1,
            jam_to: -1,
            pulses: 0,
            enables: 0,
        }
    }
}

/// the valve is at an end stop or at its obstacle in the direction dir
fn stalled(v: &SimValve, dir: i32) -> bool {
    if dir > 0 {
        return v.position >= v.stroke
            || (v.jam_from >= 0 && v.position >= v.jam_from && v.position < v.jam_to);
    }
    v.position <= 0 || (v.jam_from >= 0 && v.position > v.jam_from && v.position <= v.jam_to)
}

/// One change of the valve state machine (valve 255) or of a valve status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub ms: u32,
    pub valvestate: u8,
    /// valve whose status changed, 255 for a change of valvestate
    pub valve: u8,
    pub status: u8,
}

pub struct Rig {
    pub valve: [SimValve; VALVES],
    /// every change of valvestate or of a valve status
    pub transitions: Vec<Step>,
    /// ms since install()
    pub ms: u32,
    /// ms with more than one enable high
    pub conflicts: u32,
    board: BoardRev,
    current: i32,
    last_enabled: i32,
    enabled_ms: u32,
    pulse_acc: [f32; VALVES],
    coast_left: [i32; VALVES],
    coast_dir: [i32; VALVES],
    /// the direction the motor of each valve turns while it is enabled: after a reversal with
    /// the enable on the old one until its brake pulses are spent
    motion_dir: [i32; VALVES],
    brake_left: [i32; VALVES],
    /// pulses of the speed-up after a brake left
    spinup_left: [i32; VALVES],
    /// direction of the last pulse, and the play left before the valve moves
    turn_dir: [i32; VALVES],
    slack_left: [i32; VALVES],
    last_state: u8,
    last_status: [u8; VALVES],
}

impl Rig {
    pub fn new(board: BoardRev) -> Self {
        Rig {
            valve: [SimValve::default(); VALVES],
            transitions: Vec::new(),
            ms: 0,
            conflicts: 0,
            board,
            current: 0,
            last_enabled: -1,
            enabled_ms: 0,
            pulse_acc: [0.0; VALVES],
            coast_left: [0; VALVES],
            coast_dir: [0; VALVES],
            motion_dir: [0; VALVES],
            brake_left: [0; VALVES],
            spinup_left: [0; VALVES],
            turn_dir: [0; VALVES],
            slack_left: [0; VALVES],
            last_state: 0xFF,
            last_status: [0; VALVES],
        }
    }

    /// Starts the model: the time and the transitions count from here.
    pub fn install(&mut self, valvestate: u8, statuses: [u8; VALVES]) {
        self.ms = 0;
        self.last_state = valvestate;
        self.last_status = statuses;
    }

    /// valve PSU on
    pub fn powered(&self, board: &FakeBoard) -> bool {
        !board.latch(Out::PsuEna)
    }

    /// valve whose enable and MUX position are active, -1 if none
    pub fn enabled_valve(&self, board: &FakeBoard) -> i32 {
        for (k, pin) in ENA.iter().enumerate() {
            if board.latch(*pin) {
                let even = board.latch(Out::Mux) == self.board.mux_on_high();
                return (2 * k + usize::from(!even)) as i32;
            }
        }
        -1
    }

    /// the direction output is low
    pub fn opening(&self, board: &FakeBoard) -> bool {
        !board.latch(Out::Dir)
    }

    pub fn current_dma(&self) -> i32 {
        self.current
    }

    /// The ADC value of the current input: the inverse of timer_handler0's conversion
    /// (current = (adc - ref) * ANINCURRENTGAIN / 100).
    pub fn adc_counts(&self) -> u16 {
        let counts = (f64::from(self.current) * 100.0 / CURRENT_GAIN).round() as i64;
        (ADC_MID + counts).clamp(0, 4095) as u16
    }

    /// One pulse of the motor of valve `i` in `dir`: after a reversal the play of the gears takes
    /// the first backlash_pulses, then the valve moves.
    fn turn(&mut self, i: usize, dir: i32) {
        if dir != self.turn_dir[i] {
            self.turn_dir[i] = dir;
            self.slack_left[i] = self.valve[i].backlash_pulses;
        }
        if self.slack_left[i] > 0 {
            self.slack_left[i] -= 1;
        } else {
            self.valve[i].position += dir;
        }
        self.valve[i].pulses += 1;
    }

    /// The speed of the motor of valve `i` as a share of its running speed: lower while it brakes
    /// or speeds up after a brake.
    fn speed(&self, i: usize) -> f32 {
        let n = self.valve[i].brake_pulses;
        if n <= 0 {
            1.0
        } else if self.brake_left[i] > 0 {
            self.brake_left[i] as f32 / n as f32
        } else if self.spinup_left[i] > 0 {
            (n - self.spinup_left[i] + 1) as f32 / n as f32
        } else {
            1.0
        }
    }

    /// The motor model of one millisecond; `fire` is a rising edge on REVIN (the EXTI handler).
    pub fn on_ms(&mut self, board: &FakeBoard, fire: &mut dyn FnMut()) {
        self.ms += 1;
        let enables = ENA.iter().filter(|&&pin| board.latch(pin)).count();
        if enables > 1 {
            self.conflicts += 1;
        }

        let dir = if self.opening(board) { 1 } else { -1 };
        let e = if self.powered(board) {
            self.enabled_valve(board)
        } else {
            -1
        };
        if e != self.last_enabled {
            // the motor that lost its enable turns on for its coast pulses, fewer at a lower
            // speed
            if self.last_enabled >= 0 {
                let l = self.last_enabled as usize;
                self.coast_left[l] =
                    (self.valve[l].coast_pulses as f32 * self.speed(l)).round() as i32;
                self.coast_dir[l] = self.motion_dir[l];
                self.brake_left[l] = 0;
                self.spinup_left[l] = 0;
                self.pulse_acc[l] = 0.0;
            }
            if e >= 0 {
                let i = e as usize;
                self.valve[i].enables += 1;
                self.coast_left[i] = 0;
                self.motion_dir[i] = dir;
                self.brake_left[i] = 0;
                self.spinup_left[i] = 0;
                self.pulse_acc[i] = 0.0;
            }
            self.enabled_ms = 0;
            self.last_enabled = e;
        }

        self.current = 0;
        if e >= 0 {
            let i = e as usize;
            self.enabled_ms += 1;
            let v = self.valve[i];
            // a reversal with the enable on: a turning motor with brake pulses brakes first
            if dir != self.motion_dir[i] && self.brake_left[i] == 0 {
                if v.brake_pulses > 0 && !stalled(&v, self.motion_dir[i]) {
                    self.brake_left[i] = v.brake_pulses;
                    self.pulse_acc[i] = 0.0;
                } else {
                    self.motion_dir[i] = dir;
                    self.spinup_left[i] = 0;
                }
            }
            if v.connected && v.shorted {
                self.current = dir * v.short_current_dma;
            } else if v.connected && self.brake_left[i] > 0 {
                // braking: the pulses of the slowing motor still go its way
                let md = self.motion_dir[i];
                self.current = dir * v.brake_current_dma;
                self.pulse_acc[i] += v.pulses_per_ms * self.speed(i);
                while self.pulse_acc[i] >= 1.0
                    && self.brake_left[i] > 0
                    && !stalled(&self.valve[i], md)
                    && self.enabled_valve(board) == e
                {
                    self.pulse_acc[i] -= 1.0;
                    self.brake_left[i] -= 1;
                    self.turn(i, md);
                    fire();
                }
                if stalled(&self.valve[i], md) {
                    self.brake_left[i] = 0;
                }
                if self.brake_left[i] == 0 {
                    // the motor stands: driven on, it speeds up the other way
                    self.motion_dir[i] = dir;
                    self.spinup_left[i] = v.brake_pulses;
                    self.pulse_acc[i] = 0.0;
                }
            } else if v.connected {
                let inrush = v.inrush_peak_dma != 0 && self.enabled_ms <= v.inrush_ms;
                self.current = dir
                    * if inrush {
                        v.inrush_peak_dma
                    } else if stalled(&v, dir) {
                        v.stall_current_dma
                    } else {
                        v.run_current_dma
                    };
                self.pulse_acc[i] += v.pulses_per_ms * self.speed(i);
                // every pulse runs the EXTI handler, which may switch the enable off at once
                while self.pulse_acc[i] >= 1.0
                    && !stalled(&self.valve[i], dir)
                    && self.enabled_valve(board) == e
                {
                    self.pulse_acc[i] -= 1.0;
                    self.spinup_left[i] = (self.spinup_left[i] - 1).max(0);
                    self.turn(i, dir);
                    fire();
                }
                if stalled(&self.valve[i], dir) {
                    self.pulse_acc[i] = 0.0;
                }
            }
            if self.enabled_valve(board) != e {
                self.current = 0;
            }
        }

        for k in 0..VALVES {
            if k as i32 == e || self.coast_left[k] <= 0 {
                continue;
            }
            self.pulse_acc[k] += self.valve[k].pulses_per_ms;
            while self.pulse_acc[k] >= 1.0
                && self.coast_left[k] > 0
                && !stalled(&self.valve[k], self.coast_dir[k])
            {
                self.pulse_acc[k] -= 1.0;
                self.coast_left[k] -= 1;
                self.turn(k, self.coast_dir[k]);
                fire();
            }
            if stalled(&self.valve[k], self.coast_dir[k]) {
                self.coast_left[k] = 0;
            }
        }
    }

    /// After the interrupts of a millisecond: records the changes of the valve state machine and
    /// of the valve statuses.
    pub fn track(&mut self, valvestate: u8, status_of: impl Fn(usize) -> u8) {
        if valvestate != self.last_state {
            self.transitions.push(Step {
                ms: self.ms,
                valvestate,
                valve: 255,
                status: 0,
            });
            self.last_state = valvestate;
        }
        for v in 0..VALVES {
            let status = status_of(v);
            if status != self.last_status[v] {
                self.transitions.push(Step {
                    ms: self.ms,
                    valvestate,
                    valve: v as u8,
                    status,
                });
                self.last_status[v] = status;
            }
        }
    }
}

/// The ADC of timer_handler0 fed by the sim: (current input, reference input).
pub struct SimAdc(pub u16);

impl CurrentAdc for SimAdc {
    fn sample(&mut self) -> (u16, u16) {
        (self.0, ADC_MID as u16)
    }
}

/// A fixed ADC (C++ `fake::board.analogValue[...]` without the sim).
pub struct FixedAdc {
    pub current: u16,
    pub reference: u16,
}

impl CurrentAdc for FixedAdc {
    fn sample(&mut self) -> (u16, u16) {
        (self.current, self.reference)
    }
}

#[cfg(test)]
mod tests;
