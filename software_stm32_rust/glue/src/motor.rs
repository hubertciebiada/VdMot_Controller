//! Valve motors (port of `software_stm32/src/motor.cpp` and `include/motor.h`): the valve state
//! machine ([`valve_loop`], TIM2, every 10 ms), the motor state machine (`motorcycle`), the current
//! sampling and soft start ([`timer_handler0`], TIM1, every 1 ms), the revolution pulses
//! ([`Pulse`], EXTI4) and the hand-over from the main loop ([`MotorShared::appsetaction`],
//! [`MotorShared::appsetservice`], [`MotorShared::appstop`]).
//!
//! Real-time model (docs/rust/GLUE-DESIGN-STM.md §2): TIM1 and TIM2 share NVIC priority 14 and
//! never preempt each other; both work on [`MotorShared`], which the firmware keeps in an
//! `IsrCell`. The main loop reaches it only under the lock, where the C++ masked the interrupts
//! or relied on `valve_idle()`. EXTI4 preempts both timers and touches only the [`Pulse`] atomics;
//! [`IsrFlags`] are the atomics TIM2 shares with the main loop outside the lock.
//!
//! The pin modes (MUX, ENA0..5 and DIR outputs, REVIN input, PSU open drain) are firmware set-up
//! (design §1.3); the glue writes the levels in the order of the C++.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use vdm_stm_core::calibration::{
    calibration_floor, end_stop_bound, escalated_bound, evaluate_calibration, learn_mean_current,
    stroke_mean_current, CalibrationVerdict, EscalationConfig, ESCALATION_DEFAULT,
    MEAN_CURRENT_DEFAULT_MA, MEAN_CURRENT_FLOOR_MA, MIN_TRAVEL_COUNTS, SAFETY_LIMIT_MA,
};
use vdm_stm_core::config_store::ConfigImage;
use vdm_stm_core::end_stop_detector::{EndStopDetector, InrushMode, Trip};
use vdm_stm_core::legacy_layout::VALVE_COUNT;
use vdm_stm_core::motor_params::MotorParams;
use vdm_stm_core::move_classifier::{
    classify_move, make_move_result, position_after_end_stop, EarlyStopRun, MotorStop, MoveRequest,
    MoveResult, StopReason, DIR_CLOSE, DIR_OPEN, RUN_TO_END_STOP,
};
use vdm_stm_core::presence_test::{presence_outcome, PresenceResult, PresenceTest};
use vdm_stm_core::profile_recorder::ProfileRecorder;
use vdm_stm_core::protection_guard::PROTECT_ENFORCE;
use vdm_stm_core::stall_detector::StallDetector;
use vdm_stm_core::valve_codes::{
    ValveFault, ST_BLOCKED, ST_CLOSING, ST_FAILED, ST_IDLE, ST_OPENING, ST_OPEN_CIRCUIT,
};

use crate::app::{Valve, LEARN_AFTER_MOVEMENTS_DEFAULT, NO_OF_MIN_COUNTS, SVMOV_HOLD_10S};
use crate::board::BoardRev;
use crate::hal::{Clock, ControlTimer, CurrentAdc, Out, Pins, RevIrq};

mod pulse;
pub use pulse::Pulse;

const VALVES: usize = VALVE_COUNT as usize;

// commands of the valve state machine (appsetaction)
pub const CMD_A_OPEN: u8 = b'o';
pub const CMD_A_OPEN_END: u8 = b'p';
pub const CMD_A_CLOSE: u8 = b'c';
pub const CMD_A_CLOSE_END: u8 = b'v';
pub const CMD_A_LEARN: u8 = b'l';
pub const CMD_A_TARGET: u8 = b't';
pub const CMD_A_TEST: u8 = b'x';
/// service move (svmov), parameters in appsetservice()
pub const CMD_A_SERVICE: u8 = b's';

// flags of a move command (open/close, also to the end stop)
/// the valve keeps its status after the move (blocked valve to its failsafe position)
pub const MOVE_KEEP_STATUS: u8 = 0x01;
/// reference move: the start position is not known
pub const MOVE_REFERENCE: u8 = 0x02;

pub const SVMOV_COUNTS_MIN: u16 = 1;
pub const SVMOV_COUNTS_MAX: u16 = 10000;
pub const SVMOV_MAXMA_MIN: u8 = 5;
pub const SVMOV_MAXMA_MAX: u8 = SAFETY_LIMIT_MA;

/// 1 ms (TIMER0_INTERVAL_MS * 1000)
const TIMER0_INTERVAL_US: u32 = 1000;
/// 120 s with 10 ms cycle time (120*100)
const TIMEOUT_NORMALCURRENT: i32 = 12000;
/// cycles of motorcycle() (4*50)
const TIMEOUT_UNDERCURRENT: i32 = 200;
/// threshold for detecting undercurrent in 1/10 mA
const THRESHOLD_UNDERCURRENT: i32 = 20;
/// cycles of motorcycle() to wait for timer_handler0 (1 ms) to enable the motor
const TIMEOUT_TURNON: i32 = 10;
/// 5 minutes with 10 ms cycle time (5*60*100), more than the longest valve state (one move <= ~123 s)
const TIMEOUT_VALVESTATE: u32 = 30000;
/// longest pause between two calibration strokes for a temperature cycle (3 s)
const TIMEOUT_TEMPGAP: i32 = 300;
/// idle cycles before the valve PSU and the MUX relay go off (15 s)
const PSU_OFF_TICKS: i32 = 1500;
/// idle cycles before the temperature measurement is unlocked
const LOCK_TICKS: i32 = 50;

// commands of the motor state machine
const CMD_M_OPEN: u8 = b'o';
const CMD_M_CLOSE: u8 = b'c';
const CMD_M_STOP: u8 = b's';
const CMD_M_NOTHING: u8 = b'n';
const CMD_M_TEST: u8 = b't';

// results of the motor state machine
const M_RES_INIT: u8 = 1;
const M_RES_IDLE: u8 = 2;
const M_RES_OPENS: u8 = 3;
const M_RES_CLOSES: u8 = 4;
const M_RES_TURNING: u8 = 5;
const M_RES_ENDSTOP: u8 = 6;
const M_RES_STOP: u8 = 7;
const M_RES_NOCURRENT: u8 = 9;
const M_RES_TEST: u8 = 10;
const M_RES_ERROR: u8 = 11;

/// 2 s (2*100 cycles): PSU on before the presence test
const WAIT_TIMER100: i32 = 200;
/// 1 s (2*50 cycles): PSU on before a move, between calibrations
const WAIT_TIMER50: i32 = 100;
/// 0.4 s (2*20 cycles): between calibration strokes
const WAIT_TIMER20: i32 = 40;
/// the MUX relay settles WAIT_MUX = 100 ms after set_motor() ((WAIT_MUX + 9) / 10 cycles)
const MUX_SETTLE_TICKS: i32 = 10;

/// current valve motor in 1/10 mA per ADC digit x 100 (hardware.h ANINCURRENTGAIN)
const ANINCURRENTGAIN: i32 = 138;

// kinds of the move in progress
/// position change requested by app_loop (counts for early stops)
const MOVE_NORMAL: u8 = 0;
/// calibration stroke
const MOVE_LEARN: u8 = 1;
/// service move (svmov)
const MOVE_SERVICE: u8 = 2;

/// valveindex of valve_loop while it works on no valve
const NO_VALVE: usize = 255;

const ENA_PINS: [Out; VALVES / 2] = [
    Out::Ena0,
    Out::Ena1,
    Out::Ena2,
    Out::Ena3,
    Out::Ena4,
    Out::Ena5,
];

/// Valve status codes are `vdm_stm_core::valve_codes::ST_*` (C++ VLV_STATE_*).
/// C++ `enum CALIBSTATE {calibIdle, calibStarted, calibInProgress}`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum CalibState {
    #[default]
    Idle = 0,
    Started = 1,
    InProgress = 2,
}

/// The states of the valve state machine (C++ `enum ASTATE`, `A_*`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum AState {
    #[default]
    Init = 0,
    Idle = 1,
    Close = 2,
    Open1 = 3,
    Open2 = 4,
    Learn1 = 5,
    Learn2 = 6,
    Learn3 = 7,
    Learn4 = 8,
    Set = 9,
    Set1 = 10,
    Set2 = 11,
    Close1 = 12,
    Close2 = 13,
    Test = 14,
    Svc1 = 15,
    Svc2 = 16,
    Gap = 17,
}

/// The states of the motor state machine (`M_*` of motorcycle()).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum MState {
    Init = 0,
    Idle = 1,
    Open = 2,
    Close = 3,
    Turning = 4,
    Stop = 5,
    Undercurr = 7,
    TurnOn = 8,
    TestPrep = 9,
    Test = 10,
    /// wait for the MUX relay after set_motor()
    Settle = 11,
    /// start motor after Settle (open/close)
    Start = 12,
    /// start motor after Settle (test)
    TestStart = 13,
}

