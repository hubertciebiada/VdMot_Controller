//! Temperature conversions during a long series of valve moves: the valve
//! state machine locks the temperature state machine while a motor runs (ADC
//! interference), so a calibration series of all valves would leave the
//! temperatures unchanged for tens of minutes. At least every
//! TEMP_REFRESH_MS the motors pause (at most TEMP_HOLD_MAX_MS) for one complete
//! conversion cycle. Hardware-free; times are millis() values (wrap-safe).

pub const TEMP_REFRESH_MS: u32 = 60000;
pub const TEMP_HOLD_MAX_MS: u32 = 3000;

#[derive(Clone, Copy, Debug, Default)]
pub struct TempRefresh {
    last_cycle_ms: u32,
    period_start_ms: u32,
    hold_start_ms: u32,
    holding: bool,
}

impl TempRefresh {
    /// the temperature state machine completed a cycle
    pub fn cycle_done(&mut self, now_ms: u32) {
        self.last_cycle_ms = now_ms;
        self.period_start_ms = now_ms;
        self.holding = false;
    }

    /// the last cycle is TEMP_REFRESH_MS or more ago (and no hold of this period timed out)
    pub fn due(&self, now_ms: u32) -> bool {
        now_ms.wrapping_sub(self.period_start_ms) >= TEMP_REFRESH_MS
    }

    /// while due: true until a cycle completes or TEMP_HOLD_MAX_MS passed since the
    /// first call of this hold; a hold that timed out comes again TEMP_REFRESH_MS later
    pub fn hold_commands(&mut self, now_ms: u32) -> bool {
        if !self.due(now_ms) {
            return false;
        }
        if !self.holding {
            self.holding = true;
            self.hold_start_ms = now_ms;
        }
        if now_ms.wrapping_sub(self.hold_start_ms) < TEMP_HOLD_MAX_MS {
            return true;
        }
        // the cycle did not complete in time: go on and try again one period later
        self.hold_timed_out(now_ms);
        false
    }

    /// a hold kept by someone else (the pause between two calibration strokes) timed out: the
    /// next one comes TEMP_REFRESH_MS later
    pub fn hold_timed_out(&mut self, now_ms: u32) {
        self.holding = false;
        self.period_start_ms = now_ms;
    }

    /// seconds since the last complete cycle (since start-up if none)
    pub fn age_s(&self, now_ms: u32) -> u32 {
        now_ms.wrapping_sub(self.last_cycle_ms) / 1000
    }
}
