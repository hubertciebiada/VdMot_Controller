//! The application stage of the firmware (port of the logic of `software_stm32/src/main.cpp`):
//! the set-up order of `setup_system()` and the three branches of `loop_system()` with the
//! watchdog feed. The entry (`setup()`/`loop()` with the boot window) is the firmware's: the boot
//! stage (`vdm-stm-boot`) runs first, then the firmware starts the independent watchdog and calls
//! [`MainLoop::run`] (design §5.2, §5.4).

use core::sync::atomic::Ordering;

use vdm_stm_core::system_stats::BootReason;

use crate::hal::{Clock, ControlTimer, In, Out, Pins, Watchdog};
use crate::motor::IsrFlags;

/// TIM2 runs the valve state machine every 10 ms.
const VALVE_TIMER_US: u32 = 10_000;

/// Calls of main.cpp into the glue modules, and the set-up steps of the firmware that have no
/// HAL trait. The supertraits are the hardware of the main loop: the LED and the button, the
/// clock, the independent watchdog and TIM2 (`attach_interval` starts the valve state machine).
pub trait MainLoopEnv: Pins + Clock + Watchdog + ControlTimer {
    // sysstat.cpp
    fn sysstat_boot_reason(&mut self) -> BootReason;
    fn sysstat_safe_mode(&mut self) -> bool;
    fn sysstat_loop(&mut self);
    fn sysstat_uptime_s(&mut self) -> u32;
    // i2c_bus.cpp and Wire
    fn i2c_bus_recover(&mut self);
    /// `Wire.setSDA(PB7)`, `Wire.setSCL(PB6)`, `Wire.begin()`: the firmware creates the I2C1
    /// driver
    fn wire_begin(&mut self);
    // terminal.cpp
    fn terminal_init(&mut self) -> i16;
    fn terminal_serve(&mut self) -> i16;
    fn terminal_supervise(&mut self);
    // communication.cpp
    fn communication_setup(&mut self);
    fn communication_loop(&mut self) -> i16;
    // eeprom.cpp
    fn eepromsetup(&mut self) -> i16;
    /// `eeprom_read_layout(&eep_content)`
    fn eeprom_read_layout(&mut self) -> i16;
    fn eepromloop(&mut self) -> i16;
    // owDevices.cpp
    fn temperature_setup(&mut self);
    fn temperature_loop(&mut self);
    // app.cpp
    fn app_setup(&mut self) -> i16;
    fn app_restore(&mut self);
    fn app_loop(&mut self) -> i16;
    fn app_warm_save(&mut self);
    fn app_1s_tick(&mut self, elapsed_s: u32);
    fn app_10s_loop(&mut self, elapsed_s: u32) -> u8;
    // motor.cpp
    fn valve_setup(&mut self) -> u8;
    /// `COMM_DBG` (USART6), CR LF included
    fn debug(&mut self, text: &[u8]);
}

/// The statics of loop_system().
#[derive(Debug, Default)]
pub struct MainLoop {
    time10s: i32,
    loop_10ms: u32,
    loop_100ms: u32,
    loop_1000ms: u32,
    buttontest: bool,
    led_timer: u8,
    last_valve_ticks: u32,
    /// uptime (s) of the last app_1s_tick()
    last_1s_tick: u32,
    /// uptime (s) of the last app_10s_loop()
    last_10s_loop: u32,
}

impl MainLoop {
    pub const fn new() -> Self {
        MainLoop {
            time10s: 0,
            loop_10ms: 0,
            loop_100ms: 0,
            loop_1000ms: 0,
            buttontest: false,
            led_timer: 0,
            last_valve_ticks: 0,
            last_1s_tick: 0,
            last_10s_loop: 0,
        }
    }

    /// `setup_system()` after the boot window: the modules in the C++ order. The firmware started
    /// the independent watchdog before (8 s; it cannot be stopped, the ESP may only flash the STM
    /// in the boot window); this feeds it between the long blocking steps (EEPROM read, 1-Wire
    /// enumeration). The pin modes (PSU open drain, LED, button, analog inputs) and the 12-bit ADC
    /// are firmware set-up.
    pub fn setup_system(&mut self, env: &mut impl MainLoopEnv) {
        let watchdog_reset = env.sysstat_boot_reason() == BootReason::IndependentWatchdog;

        env.i2c_bus_recover();
        env.wire_begin();

        // high to disable the valve PSU
        env.set(Out::PsuEna, true);

        // terminal for debug
        env.terminal_init();
        if watchdog_reset {
            env.debug(b"reset by watchdog\r\n");
        }
        if env.sysstat_safe_mode() {
            env.debug(b"safe mode\r\n");
        }

        // serial communication to the ESP32
        env.communication_setup();
        env.delay_ms(500);

        // EEPROM
        env.reload();
        env.eepromsetup();
        env.eeprom_read_layout();

        // 1-Wire temperature sensors
        env.reload();
        env.temperature_setup();
        env.reload();

        // valve app
        env.app_setup();
        env.valve_setup();
        // the valve state kept across a warm reset, before the valve timer runs
        env.app_restore();

        // hardware timer of the valve state machine (the result is only printed by motDebug)
        env.attach_interval(VALVE_TIMER_US);
    }

    /// `loop_system()`: one pass of the main loop.
    pub fn loop_system(&mut self, flags: &IsrFlags, env: &mut impl MainLoopEnv) {
        env.sysstat_loop();

        // 1000 ms loop
        if env.millis().wrapping_sub(self.loop_1000ms) > 1000 {
            self.loop_1000ms = env.millis();

            // the branches run a little later than their period: the countdowns get the real
            // elapsed seconds
            let uptime = env.sysstat_uptime_s();
            env.app_1s_tick(uptime.wrapping_sub(self.last_1s_tick));
            self.last_1s_tick = uptime;

            self.time10s += 1;
            if self.time10s >= 10 {
                self.time10s = 0;
                env.app_10s_loop(uptime.wrapping_sub(self.last_10s_loop));
                self.last_10s_loop = uptime;
            }

            env.eepromloop();
        }

        // 100 ms loop
        if env.millis().wrapping_sub(self.loop_100ms) > 100 {
            self.loop_100ms = env.millis();
            self.led_timer += 1;
            if self.led_timer == 30 {
                env.set(Out::Led, false);
            }
            if self.led_timer == 31 {
                env.set(Out::Led, true);
                self.led_timer = 0;
            }
            env.terminal_serve();

            // button test
            if env.read(In::Button) && !self.buttontest {
                self.buttontest = true;
                env.debug(b"Button pressed\r\n");
            } else {
                self.buttontest = false;
            }
        }

        // 10 ms loop
        if env.millis().wrapping_sub(self.loop_10ms) > 10 {
            self.loop_10ms = env.millis();

            // the watchdog is fed only while the valve state machine (TIM2) runs and makes
            // progress
            let valve_ticks = flags.valve_loop_ticks.load(Ordering::SeqCst);
            if valve_ticks != self.last_valve_ticks
                && !flags.valve_loop_stalled.load(Ordering::SeqCst)
            {
                self.last_valve_ticks = valve_ticks;
                env.reload();
            }

            env.app_loop();
            env.app_warm_save();
            env.communication_loop();
            env.temperature_loop();
            env.terminal_supervise();
        }
    }

    /// `setup_system()`, then `loop_system()` for ever.
    pub fn run(&mut self, flags: &IsrFlags, env: &mut impl MainLoopEnv) -> ! {
        self.setup_system(env);
        loop {
            self.loop_system(flags, env);
        }
    }
}

#[cfg(test)]
mod stub_env;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