/// One valve motor (C++ `struct valvemotor`, `myvalvemots[]`), shared between the valve state
/// machine (TIM2) and the main loop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValveMotor {
    pub closing_count: u32,
    pub opening_count: u32,
    /// closing_count - opening_count, may be negative
    pub deadzone_count: i32,
    pub scaler: u32,
    pub meancurrent: u32,
    pub target_position: u8,
    pub actual_position: u8,
    pub status: u8,
    pub calibration: bool,
    pub calib_time: u8,
    pub calib_state: CalibState,
    pub connected: bool,
    pub calib_retries: u8,
    /// a calibration of this valve is running (valve state machine)
    pub calib_active: bool,
    // written by the valve state machine, read and cleared by the main loop where noted
    /// counts of a successful calibration (learned or restored from the EEPROM)
    pub calibrated: bool,
    /// a full calibration is required (contact lost, stdet, stored calibration ended blocked)
    pub recal: bool,
    /// the position is not referenced: the next move goes to an end stop first
    pub needs_reference: bool,
    /// counts the calibrations that ended accepted or blocked (EEPROM record)
    pub calib_seq: u8,
    /// the last calibration ended blocked
    pub calib_failed: bool,
    /// second early partial stop in a row (main loop clears it)
    pub early_learn_due: bool,
    /// ValveFault (gvlvy fault)
    pub fault_reason: u8,
    /// counts the normal moves that ended (end-stop latch of the scheduler)
    pub move_seq: u8,
    /// counts short verdicts and inrush trips (protection guard)
    pub trip_seq: u8,
}

/// Diagnostics of one valve, written by the valve state machine (C++ `struct valve_diag`).
#[derive(Clone, Copy, Debug, Default)]
pub struct ValveDiag {
    /// last move (normal, calibration stroke or service move)
    pub last: MoveResult,
    /// early end stops of normal moves since start-up
    pub early_stops: u16,
    /// early end stop since the last successful calibration
    pub early_warn: bool,
    /// the last calibration did not succeed
    pub last_cal_failed: bool,
    /// the last move stopped early (classify_move)
    pub last_early: bool,
    /// early partial stops in a row
    pub early_run: EarlyStopRun,
}

/// The fields gvlvx reports, copied together (C++ `struct valve_snapshot`): the valve state
/// machine changes several of them in one step (a calibration pass, the end of a move).
#[derive(Clone, Copy, Debug, Default)]
pub struct ValveSnapshot {
    pub diag: ValveDiag,
    pub opening_count: u32,
    pub closing_count: u32,
    pub deadzone_count: i32,
    pub meancurrent: u32,
    pub movements: u32,
    pub status: u8,
    pub actual_position: u8,
    pub target_position: u8,
    pub calibration: bool,
    pub calib_retries: u8,
    pub calib_active: bool,
}

/// Per valve diagnostics and the current profile of its last move.
#[derive(Clone, Copy, Debug, Default)]
struct ValveRecord {
    diag: ValveDiag,
    profile: ProfileRecorder,
}

/// The flags TIM2 shares with the main loop outside the lock (design §2.3).
pub struct IsrFlags {
    /// incremented on every valve_loop run (watchdog heartbeat)
    pub valve_loop_ticks: AtomicU32,
    /// valve state machine stuck in one busy state (the watchdog must starve)
    pub valve_loop_stalled: AtomicBool,
    /// owDevices `lock`: the temperature state machine is locked (valve_loop's
    /// `temp_command(TEMP_CMD_LOCK)` sets it, `TEMP_CMD_UNLOCK` clears it)
    pub temp_lock: AtomicBool,
    /// main loop: a temperature cycle is due (pause between calibration strokes)
    pub temp_refresh_request: AtomicBool,
    /// valve_loop: the pause between two calibration strokes timed out
    pub temp_gap_timeout: AtomicBool,
    /// main loop: the short and inrush limits are off until the next start
    pub protect_suspended: AtomicBool,
    /// short and inrush limits stop the motor (protection_guard::PROTECT_ENFORCE)
    pub protect_enforce: AtomicBool,
}

impl IsrFlags {
    pub const fn new() -> Self {
        IsrFlags {
            valve_loop_ticks: AtomicU32::new(0),
            valve_loop_stalled: AtomicBool::new(false),
            temp_lock: AtomicBool::new(false),
            temp_refresh_request: AtomicBool::new(false),
            temp_gap_timeout: AtomicBool::new(false),
            protect_suspended: AtomicBool::new(false),
            protect_enforce: AtomicBool::new(PROTECT_ENFORCE),
        }
    }
}

impl Default for IsrFlags {
    fn default() -> Self {
        Self::new()
    }
}

/// Calls of motor.cpp into other glue modules (main-loop context: the hand-over and the
/// setters); the tests implement it with stubs that log the calls.
pub trait MotorEnv {
    /// app.cpp `app_warm_moving(valve)`: from here on the kept position of the valve is not
    /// valid (a warm reset during the move must not trust it)
    fn app_warm_moving(&mut self, valve: u32);
    /// eeprom.cpp `eep_content`: the RAM mirror of the EEPROM
    fn eep_content(&mut self) -> &mut ConfigImage;
}

/// The hardware of the valve state machine (TIM2): the outputs, the EXTI line of the revolution
/// pulses and the clock.
pub trait MotorHw: Pins + RevIrq + Clock {}

impl<T: Pins + RevIrq + Clock> MotorHw for T {}

/// The statics of valve_loop().
#[derive(Clone, Copy, Debug)]
struct LoopState {
    closing_count: u32,
    opening_count: u32,
    open_mean_ma: u16,
    open_mean_samples: u16,
    valveindex: usize,
    psuofftimer: i32,
    locktimer: i32,
    gaptimer: i32,
    /// stroke after the temperature gap
    gap_dir: u8,
    gap_next: AState,
    svc_move_dir: u8,
    svc_move_counts: u16,
    svc_move_maxma: u8,
    svc_prev_status: u8,
}

/// The statics of motorcycle().
#[derive(Clone, Copy, Debug)]
struct CycleState {
    motorstate: MState,
    settle_next: MState,
    settle_result: u8,
    settlecnt: i32,
    turnoncnt: i32,
    cyclecnt: i32,
    debouncecnt: i32,
    normalcurrcnt: i32,
    /// counts meancurrent values
    meancurrent_cnt: u16,
    /// memory for meancurrent values
    meancurrent_mem: i32,
}

/// What TIM2 works with besides [`MotorShared`].
struct Io<'a, H> {
    pulse: &'a Pulse,
    flags: &'a IsrFlags,
    hw: &'a H,
}

/// The state of motor.cpp that TIM1, TIM2 and the main loop share (design §2.3), with the app.cpp
/// data the valve state machine writes (`myvalves[]`, `learning_movements`). The firmware keeps it
/// in an `IsrCell`: TIM1 and TIM2 use it directly, the main loop under the lock.
pub struct MotorShared {
    /// `myvalvemots[]`
    pub mots: [ValveMotor; VALVES],
    /// app.cpp `myvalves[]`
    pub valves: [Valve; VALVES],
    /// app.cpp `learning_movements`: reload value of the movement trigger
    pub learning_movements: u32,
    /// lower current limit factor for detection of end stop
    pub currentbound_low_fac: u8,
    /// upper current limit factor for detection of end stop
    pub currentbound_high_fac: u8,
    /// valve % on power start
    pub start_on_power: u8,
    pub no_of_min_counts: u16,
    pub max_calib_retries: u8,
    /// filtered motor current of the presence test in 0.1 mA (TIM1; the terminal reads it)
    pub analog_current: i32,
    board: BoardRev,
    valvestate: AState,
    /// failed passes of the calibration in progress
    calib_retries: u8,
    /// current valve motor in 1/10 mA for normal mode
    current_ma: i32,
    /// filter memory of analog_current
    analog_current_old: i32,
    // hand-over main loop -> valve_loop
    command: u8,
    valvenr: u8,
    poschangecmd: u8,
    /// MOVE_KEEP_STATUS, MOVE_REFERENCE
    moveflagscmd: u8,
    svc_dir: u8,
    svc_counts: u16,
    svc_maxma: u8,
    /// sstop of the valve in work; cleared when the valve state machine is back in Idle
    stop_request: bool,
    calib_escalation: EscalationConfig,
    // TIM1 <-> TIM2
    isr_valvenr: i32,
    /// soft start requested (M_TURNON)
    isr_timer_go: bool,
    /// soft start done (timer_handler0 enabled the motor)
    isr_timer_fin: bool,
    isr_overcurrentevent: bool,
    /// end-stop detection, fed by timer_handler0, armed and read by the motor state machine
    endstop: EndStopDetector,
    // the move in progress (TIM2)
    undercurrcnt: i32,
    move_req: MoveRequest,
    move_kind: u8,
    /// end-stop bounds for the next motor start, 1/10 mA
    move_bound_low: i32,
    move_bound_high: i32,
    move_start_ms: u32,
    motor_stop_cause: MotorStop,
    /// current_ma at the last M_TURNING tick
    last_turning_current: i32,
    /// mean current of the last stroke that ended at an end stop
    stroke_mean_ma: u16,
    stroke_mean_samples: u16,
    move_profile: ProfileRecorder,
    /// flags of the normal move in progress
    move_flags: u8,
    /// status a MOVE_KEEP_STATUS move restores
    keep_status: u8,
    /// requested change of the normal move in %, 255: to the end stop
    pos_change: u8,
    waittimer: i32,
    test_presence: PresenceTest,
    valve_records: [ValveRecord; VALVES],
    valve_stall: StallDetector,
    lp: LoopState,
    mc: CycleState,
}

// position arithmetic clamped to 0..100 %
fn position_add(position: u8, delta: u8) -> u8 {
    // at most 100, fits a u8
    (u32::from(position) + u32::from(delta)).min(100) as u8
}

fn position_sub(position: u8, delta: u8) -> u8 {
    position.saturating_sub(delta)
}

fn psu_on(pins: &impl Pins) {
    pins.set(Out::PsuEna, false);
}

fn psu_off(pins: &impl Pins) {
    pins.set(Out::PsuEna, true);
}

fn mux_on(pins: &impl Pins, board: BoardRev) {
    pins.set(Out::Mux, board.mux_on_high());
}

fn mux_off(pins: &impl Pins, board: BoardRev) {
    pins.set(Out::Mux, !board.mux_on_high());
}

/// all motor enables off
pub(crate) fn ena_all_off(pins: &impl Pins) {
    for pin in ENA_PINS {
        pins.set(pin, false);
    }
}

/// enables the L293 channel of the valve (valves 2k and 2k + 1 share ENAk)
fn ena_on(pins: &impl Pins, valve: i32) {
    let pin = usize::try_from(valve)
        .ok()
        .and_then(|v| ENA_PINS.get(v / 2));
    if let Some(&pin) = pin {
        pins.set(pin, true);
    }
}

/// sets direction and MUX relay; the relay needs WAIT_MUX ms to settle before the motor is
/// enabled, motorcycle() waits in Settle
fn set_motor(pins: &impl Pins, board: BoardRev, valve: i32, dir: u8) {
    if valve % 2 != 0 {
        mux_off(pins, board);
    } else {
        mux_on(pins, board);
    }
    pins.set(Out::Dir, dir != DIR_OPEN);
}

/// Drives the valve PSU and the motor enables to their inactive level, right after reset and
/// before the 3 s boot window. The C++ writes the PSU latch before and after switching PB9 to
/// open drain (a reset value LOW would enable the PSU for a moment); the modes are firmware
/// set-up (PA15 and PB3 are JTAG pins with pull resistors after reset).
pub fn valve_pins_safe(pins: &impl Pins) {
    psu_off(pins);
    psu_off(pins);
    ena_all_off(pins);
}

/// `valve_loop`, the TIM2 interrupt (every 10 ms): the valve state machine.
pub fn valve_loop(m: &mut MotorShared, pulse: &Pulse, flags: &IsrFlags, hw: &impl MotorHw) {
    m.tick(&Io { pulse, flags, hw });
}

/// `TimerHandler0`, the TIM1 interrupt (every 1 ms): the motor current, the filtered current of
/// the presence test, the end-stop detection while the motor turns, and the soft start that the
/// motor state machine requested.
pub fn timer_handler0(
    m: &mut MotorShared,
    pulse: &Pulse,
    adc: &mut impl CurrentAdc,
    hw: &(impl Pins + RevIrq),
) {
    let (current, reference) = adc.sample();
    // current valve motor in 1/10 mA read by the analog pins
    let analog_value = (i32::from(current) - i32::from(reference)) * ANINCURRENTGAIN / 100;
    // filter of the test mode
    m.analog_current = (m.analog_current_old * 9800 + analog_value * 200) / 10000;
    m.analog_current_old = m.analog_current;

    if pulse.turning.load(Ordering::SeqCst) {
        // inrush time, filter, end-stop bounds, safety and hard limit (EndStopDetector)
        let trip = m.endstop.sample(analog_value) != Trip::None;
        m.current_ma = m.endstop.current();
        if trip {
            // stop the motor at once
            hw.detach();
            ena_all_off(hw);
            pulse.turning.store(false, Ordering::SeqCst);
            m.isr_overcurrentevent = true;
        }
    } else {
        m.endstop.idle();
        m.current_ma = 0;
    }

    // enable the valve (soft start requested by M_TURNON and not cancelled since)
    if pulse.turning.load(Ordering::SeqCst) && m.isr_timer_go && !m.isr_timer_fin {
        m.isr_timer_fin = true;
        ena_on(hw, m.isr_valvenr);
    }
}

impl MotorShared {
    /// The start values of the C++ statics. `board` selects the MUX levels.
    pub fn new(board: BoardRev) -> Self {
        MotorShared {
            mots: [ValveMotor::default(); VALVES],
            valves: [Valve::default(); VALVES],
            learning_movements: u32::from(LEARN_AFTER_MOVEMENTS_DEFAULT),
            currentbound_low_fac: 17,
            currentbound_high_fac: 17,
            start_on_power: 50,
            no_of_min_counts: NO_OF_MIN_COUNTS,
            max_calib_retries: 0,
            analog_current: 0,
            board,
            valvestate: AState::Init,
            calib_retries: 0,
            current_ma: 0,
            analog_current_old: 0,
            command: 0,
            valvenr: 0,
            poschangecmd: 0,
            moveflagscmd: 0,
            svc_dir: 0,
            svc_counts: 0,
            svc_maxma: 0,
            stop_request: false,
            calib_escalation: ESCALATION_DEFAULT,
            isr_valvenr: 0,
            isr_timer_go: false,
            isr_timer_fin: false,
            isr_overcurrentevent: false,
            endstop: EndStopDetector::default(),
            undercurrcnt: 0,
            move_req: MoveRequest::default(),
            move_kind: MOVE_NORMAL,
            move_bound_low: 0,
            move_bound_high: 0,
            move_start_ms: 0,
            motor_stop_cause: MotorStop::None,
            last_turning_current: 0,
            stroke_mean_ma: 0,
            stroke_mean_samples: 0,
            move_profile: ProfileRecorder::default(),
            move_flags: 0,
            keep_status: 0,
            pos_change: 0,
            waittimer: 0,
            test_presence: PresenceTest::default(),
            valve_records: [ValveRecord::default(); VALVES],
            valve_stall: StallDetector::new(TIMEOUT_VALVESTATE),
            lp: LoopState {
                closing_count: 0,
                opening_count: 0,
                open_mean_ma: 0,
                open_mean_samples: 0,
                valveindex: 0,
                psuofftimer: 0,
                locktimer: 0,
                gaptimer: 0,
                gap_dir: DIR_OPEN,
                gap_next: AState::Learn3,
                svc_move_dir: 0,
                svc_move_counts: 0,
                svc_move_maxma: 0,
                svc_prev_status: 0,
            },
            mc: CycleState {
                motorstate: MState::Init,
                settle_next: MState::Idle,
                settle_result: M_RES_TURNING,
                settlecnt: 0,
                turnoncnt: 0,
                cyclecnt: 0,
                debouncecnt: 0,
                normalcurrcnt: 0,
                meancurrent_cnt: 0,
                meancurrent_mem: 0,
            },
        }
    }

    /// Start values of every valve and the 1 ms current timer (TIM1); called from
    /// setup_system() after app_setup() loaded the motor parameters.
    pub fn valve_setup(&mut self, tim1: &mut impl ControlTimer) -> u8 {
        let start = self.start_on_power;
        for mot in &mut self.mots {
            mot.actual_position = start;
            mot.target_position = start;
            mot.meancurrent = u32::from(MEAN_CURRENT_DEFAULT_MA);
            mot.scaler = 89;
            mot.calib_retries = 0;
            mot.calib_active = false;
            mot.calibrated = false;
            mot.recal = false;
            mot.needs_reference = false;
            mot.calib_seq = 0;
            mot.calib_failed = false;
            mot.early_learn_due = false;
            mot.fault_reason = ValveFault::None as u8;
            mot.move_seq = 0;
            mot.trip_seq = 0;
        }
        self.valvestate = AState::Init;
        // the result is only printed by the debug lines of motDebug, which no release env builds
        tim1.attach_interval(TIMER0_INTERVAL_US);
        0
    }

    pub fn motor_get_params(&self) -> MotorParams {
        MotorParams {
            low_fac: self.currentbound_low_fac,
            high_fac: self.currentbound_high_fac,
            start_on_power: self.start_on_power,
            min_counts: self.no_of_min_counts,
            max_retries: self.max_calib_retries,
        }
    }

    /// Takes validated parameters into RAM and the EEPROM mirror (the caller decides about
    /// writing).
    pub fn motor_set_params(&mut self, env: &mut impl MotorEnv, params: &MotorParams) {
        self.currentbound_low_fac = params.low_fac;
        self.currentbound_high_fac = params.high_fac;
        self.start_on_power = params.start_on_power;
        self.no_of_min_counts = params.min_counts;
        self.max_calib_retries = params.max_retries;
        let layout = &mut env.eep_content().layout;
        layout.currentbound_low_fac = params.low_fac;
        layout.currentbound_high_fac = params.high_fac;
        layout.start_on_power = params.start_on_power;
        layout.no_of_min_counts = params.min_counts;
        layout.max_calib_retries = params.max_retries;
    }

    /// Breakaway escalation of calibration repetitions (scalx/gcalx).
    pub fn motor_get_escalation(&self) -> EscalationConfig {
        self.calib_escalation
    }

    /// Takes a validated configuration into RAM and the EEPROM mirror.
    pub fn motor_set_escalation(&mut self, env: &mut impl MotorEnv, config: &EscalationConfig) {
        self.calib_escalation = *config;
        env.eep_content().escalation = *config;
    }

    /// Consistent copy of the fields gvlvx reports; an empty one for valve 12 and above.
    pub fn valve_get_snapshot(&self, valveindex: u32, out: &mut ValveSnapshot) {
        let Some(v) = valve_index(valveindex) else {
            *out = ValveSnapshot::default();
            return;
        };
        let mot = &self.mots[v];
        out.diag = self.valve_records[v].diag;
        out.opening_count = mot.opening_count;
        out.closing_count = mot.closing_count;
        out.deadzone_count = mot.deadzone_count;
        out.meancurrent = mot.meancurrent;
        out.movements = self.valves[v].movements;
        out.status = mot.status;
        out.actual_position = mot.actual_position;
        out.target_position = mot.target_position;
        out.calibration = mot.calibration;
        out.calib_retries = mot.calib_retries;
        out.calib_active = mot.calib_active;
    }

    /// The current profile of the last move of the valve; empty for valve 12 and above.
    pub fn valve_get_profile(&self, valveindex: u32, out: &mut ProfileRecorder) {
        match valve_index(valveindex) {
            Some(v) => *out = self.valve_records[v].profile,
            None => out.reset(),
        }
    }

    /// State of the valve state machine.
    pub fn valve_getstate(&self) -> AState {
        self.valvestate
    }

    /// The valve state machine is idle and no command is pending: no valve moves until the main
    /// loop hands over the next command, so until then the main loop may change the state of any
    /// valve.
    pub fn valve_idle(&self) -> bool {
        self.valvestate == AState::Idle && self.command == 0
    }

    /// Hands a command to the valve state machine: 0 taken, -1 refused (valve 12 and above, or a
    /// command accepted earlier but not yet taken, or the machine busy, unless `force`).
    /// C++ defaults: force false, flags 0.
    pub fn appsetaction(
        &mut self,
        env: &mut impl MotorEnv,
        cmd: u8,
        valveindex: u32,
        posdelta: u8,
        force: bool,
        flags: u8,
    ) -> i16 {
        let mut accepted = false;
        if let Some(v) = valve_index(valveindex) {
            // a command accepted earlier but not yet taken by valve_loop must not be overwritten
            if (self.valvestate == AState::Idle && self.command == 0) || force {
                self.valvenr = v as u8;
                self.poschangecmd = posdelta;
                self.moveflagscmd = flags;
                // from here on the kept position of the valve is not valid (warm reset in the move)
                env.app_warm_moving(valveindex);
                // last: valve_loop acts on the command
                self.command = cmd;
                accepted = true;
            }
        }
        if accepted {
            0
        } else {
            -1
        }
    }

    /// sstop: drops a command not yet taken and stops the move, calibration or service move of
    /// the valve (255: of any valve). Returns the valve whose command or move was stopped, -1 if
    /// none.
    pub fn appstop(&mut self, valve: u32) -> i16 {
        let mut stopped = -1;
        let mine = valve == 255 || u32::from(self.valvenr) == valve;
        if self.command != 0 && mine {
            // handed over but not taken yet: the command is dropped
            self.command = 0;
            stopped = i16::from(self.valvenr);
        } else if self.valvestate != AState::Idle && self.valvestate != AState::Init && mine {
            // the waiting states stop the motor; the request ends when the machine is back in Idle
            self.stop_request = true;
            stopped = i16::from(self.valvenr);
        }
        stopped
    }

    /// The valve the valve state machine works on or got a command for, -1 while idle.
    pub fn valve_busy_index(&self) -> i32 {
        if (self.valvestate == AState::Idle || self.valvestate == AState::Init) && self.command == 0
        {
            -1
        } else {
            i32::from(self.valvenr)
        }
    }

    /// Service move: dir DIR_OPEN/DIR_CLOSE, counts 1..10000, end-stop threshold maxma 5..60.
    /// 0 taken, -1 invalid arguments, -2 the valve state machine is busy.
    pub fn appsetservice(
        &mut self,
        env: &mut impl MotorEnv,
        valveindex: u32,
        dir: u8,
        counts: u16,
        maxma: u8,
    ) -> i16 {
        let Some(v) = valve_index(valveindex) else {
            return -1;
        };
        if dir > DIR_CLOSE
            || !(SVMOV_COUNTS_MIN..=SVMOV_COUNTS_MAX).contains(&counts)
            || !(SVMOV_MAXMA_MIN..=SVMOV_MAXMA_MAX).contains(&maxma)
        {
            return -1;
        }
        if self.valvestate != AState::Idle || self.command != 0 {
            return -2;
        }
        self.svc_dir = dir;
        self.svc_counts = counts;
        self.svc_maxma = maxma;
        self.valvenr = v as u8;
        // the valve is left where the move puts it; set before the command, as a refused start
        // clears it
        self.valves[v].svc_hold = SVMOV_HOLD_10S;
        env.app_warm_moving(valveindex);
        // last: valve_loop acts on the command
        self.command = CMD_A_SERVICE;
        0
    }

    // ------------------------------------------------------------------ the move in progress

    /// Full stroke of the last successful calibration in the direction of the move, 0 if unknown.
    fn learned_travel(&self, v: usize, dir: u8) -> u32 {
        let mot = &self.mots[v];
        let travel = if dir == DIR_OPEN {
            mot.opening_count
        } else {
            mot.closing_count
        };
        if mot.scaler > 0 && travel >= MIN_TRAVEL_COUNTS {
            travel
        } else {
            0
        }
    }

    /// End-stop bounds from the learned mean current (with a floor), optionally escalated.
    fn set_move_bounds(&mut self, v: usize, floor_ma: u16, repetition: u8) {
        // at most 0xFFFF
        let mean = self.mots[v].meancurrent.min(0xFFFF) as u16;
        let high = end_stop_bound(mean, self.currentbound_high_fac, floor_ma);
        let low = end_stop_bound(mean, self.currentbound_low_fac, floor_ma);
        self.move_bound_high = escalated_bound(high, repetition, &self.calib_escalation);
        self.move_bound_low = -escalated_bound(low, repetition, &self.calib_escalation);
    }

    /// Prepares a position change by `change` % (255: to the end stop).
    fn prepare_normal_move<H: MotorHw>(&mut self, io: &Io<H>, v: usize, dir: u8, change: u8) {
        let mut requested = RUN_TO_END_STOP;
        let mut expected = 0;
        if change == 255 {
            // no early check when the start is not known (reference move) or the valve is failed
            // or blocked
            if self.move_flags & (MOVE_REFERENCE + MOVE_KEEP_STATUS) == 0 {
                let actual = self.mots[v].actual_position;
                expected = if dir == DIR_OPEN {
                    100 - actual.min(100)
                } else {
                    actual
                };
            }
        } else {
            let counts = self.mots[v].scaler.wrapping_mul(u32::from(change));
            // below RUN_TO_END_STOP, fits a u16
            requested = counts.min(u32::from(RUN_TO_END_STOP - 1)) as u16;
        }
        io.pulse
            .target
            .store(u32::from(requested), Ordering::SeqCst);
        self.move_req = MoveRequest {
            dir,
            requested_counts: requested,
            expected_travel_pct: expected,
            learned_travel: self.learned_travel(v, dir),
            partial_early_check: change != 255 && self.move_flags & MOVE_KEEP_STATUS == 0,
        };
        self.move_kind = MOVE_NORMAL;
        self.set_move_bounds(v, MEAN_CURRENT_FLOOR_MA, 0);
    }

    /// Prepares a calibration stroke to the end stop; `full` if it starts at the opposite end
    /// stop.
    fn prepare_learn_stroke<H: MotorHw>(&mut self, io: &Io<H>, v: usize, dir: u8, full: bool) {
        // the largest count: the motor does not stop on pulses
        io.pulse
            .target
            .store(u32::from(RUN_TO_END_STOP), Ordering::SeqCst);
        self.move_req = MoveRequest {
            dir,
            requested_counts: RUN_TO_END_STOP,
            expected_travel_pct: if full { 100 } else { 0 },
            learned_travel: self.learned_travel(v, dir),
            partial_early_check: false,
        };
        self.move_kind = MOVE_LEARN;
        self.set_move_bounds(v, calibration_floor(dir), self.calib_retries);
    }

    /// Prepares a service move: exact pulse count, fixed threshold of maxma.
    fn prepare_service_move<H: MotorHw>(
        &mut self,
        io: &Io<H>,
        v: usize,
        dir: u8,
        counts: u16,
        maxma: u8,
    ) {
        // the motor stops on pulse target + 1
        io.pulse
            .target
            .store(u32::from(counts).wrapping_sub(1), Ordering::SeqCst);
        self.move_req = MoveRequest {
            dir,
            requested_counts: counts,
            expected_travel_pct: 0,
            learned_travel: self.learned_travel(v, dir),
            partial_early_check: false,
        };
        self.move_kind = MOVE_SERVICE;
        self.move_bound_high = i32::from(maxma) * 10;
        self.move_bound_low = -self.move_bound_high;
    }

    /// Records the move that just ended.
    fn finish_move<H: MotorHw>(&mut self, io: &Io<H>, v: usize) {
        let counted = io.pulse.counter.load(Ordering::SeqCst);
        let c = classify_move(&self.move_req, self.motor_stop_cause, counted);
        let stop_current = if self.endstop.trip() != Trip::None {
            self.endstop.trip_current()
        } else {
            self.last_turning_current
        };
        self.move_profile.finish(counted, stop_current);
        let duration = io.hw.millis().wrapping_sub(self.move_start_ms);
        let rec = &mut self.valve_records[v];
        rec.profile = self.move_profile;
        rec.diag.last = make_move_result(
            &self.move_req,
            c.reason,
            counted,
            self.endstop.peak(),
            duration,
        );
        rec.diag.last_early = c.early;
        if c.early && self.move_kind == MOVE_NORMAL {
            rec.diag.early_stops = rec.diag.early_stops.saturating_add(1);
            rec.diag.early_warn = true;
        }
        let mot = &mut self.mots[v];
        if self.move_kind == MOVE_NORMAL {
            // the second early partial stop in a row requests a calibration; the moves of a
            // failed or blocked valve never do (its calibrations come from staln and the
            // automatic retry only)
            if self.move_flags & MOVE_KEEP_STATUS == 0
                && rec
                    .diag
                    .early_run
                    .on_move(c.early && self.move_req.partial_early_check)
            {
                mot.early_learn_due = true;
            }
            mot.move_seq = mot.move_seq.wrapping_add(1);
        }
        // the inrush limit was exceeded (reported, or the move was stopped by it)
        if self.endstop.inrush_seen() {
            mot.fault_reason = ValveFault::InrushTrip as u8;
            mot.trip_seq = mot.trip_seq.wrapping_add(1);
        }
    }

    /// Records a move the motor state machine did not start.
    fn record_refused_move(&mut self, v: usize) {
        let rec = &mut self.valve_records[v];
        rec.profile.reset();
        rec.diag.last = make_move_result(&self.move_req, StopReason::Aborted, 0, 0, 0);
    }

    /// Every way out of a calibration: clears the request (calibration flag, gvlvd bit 7) and its
    /// state.
    fn learn_end(&mut self, v: usize, success: bool) {
        let mot = &mut self.mots[v];
        mot.calibration = false;
        mot.calib_state = CalibState::Idle;
        mot.calib_active = false;
        let diag = &mut self.valve_records[v].diag;
        diag.last_cal_failed = !success;
        if success {
            diag.early_warn = false;
        }
    }

    /// A calibration stopped by sstop: not a failed calibration; the position is lost.
    fn learn_abort(&mut self, v: usize) {
        let mot = &mut self.mots[v];
        mot.calibration = false;
        mot.calib_state = CalibState::Idle;
        mot.calib_active = false;
        mot.status = ST_IDLE;
        mot.needs_reference = true;
    }

    /// The valve lost its contact: no current in a move, a calibration stroke or a service move.
    fn lost_contact(&mut self, v: usize) {
        self.mots[v].status = ST_OPEN_CIRCUIT;
        self.mots[v].recal = true;
    }

    /// A failed calibration stroke (timeout, no end stop, or the inrush limit stopped it).
    fn learn_failed(&mut self, v: usize, fault: ValveFault) {
        self.learn_end(v, false);
        self.mots[v].status = ST_FAILED;
        self.mots[v].fault_reason = fault as u8;
    }

    /// Status after a normal move that did not fail.
    fn move_status(&self) -> u8 {
        if self.move_flags & MOVE_KEEP_STATUS != 0 {
            self.keep_status
        } else {
            ST_IDLE
        }
    }

    /// Position after a service move: counted pulses converted with the learned scaler.
    fn service_move_position(&mut self, v: usize, counted: u32) {
        let mot = &mut self.mots[v];
        let Some(pct) = counted.checked_div(mot.scaler) else {
            return;
        };
        // at most 100
        let delta = pct.min(100) as u8;
        mot.actual_position = if self.move_req.dir == DIR_OPEN {
            position_add(mot.actual_position, delta)
        } else {
            position_sub(mot.actual_position, delta)
        };
    }

    /// End of a normal move (also to an end stop, the failsafe move of a blocked valve and a
    /// reference move); false while the motor runs.
    fn end_normal_move<H: MotorHw>(&mut self, io: &Io<H>, v: usize, result: u8) -> bool {
        if result == M_RES_STOP {
            self.finish_move(io, v);
            // sstop: the position follows the pulses that really turned
            if self.motor_stop_cause == MotorStop::Aborted {
                self.service_move_position(v, io.pulse.counter.load(Ordering::SeqCst));
            } else if self.move_req.dir == DIR_OPEN {
                self.mots[v].actual_position =
                    position_add(self.mots[v].actual_position, self.pos_change);
            } else {
                self.mots[v].actual_position =
                    position_sub(self.mots[v].actual_position, self.pos_change);
            }
            self.mots[v].status = self.move_status();
        } else if result == M_RES_NOCURRENT {
            self.finish_move(io, v);
            self.lost_contact(v);
            self.mots[v].actual_position = self.mots[v].target_position;
        } else if result == M_RES_ENDSTOP {
            self.finish_move(io, v);
            // the inrush limit stopped the motor at its start: the position did not change
            if self.endstop.inrush_trip() {
                self.mots[v].status = ST_FAILED;
            } else {
                // a partial move takes the position from the counted pulses, a move to the end
                // stop 0 / 100 %
                let counted = io.pulse.counter.load(Ordering::SeqCst);
                let mot = &mut self.mots[v];
                mot.actual_position = position_after_end_stop(
                    mot.actual_position,
                    self.move_req.dir,
                    self.move_req.requested_counts,
                    counted,
                    mot.scaler,
                );
                if self.pos_change == 255 {
                    mot.needs_reference = false;
                }
                self.mots[v].status = self.move_status();
            }
        } else if result == M_RES_ERROR {
            // the position is unknown: it stays as it was, app_loop reports the target as rejected
            self.finish_move(io, v);
            self.mots[v].status = ST_FAILED;
            self.mots[v].fault_reason = ValveFault::MoveTimeout as u8;
        } else {
            return false;
        }
        true
    }

    /// Starts a full calibration stroke (the valve stands at the opposite end stop).
    fn start_learn_stroke<H: MotorHw>(&mut self, io: &Io<H>, v: usize, dir: u8) {
        let open = dir == DIR_OPEN;
        self.mots[v].status = if open { ST_OPENING } else { ST_CLOSING };
        self.prepare_learn_stroke(io, v, dir, true);
        self.motorcycle(io, v as i32, if open { CMD_M_OPEN } else { CMD_M_CLOSE });
        self.waittimer = WAIT_TIMER20;
    }

    /// The valve state machine went back to Idle without starting the motor (sstop).
    fn refuse_move(&mut self, v: usize) {
        self.record_refused_move(v);
        self.valvestate = AState::Idle;
    }

    /// Switches the motor off and cancels a soft start that timer_handler0 has not done yet.
    fn motor_halt<H: MotorHw>(&mut self, io: &Io<H>) {
        io.hw.detach();
        io.pulse.turning.store(false, Ordering::SeqCst);
        // before fin: timer_handler0 enables the motor on go && !fin
        self.isr_timer_go = false;
        self.isr_timer_fin = false;
        ena_all_off(io.hw);
    }

    // ------------------------------------------------------------------ valve state machine

    fn tick<H: MotorHw>(&mut self, io: &Io<H>) {
        let ticks = io.flags.valve_loop_ticks.load(Ordering::SeqCst);
        io.flags
            .valve_loop_ticks
            .store(ticks.wrapping_add(1), Ordering::SeqCst);

        if self.waittimer != 0 {
            self.waittimer -= 1;
        }

        // the motor state machine gets a stop command while an operator stop (sstop) is pending
        let runcmd = if self.stop_request { CMD_M_STOP } else { 0 };

        match self.valvestate {
            AState::Init => {
                if self.motorcycle(io, 0, CMD_M_NOTHING) == M_RES_IDLE {
                    self.valvestate = AState::Idle;
                    self.lp.valveindex = NO_VALVE;
                }
            }
            AState::Idle => self.state_idle(io),
            AState::Open1 | AState::Close1 => {
                let v = self.lp.valveindex;
                let open = self.valvestate == AState::Open1;
                let dir = if open { DIR_OPEN } else { DIR_CLOSE };
                if self.stop_request {
                    self.prepare_normal_move(io, v, dir, self.pos_change);
                    self.refuse_move(v);
                } else if self.waittimer == 0 {
                    self.prepare_normal_move(io, v, dir, self.pos_change);
                    let cmd = if open { CMD_M_OPEN } else { CMD_M_CLOSE };
                    let started = if open { M_RES_OPENS } else { M_RES_CLOSES };
                    if self.motorcycle(io, v as i32, cmd) == started {
                        // a failed or blocked valve keeps its status also while it moves
                        if self.move_flags & MOVE_KEEP_STATUS == 0 {
                            self.mots[v].status = if open { ST_OPENING } else { ST_CLOSING };
                        }
                        self.valvestate = if open { AState::Open2 } else { AState::Close2 };
                    } else {
                        self.refuse_move(v);
                    }
                }
            }
            AState::Open2 | AState::Close2 => {
                let v = self.lp.valveindex;
                let result = self.motorcycle(io, v as i32, runcmd);
                if self.end_normal_move(io, v, result) {
                    self.valvestate = AState::Idle;
                }
            }
            AState::Learn1 => {
                let v = self.lp.valveindex;
                if self.stop_request {
                    self.learn_abort(v);
                    self.refuse_move(v);
                } else {
                    // first: closing completely
                    self.prepare_learn_stroke(io, v, DIR_CLOSE, false);
                    self.motorcycle(io, v as i32, CMD_M_CLOSE);
                    self.mots[v].status = ST_CLOSING;
                    self.valvestate = AState::Learn2;
                    self.waittimer = WAIT_TIMER20;
                }
            }
            AState::Learn2 | AState::Learn3 | AState::Learn4 => {
                if self.waittimer == 0 {
                    self.state_learn_stroke(io, runcmd);
                }
            }
            AState::Gap => self.state_gap(io),
            AState::Set => {
                self.lp.valveindex = usize::from(self.valvenr);
                let v = self.lp.valveindex;
                self.mots[v].actual_position = 0;
                if self.mots[v].target_position == 0 {
                    self.learn_end(v, true);
                    self.lp.valveindex = NO_VALVE;
                    self.valvestate = AState::Idle;
                } else {
                    self.valvestate = AState::Set1;
                    psu_on(io.hw);
                    self.waittimer = WAIT_TIMER50;
                    self.lp.psuofftimer = 0;
                    self.pos_change = self.mots[v].target_position;
                }
            }
            AState::Set1 => {
                let v = self.lp.valveindex;
                if self.stop_request {
                    self.prepare_normal_move(io, v, DIR_OPEN, self.pos_change);
                    self.learn_end(v, true);
                    self.refuse_move(v);
                } else if self.waittimer == 0 {
                    self.prepare_normal_move(io, v, DIR_OPEN, self.pos_change);
                    if self.motorcycle(io, v as i32, CMD_M_OPEN) == M_RES_OPENS {
                        self.mots[v].status = ST_OPENING;
                        self.valvestate = AState::Set2;
                    } else {
                        self.record_refused_move(v);
                        self.learn_end(v, true);
                        self.valvestate = AState::Idle;
                    }
                }
            }
            AState::Set2 => {
                // the calibration already succeeded
                let v = self.lp.valveindex;
                let result = self.motorcycle(io, v as i32, runcmd);
                if self.end_normal_move(io, v, result) {
                    self.learn_end(v, true);
                    self.valvestate = AState::Idle;
                }
            }
            AState::Test => {
                if self.waittimer == 0 {
                    self.state_test(io);
                }
            }
            AState::Svc1 => self.state_service_start(io),
            AState::Svc2 => self.state_service_wait(io, runcmd),
            AState::Close => self.valvestate = AState::Idle,
        }

        if let Some(mot) = self.mots.get_mut(self.lp.valveindex) {
            mot.connected = mot.status != ST_OPEN_CIRCUIT;
        }

        // a valve state that never ends stops feeding the watchdog (see loop_system)
        self.valve_stall
            .tick(self.valvestate as u8, self.valvestate == AState::Idle);
        io.flags
            .valve_loop_stalled
            .store(self.valve_stall.stalled(), Ordering::SeqCst);
    }

    fn state_idle<H: MotorHw>(&mut self, io: &Io<H>) {
        self.stop_request = false;
        self.lp.valveindex = usize::from(self.valvenr);
        let v = self.lp.valveindex;
        self.move_flags = 0;
        let command = self.command;
        if command == CMD_A_OPEN
            || command == CMD_A_CLOSE
            || command == CMD_A_OPEN_END
            || command == CMD_A_CLOSE_END
        {
            self.valvestate = if command == CMD_A_OPEN || command == CMD_A_OPEN_END {
                AState::Open1
            } else {
                AState::Close1
            };
            psu_on(io.hw);
            self.waittimer = WAIT_TIMER50;
            self.lp.psuofftimer = 0;
            self.pos_change = if command == CMD_A_OPEN || command == CMD_A_CLOSE {
                self.poschangecmd
            } else {
                255
            };
            self.move_flags = self.moveflagscmd;
            self.keep_status = self.mots[v].status;
        } else if command == CMD_A_LEARN {
            self.valvestate = AState::Learn1;
            psu_on(io.hw);
            self.lp.psuofftimer = 0;
            self.waittimer = WAIT_TIMER50;
            self.calib_retries = 0;
            self.mots[v].calib_retries = 0;
            self.mots[v].calib_active = true;
        } else if command == CMD_A_TEST {
            self.valvestate = AState::Test;
            psu_on(io.hw);
            self.waittimer = WAIT_TIMER100;
            self.lp.psuofftimer = 0;
        } else if command == CMD_A_SERVICE {
            self.valvestate = AState::Svc1;
            psu_on(io.hw);
            self.waittimer = WAIT_TIMER50;
            self.lp.psuofftimer = 0;
            self.lp.svc_move_dir = self.svc_dir;
            self.lp.svc_move_counts = self.svc_counts;
            self.lp.svc_move_maxma = self.svc_maxma;
        } else {
            self.valvestate = AState::Idle;
            // switch off the PSU after some inactive (idle) time
            self.lp.psuofftimer += 1;
            if self.lp.psuofftimer > PSU_OFF_TICKS {
                self.lp.psuofftimer = 0;
                psu_off(io.hw);
                mux_off(io.hw, self.board);
            }
        }

        // the learning counter counts the moves that are not part of a calibration
        if !self.mots[v].calibration
            && (self.valvestate == AState::Open1 || self.valvestate == AState::Close1)
        {
            let valve = &mut self.valves[v];
            if valve.learn_movements != 0 {
                valve.learn_movements -= 1;
            }
            valve.movements = valve.movements.wrapping_add(1);
        }

        // pause the temperature measurement to avoid ADC interference
        if self.valvestate != AState::Idle {
            io.flags.temp_lock.store(true, Ordering::SeqCst);
            self.lp.locktimer = 0;
        } else if self.lp.locktimer < LOCK_TICKS {
            self.lp.locktimer += 1;
        } else {
            io.flags.temp_lock.store(false, Ordering::SeqCst);
        }

        // clear the command for the next call, prevents reevaluating
        self.command = 0;
    }

    /// Learn2 (to the start position), Learn3 (the opening stroke), Learn4 (the closing stroke).
    fn state_learn_stroke<H: MotorHw>(&mut self, io: &Io<H>, runcmd: u8) {
        let v = self.lp.valveindex;
        let temp = self.motorcycle(io, v as i32, runcmd);
        if temp == M_RES_STOP && self.motor_stop_cause == MotorStop::Aborted && self.stop_request {
            // sstop: the calibration ends without a result, the position is lost
            self.finish_move(io, v);
            self.learn_abort(v);
            self.valvestate = AState::Idle;
        } else if temp == M_RES_ENDSTOP && self.endstop.inrush_trip() {
            // never calibration evidence: no counts, no BLOCKS
            self.finish_move(io, v);
            self.learn_failed(v, ValveFault::InrushTrip);
            self.valvestate = AState::Idle;
        } else if temp == M_RES_ENDSTOP && self.valvestate == AState::Learn2 {
            self.finish_move(io, v);
            // second: opening completely and count rotations
            self.lp.gap_dir = DIR_OPEN;
            self.lp.gap_next = AState::Learn3;
        } else if temp == M_RES_ENDSTOP && self.valvestate == AState::Learn3 {
            self.finish_move(io, v);
            self.lp.opening_count = io.pulse.counter.load(Ordering::SeqCst);
            // kept until the pass is accepted
            self.lp.open_mean_ma = self.stroke_mean_ma;
            self.lp.open_mean_samples = self.stroke_mean_samples;
            // third: closing completely and count rotations, threshold from the learned mean
            // current but not below the 1.x closing threshold
            self.lp.gap_dir = DIR_CLOSE;
            self.lp.gap_next = AState::Learn4;
        } else if temp == M_RES_ENDSTOP {
            self.finish_move(io, v);
            self.lp.closing_count = io.pulse.counter.load(Ordering::SeqCst);
            // because the valve was closed completely
            self.mots[v].actual_position = 0;
            self.learn_verdict(io, v);
        } else if temp == M_RES_NOCURRENT {
            self.finish_move(io, v);
            self.lost_contact(v);
            // the target stays: the valve goes there once it is found again
            self.mots[v].actual_position = self.mots[v].target_position;
            self.learn_end(v, false);
            self.valvestate = AState::Idle;
        } else if temp == M_RES_ERROR || temp == M_RES_STOP || temp == M_RES_IDLE {
            // stop: the counter ran out (target 65535) without an end stop, idle: the motor
            // state machine is not running
            if temp != M_RES_IDLE {
                self.finish_move(io, v);
            }
            self.learn_failed(v, ValveFault::StrokeTimeout);
            self.valvestate = AState::Idle;
        }

        // between two strokes (motor stopped at an end stop): a due temperature cycle first
        if (self.valvestate == AState::Learn2 || self.valvestate == AState::Learn3)
            && temp == M_RES_ENDSTOP
        {
            if io.flags.temp_refresh_request.load(Ordering::SeqCst) {
                io.flags.temp_lock.store(false, Ordering::SeqCst);
                self.lp.gaptimer = 0;
                self.valvestate = AState::Gap;
            } else {
                self.start_learn_stroke(io, v, self.lp.gap_dir);
                self.valvestate = self.lp.gap_next;
            }
        }
    }

    /// The counts of a calibration pass after its closing stroke.
    fn learn_verdict<H: MotorHw>(&mut self, io: &Io<H>, v: usize) {
        let opening = self.lp.opening_count;
        let closing = self.lp.closing_count;
        let verdict = evaluate_calibration(
            opening,
            closing,
            self.no_of_min_counts,
            self.calib_retries,
            self.max_calib_retries,
        );
        let mot = &mut self.mots[v];
        if verdict == CalibrationVerdict::Accept {
            // only a successful pass changes the learned values
            mot.meancurrent = u32::from(learn_mean_current(
                mot.meancurrent as u16,
                self.lp.open_mean_ma,
                self.lp.open_mean_samples,
                self.stroke_mean_ma,
                self.stroke_mean_samples,
            ));
            mot.closing_count = closing;
            mot.opening_count = opening;
            mot.deadzone_count = closing as i32 - opening as i32;
            mot.scaler = opening / 100;
            mot.status = ST_IDLE;
            mot.calibrated = true;
            mot.recal = false;
            mot.needs_reference = false;
            mot.calib_failed = false;
            mot.fault_reason = ValveFault::None as u8;
            mot.calib_seq = mot.calib_seq.wrapping_add(1);
            let diag = &mut self.valve_records[v].diag;
            diag.last_cal_failed = false;
            diag.early_warn = false;
            diag.early_run.reset();
            // the movement trigger counts from this calibration
            self.valves[v].movements = 0;
            self.valves[v].learn_movements = self.learning_movements;
            self.valvestate = AState::Set;
            return;
        }
        self.calib_retries = self.calib_retries.wrapping_add(1);
        mot.calib_retries = mot.calib_retries.saturating_add(1);
        if verdict == CalibrationVerdict::Retry {
            self.valvestate = AState::Learn1;
            psu_on(io.hw);
            self.lp.psuofftimer = 0;
            self.waittimer = WAIT_TIMER50;
        } else {
            // BLOCKS sticks: no positioning with the counts of a failed pass
            mot.status = ST_BLOCKED;
            mot.fault_reason = ValveFault::StrokesTooShort as u8;
            mot.calib_failed = true;
            mot.calib_seq = mot.calib_seq.wrapping_add(1);
            self.learn_end(v, false);
            self.lp.valveindex = NO_VALVE;
            self.valvestate = AState::Idle;
        }
    }

    /// Gap: the pause between two calibration strokes for a temperature cycle.
    fn state_gap<H: MotorHw>(&mut self, io: &Io<H>) {
        let v = self.lp.valveindex;
        if self.stop_request {
            io.flags.temp_lock.store(true, Ordering::SeqCst);
            self.learn_abort(v);
            self.valvestate = AState::Idle;
            return;
        }
        let refresh = io.flags.temp_refresh_request.load(Ordering::SeqCst);
        let mut end = !refresh;
        if refresh {
            self.lp.gaptimer += 1;
            end = self.lp.gaptimer >= TIMEOUT_TEMPGAP;
        }
        if end {
            // app_loop ends this period, the next pause comes 60 s later
            if refresh {
                io.flags.temp_gap_timeout.store(true, Ordering::SeqCst);
            }
            io.flags.temp_lock.store(true, Ordering::SeqCst);
            self.start_learn_stroke(io, v, self.lp.gap_dir);
            self.valvestate = self.lp.gap_next;
        }
    }

    /// Test: is a valve connected (the PSU had 2 s).
    fn state_test<H: MotorHw>(&mut self, io: &Io<H>) {
        let v = self.lp.valveindex;
        let temp = self.motorcycle(io, v as i32, CMD_M_TEST);
        if temp == M_RES_TEST {
            return;
        }
        let result = if temp == M_RES_NOCURRENT {
            PresenceResult::Absent
        } else if temp == M_RES_ERROR {
            PresenceResult::Short
        } else {
            PresenceResult::Present
        };
        let mot = &mut self.mots[v];
        let outcome = presence_outcome(result, mot.calibrated, mot.recal);
        mot.status = outcome.status;
        mot.fault_reason = outcome.fault;
        if outcome.needs_reference {
            mot.needs_reference = true;
        }
        // no contact: the valve is tested again at its next target change
        if result == PresenceResult::Absent {
            mot.actual_position = mot.target_position;
        }
        // a short (enforced or only reported)
        if self.test_presence.short_seen() {
            mot.fault_reason = ValveFault::Short as u8;
            mot.trip_seq = mot.trip_seq.wrapping_add(1);
        }
        self.valvestate = AState::Idle;
    }

    /// Svc1: start of a service move.
    fn state_service_start<H: MotorHw>(&mut self, io: &Io<H>) {
        let v = self.lp.valveindex;
        let (dir, counts, maxma) = (
            self.lp.svc_move_dir,
            self.lp.svc_move_counts,
            self.lp.svc_move_maxma,
        );
        if self.stop_request {
            self.prepare_service_move(io, v, dir, counts, maxma);
            self.refuse_move(v);
        } else if self.waittimer == 0 {
            self.prepare_service_move(io, v, dir, counts, maxma);
            self.lp.svc_prev_status = self.mots[v].status;
            let open = dir == DIR_OPEN;
            let cmd = if open { CMD_M_OPEN } else { CMD_M_CLOSE };
            let started = if open { M_RES_OPENS } else { M_RES_CLOSES };
            if self.motorcycle(io, v as i32, cmd) == started {
                self.mots[v].status = if open { ST_OPENING } else { ST_CLOSING };
                self.valvestate = AState::Svc2;
            } else {
                self.record_refused_move(v);
                // nothing moved: app_loop may correct the position again
                self.valves[v].svc_hold = 0;
                self.valvestate = AState::Idle;
            }
        }
    }

    /// Svc2: wait for the end of the service move.
    fn state_service_wait<H: MotorHw>(&mut self, io: &Io<H>, runcmd: u8) {
        let v = self.lp.valveindex;
        let temp = self.motorcycle(io, v as i32, runcmd);
        if temp != M_RES_STOP
            && temp != M_RES_ENDSTOP
            && temp != M_RES_NOCURRENT
            && temp != M_RES_ERROR
        {
            return;
        }
        self.finish_move(io, v);
        // the pulses really turned: the position follows them, also after a threshold stop
        self.service_move_position(v, io.pulse.counter.load(Ordering::SeqCst));
        let prev = self.lp.svc_prev_status;
        let mot = &mut self.mots[v];
        if temp == M_RES_NOCURRENT {
            mot.recal = true;
        }
        // a fault or a pending request stays until a calibration clears it
        if prev != ST_IDLE {
            mot.status = prev;
        } else if temp == M_RES_NOCURRENT {
            mot.status = ST_OPEN_CIRCUIT;
        } else if temp == M_RES_ERROR {
            mot.status = ST_FAILED;
            mot.fault_reason = ValveFault::MoveTimeout as u8;
        } else {
            mot.status = ST_IDLE;
        }
        // the inrush limit stopped the motor at its start (fault 5, set by finish_move)
        if temp == M_RES_ENDSTOP && self.endstop.inrush_trip() {
            mot.status = ST_FAILED;
        }
        self.valvestate = AState::Idle;
    }

    // ------------------------------------------------------------------ motor state machine

    /// `motorcycle`: one step of the motor state machine (valve_loop, every 10 ms).
    fn motorcycle<H: MotorHw>(&mut self, io: &Io<H>, mvalvenr: i32, cmd: u8) -> u8 {
        let mut result: u8;

        // target count reached (flag set by the EXTI handler, which must not run this state
        // machine itself)
        if io.pulse.stop_request.load(Ordering::SeqCst) {
            io.pulse.stop_request.store(false, Ordering::SeqCst);
            if self.mc.motorstate == MState::TurnOn || self.mc.motorstate == MState::Turning {
                self.mc.motorstate = MState::Stop;
                self.motor_stop_cause = MotorStop::CountReached;
            }
        }

        match self.mc.motorstate {
            MState::Init => {
                mux_off(io.hw, self.board);
                ena_all_off(io.hw);
                io.hw.set(Out::Dir, false);
                self.mc.motorstate = MState::Idle;
                io.pulse.turning.store(false, Ordering::SeqCst);
                result = M_RES_INIT;
            }
            MState::Idle => {
                (self.mc.motorstate, result) = if cmd == CMD_M_OPEN {
                    (MState::Open, M_RES_OPENS)
                } else if cmd == CMD_M_CLOSE {
                    (MState::Close, M_RES_CLOSES)
                } else if cmd == CMD_M_STOP {
                    (MState::Stop, M_RES_STOP)
                } else if cmd == CMD_M_TEST {
                    (MState::TestPrep, M_RES_TEST)
                } else {
                    (MState::Idle, M_RES_IDLE)
                };
                // correct motor number?
                if self.mc.motorstate != MState::Idle && (mvalvenr < 0 || mvalvenr >= VALVES as i32)
                {
                    self.mc.motorstate = MState::Idle;
                    result = M_RES_IDLE;
                }
            }
            MState::Open | MState::Close => {
                let open = self.mc.motorstate == MState::Open;
                set_motor(
                    io.hw,
                    self.board,
                    mvalvenr,
                    if open { DIR_OPEN } else { DIR_CLOSE },
                );
                self.mc.settlecnt = MUX_SETTLE_TICKS;
                self.mc.settle_next = MState::Start;
                self.mc.settle_result = M_RES_TURNING;
                self.mc.motorstate = MState::Settle;
                result = if open { M_RES_OPENS } else { M_RES_CLOSES };
            }
            MState::Settle => {
                // counts down to 0 and stays there
                self.mc.settlecnt = (self.mc.settlecnt - 1).max(0);
                if self.mc.settlecnt == 0 {
                    self.mc.motorstate = self.mc.settle_next;
                }
                result = self.mc.settle_result;
            }
            MState::Start => {
                self.mc.motorstate = MState::TurnOn;
                self.isr_valvenr = mvalvenr;
                io.pulse.counter.store(0, Ordering::SeqCst);
                self.mc.cyclecnt = 0;
                self.mc.debouncecnt = 0;
                io.pulse.stop_request.store(false, Ordering::SeqCst);
                // may be left over from the previous move
                self.isr_overcurrentevent = false;
                // the soft start hand-shake with timer_handler0 starts from scratch
                self.isr_timer_go = false;
                self.isr_timer_fin = false;
                self.mc.turnoncnt = 0;
                // turning is still false: timer_handler0 does not sample before the motor is on
                let inrush = if io.flags.protect_suspended.load(Ordering::SeqCst) {
                    InrushMode::Off
                } else if io.flags.protect_enforce.load(Ordering::SeqCst) {
                    InrushMode::Enforce
                } else {
                    InrushMode::Report
                };
                self.endstop
                    .arm(self.move_bound_low, self.move_bound_high, inrush);
                self.move_profile.reset();
                self.motor_stop_cause = MotorStop::None;
                self.last_turning_current = 0;
                self.move_start_ms = io.hw.millis();
                result = M_RES_TURNING;
                io.hw.attach();
            }
            MState::TurnOn => {
                io.pulse.turning.store(true, Ordering::SeqCst);
                result = M_RES_TURNING;
                self.isr_timer_go = true;
                self.mc.meancurrent_mem = 0;
                self.mc.meancurrent_cnt = 0;
                self.analog_current_old = 0;
                self.undercurrcnt = 0;
                self.mc.normalcurrcnt = 0;
                if self.isr_timer_fin {
                    // the soft start is done
                    self.mc.motorstate = MState::Turning;
                    self.isr_timer_fin = false;
                    self.isr_timer_go = false;
                } else {
                    // timer_handler0 (TIM1) did not enable the motor
                    self.mc.turnoncnt += 1;
                    if self.mc.turnoncnt > TIMEOUT_TURNON {
                        self.motor_halt(io);
                        self.motor_stop_cause = MotorStop::Aborted;
                        self.mc.motorstate = MState::Idle;
                        result = M_RES_ERROR;
                    }
                }
            }
            MState::Turning => result = self.motor_turning(io, cmd),
            MState::Stop => {
                self.motor_halt(io);
                if self.motor_stop_cause == MotorStop::None {
                    self.motor_stop_cause = MotorStop::Aborted;
                }
                self.mc.motorstate = MState::Idle;
                result = M_RES_STOP;
            }
            MState::Undercurr => {
                self.motor_halt(io);
                self.motor_stop_cause = MotorStop::Undercurrent;
                self.mc.motorstate = MState::Idle;
                result = M_RES_NOCURRENT;
            }
            MState::TestPrep => {
                set_motor(io.hw, self.board, mvalvenr, DIR_OPEN);
                self.mc.settlecnt = MUX_SETTLE_TICKS;
                self.mc.settle_next = MState::TestStart;
                self.mc.settle_result = M_RES_TEST;
                self.mc.motorstate = MState::Settle;
                result = M_RES_TEST;
            }
            MState::TestStart => {
                // may be left over from the previous move
                self.isr_overcurrentevent = false;
                ena_on(io.hw, mvalvenr);
                self.test_presence.start(
                    !io.flags.protect_suspended.load(Ordering::SeqCst),
                    io.flags.protect_enforce.load(Ordering::SeqCst),
                );
                self.analog_current_old = 0;
                self.analog_current = 0;
                self.mc.motorstate = MState::Test;
                result = M_RES_TEST;
            }
            MState::Test => {
                // open circuit, motor present or short, from the filtered test current
                result = match self.test_presence.sample(self.analog_current) {
                    PresenceResult::Absent => M_RES_NOCURRENT,
                    PresenceResult::Present => M_RES_OPENS,
                    PresenceResult::Short => M_RES_ERROR,
                    PresenceResult::Pending => M_RES_TEST,
                };
                if result != M_RES_TEST {
                    self.mc.motorstate = MState::Idle;
                    ena_all_off(io.hw);
                }
            }
        }
        result
    }

    /// M_TURNING: end stop, undercurrent, timeout and the mean current of the stroke.
    fn motor_turning<H: MotorHw>(&mut self, io: &Io<H>, cmd: u8) -> u8 {
        let mut result = M_RES_TURNING;
        if cmd == CMD_M_STOP {
            self.mc.motorstate = MState::Stop;
        }
        self.mc.cyclecnt += 1;
        // counts up to 255 and stays there
        self.mc.debouncecnt = (self.mc.debouncecnt + 1).min(255);
        self.last_turning_current = self.current_ma;
        self.move_profile.add(
            io.pulse.counter.load(Ordering::SeqCst),
            self.last_turning_current,
        );

        if self.mc.debouncecnt > 3 {
            if self.isr_overcurrentevent {
                // end stop or current limit (timer_handler0 already switched the motor off)
                self.motor_halt(io);
                self.mc.motorstate = MState::Idle;
                result = M_RES_ENDSTOP;
                self.isr_overcurrentevent = false;
                self.motor_stop_cause = if self.endstop.trip() == Trip::Bound {
                    MotorStop::EndStop
                } else {
                    MotorStop::SafetyOvercurrent
                };
                self.stroke_mean_ma =
                    stroke_mean_current(self.mc.meancurrent_mem, self.mc.meancurrent_cnt);
                self.stroke_mean_samples = self.mc.meancurrent_cnt;
            } else if self.current_ma < THRESHOLD_UNDERCURRENT
                && self.current_ma > -THRESHOLD_UNDERCURRENT
            {
                // under current detection
                self.undercurrcnt += 1;
                if self.undercurrcnt > TIMEOUT_UNDERCURRENT {
                    self.mc.motorstate = MState::Undercurr;
                }
            } else {
                // normal turning
                self.mc.normalcurrcnt += 1;
                if self.mc.normalcurrcnt > TIMEOUT_NORMALCURRENT {
                    self.motor_halt(io);
                    self.motor_stop_cause = MotorStop::Timeout;
                    self.mc.motorstate = MState::Idle;
                    result = M_RES_ERROR;
                }
            }
        }

        if self.mc.debouncecnt > 50 && self.mc.cyclecnt > 50 {
            self.mc.cyclecnt = 0;
            self.mc.meancurrent_mem += self.current_ma;
            self.mc.meancurrent_cnt = self.mc.meancurrent_cnt.saturating_add(1);
        }
        result
    }
}

/// The array index of a valve number below 12.
fn valve_index(valve: u32) -> Option<usize> {
    let v = usize::try_from(valve).ok()?;
    (v < VALVES).then_some(v)
}

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
#[cfg(test)]
mod tests_v3;
