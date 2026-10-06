//! Fake time: the monotonic clock of a boot and the wall clock with POSIX TZ rules.

use std::sync::{Arc, Mutex};

use super::lock;
use crate::port::{Clock, LocalTime, WallClock};

/// Payload of the panic that ends a task loop after [`FakeClock::stop_after_sleeps`] (the C++
/// `fakes::YieldLimit`).
#[derive(Debug)]
pub(crate) struct YieldLimit;

type SleepHook = Box<dyn FnMut(u32) + Send>;

#[derive(Default)]
struct ClockState {
    us: u64,
    sleeps: Vec<u32>,
    sleeps_left: Option<u64>,
}

/// Monotonic fake clock since boot. `now_ms` returns the low 32 bits of the milliseconds
/// (`set_ms(0xFFFF_F000)` tests the wrap); `sleep_ms` advances it and records the delay.
#[derive(Clone, Default)]
pub(crate) struct FakeClock {
    state: Arc<Mutex<ClockState>>,
    hook: Arc<Mutex<Option<SleepHook>>>,
}

impl FakeClock {
    /// Milliseconds since boot (not wrapped).
    pub(crate) fn ms(&self) -> u64 {
        lock(&self.state).us / 1000
    }
    /// Microseconds since boot.
    pub(crate) fn us(&self) -> u64 {
        lock(&self.state).us
    }
    /// Sets the time since boot.
    pub(crate) fn set_ms(&self, ms: u64) {
        lock(&self.state).us = ms * 1000;
    }
    /// Advances the time.
    pub(crate) fn advance_ms(&self, ms: u64) {
        lock(&self.state).us += ms * 1000;
    }
    /// Advances the time in microseconds.
    pub(crate) fn advance_us(&self, us: u64) {
        lock(&self.state).us += us;
    }
    /// Every `sleep_ms` argument so far.
    pub(crate) fn sleeps(&self) -> Vec<u32> {
        lock(&self.state).sleeps.clone()
    }
    /// The `n`-th following `sleep_ms` (1 = the next) panics with [`YieldLimit`] after the clock
    /// advanced: it ends a task loop under test.
    pub(crate) fn stop_after_sleeps(&self, n: u64) {
        lock(&self.state).sleeps_left = Some(n);
    }
    /// Runs `hook` after every `sleep_ms` (with its argument), e.g. a peer that acts meanwhile.
    pub(crate) fn on_sleep(&self, hook: impl FnMut(u32) + Send + 'static) {
        *lock(&self.hook) = Some(Box::new(hook));
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> u32 {
        (self.ms() & 0xFFFF_FFFF) as u32
    }
    fn uptime_s(&self) -> u32 {
        (lock(&self.state).us / 1_000_000) as u32
    }
    fn sleep_ms(&self, ms: u32) {
        let stop = {
            let mut s = lock(&self.state);
            s.us += u64::from(ms) * 1000;
            s.sleeps.push(ms);
            match s.sleeps_left {
                Some(n) if n <= 1 => {
                    s.sleeps_left = None;
                    true
                }
                Some(n) => {
                    s.sleeps_left = Some(n - 1);
                    false
                }
                None => false,
            }
        };
        if let Some(h) = lock(&self.hook).as_mut() {
            h(ms);
        }
        if stop {
            std::panic::panic_any(YieldLimit);
        }
    }
}

// ---------------------------------------------------------------- wall clock

/// One end of the DST period of a POSIX TZ rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuleDay {
    /// `Jn`: day 1..365, February 29 never counted.
    Julian1(u16),
    /// `n`: day 0..365, February 29 counted.
    Julian0(u16),
    /// `Mm.w.d`: day `d` (0 = Sunday) of week `w` (5 = last) of month `m`.
    Month(u8, u8, u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Transition {
    day: RuleDay,
    /// Seconds after local midnight (standard time for the start, daylight time for the end).
    time: i64,
}

/// A parsed POSIX TZ string: offsets in seconds east of UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TzRule {
    std_east: i64,
    dst: Option<(i64, Transition, Transition)>,
}

impl TzRule {
    const UTC: TzRule = TzRule {
        std_east: 0,
        dst: None,
    };
}

struct Cursor<'a> {
    s: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn name(&mut self) -> Option<()> {
        if self.eat(b'<') {
            while self.peek()? != b'>' {
                self.i += 1;
            }
            self.i += 1;
            return Some(());
        }
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.i += 1;
        }
        (self.i - start >= 3).then_some(())
    }
    fn number(&mut self) -> Option<i64> {
        let start = self.i;
        let mut v: i64 = 0;
        while let Some(c) = self.peek().filter(u8::is_ascii_digit) {
            v = v * 10 + i64::from(c - b'0');
            self.i += 1;
        }
        (self.i > start).then_some(v)
    }
    /// `[+-]hh[:mm[:ss]]` in seconds.
    fn hms(&mut self) -> Option<i64> {
        let sign = if self.eat(b'-') {
            -1
        } else {
            self.eat(b'+');
            1
        };
        let mut secs = self.number()? * 3600;
        if self.eat(b':') {
            secs += self.number()? * 60;
            if self.eat(b':') {
                secs += self.number()?;
            }
        }
        Some(sign * secs)
    }
    fn transition(&mut self) -> Option<Transition> {
        let day = if self.eat(b'J') {
            RuleDay::Julian1(u16::try_from(self.number()?).ok()?)
        } else if self.eat(b'M') {
            let m = self.number()?;
            self.eat(b'.').then_some(())?;
            let w = self.number()?;
            self.eat(b'.').then_some(())?;
            let d = self.number()?;
            if !(1..=12).contains(&m) || !(1..=5).contains(&w) || d > 6 {
                return None;
            }
            RuleDay::Month(m as u8, w as u8, d as u8)
        } else {
            RuleDay::Julian0(u16::try_from(self.number()?).ok()?)
        };
        let time = if self.eat(b'/') { self.hms()? } else { 7200 };
        Some(Transition { day, time })
    }
}

/// Parses a POSIX TZ string; `None` when it is not one (the fake then uses UTC, as glibc does).
fn parse_tz(posix: &str) -> Option<TzRule> {
    let mut c = Cursor {
        s: posix.as_bytes(),
        i: 0,
    };
    c.name()?;
    let std_east = -c.hms()?;
    if c.peek().is_none() {
        return Some(TzRule {
            std_east,
            dst: None,
        });
    }
    c.name()?;
    let dst_east = match c.peek() {
        Some(b',') | None => std_east + 3600,
        _ => -c.hms()?,
    };
    // glibc's default rule for a DST zone without one: the US rule of 2007
    let (start, end) = if c.eat(b',') {
        let start = c.transition()?;
        c.eat(b',').then_some(())?;
        (start, c.transition()?)
    } else {
        (
            Transition {
                day: RuleDay::Month(3, 2, 0),
                time: 7200,
            },
            Transition {
                day: RuleDay::Month(11, 1, 0),
                time: 7200,
            },
        )
    };
    c.peek().is_none().then_some(())?;
    Some(TzRule {
        std_east,
        dst: Some((dst_east, start, end)),
    })
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Civil date (year, month, day) of days since 1970-01-01.
pub(crate) fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Days since 1970 of the rule day in year `y`.
fn rule_day(y: i64, day: RuleDay) -> i64 {
    let jan1 = days_from_civil(y, 1, 1);
    match day {
        RuleDay::Julian1(n) => {
            let n = i64::from(n);
            jan1 + n - 1 + i64::from(is_leap(y) && n >= 60)
        }
        RuleDay::Julian0(n) => jan1 + i64::from(n),
        RuleDay::Month(m, w, d) => {
            let first = days_from_civil(y, i64::from(m), 1);
            let wday_first = (first + 4).rem_euclid(7); // 1970-01-01 was a Thursday
            let mut day =
                first + (i64::from(d) - wday_first).rem_euclid(7) + 7 * (i64::from(w) - 1);
            let next_month = if m == 12 {
                days_from_civil(y + 1, 1, 1)
            } else {
                days_from_civil(y, i64::from(m) + 1, 1)
            };
            while day >= next_month {
                day -= 7;
            }
            day
        }
    }
}

impl TzRule {
    /// Offset east of UTC in force at `epoch`.
    fn offset_at(&self, epoch: i64) -> i64 {
        let Some((dst_east, start, end)) = self.dst else {
            return self.std_east;
        };
        let (y, _, _) = civil_from_days((epoch + self.std_east).div_euclid(86_400));
        let start_utc = rule_day(y, start.day) * 86_400 + start.time - self.std_east;
        let end_utc = rule_day(y, end.day) * 86_400 + end.time - dst_east;
        let dst = if start_utc < end_utc {
            epoch >= start_utc && epoch < end_utc
        } else {
            !(epoch >= end_utc && epoch < start_utc)
        };
        if dst {
            dst_east
        } else {
            self.std_east
        }
    }
}

/// Converts `epoch` with `rule` into local civil time.
fn local(rule: &TzRule, epoch: i64) -> LocalTime {
    let t = epoch + rule.offset_at(epoch);
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    LocalTime {
        valid: true,
        year: y as u16,
        month: m as u8,
        mday: d as u8,
        wday: (days + 4).rem_euclid(7) as u8,
        hour: (secs / 3600) as u8,
        minute: (secs / 60 % 60) as u8,
        second: (secs % 60) as u8,
        epoch,
    }
}

struct WallState {
    /// Wall-clock microseconds at boot (0 = 1970-01-01, an ESP32 before SNTP).
    base_us: i64,
    rule: TzRule,
    zones: Vec<String>,
}

/// Wall clock of a boot: `epoch()` advances with the [`FakeClock`]; before [`FakeWall::set`] it
/// is 1970-01-01 plus the time since boot. Local time follows the POSIX TZ string set last
/// (UTC before), with the rules glibc applies.
#[derive(Clone)]
pub(crate) struct FakeWall {
    clock: FakeClock,
    state: Arc<Mutex<WallState>>,
}

impl FakeWall {
    /// The wall clock of the boot that runs `clock`.
    pub(crate) fn new(clock: FakeClock) -> Self {
        FakeWall {
            clock,
            state: Arc::new(Mutex::new(WallState {
                base_us: 0,
                rule: TzRule::UTC,
                zones: Vec::new(),
            })),
        }
    }
    /// The wall clock reads `epoch` seconds now (SNTP set it).
    pub(crate) fn set(&self, epoch: i64) {
        let now = self.clock.us() as i64;
        lock(&self.state).base_us = epoch * 1_000_000 - now;
    }
    /// Wall-clock microseconds now.
    pub(crate) fn now_us(&self) -> i64 {
        lock(&self.state).base_us + self.clock.us() as i64
    }
    /// Every TZ string set so far.
    pub(crate) fn zones(&self) -> Vec<String> {
        lock(&self.state).zones.clone()
    }
}

impl WallClock for FakeWall {
    fn epoch(&self) -> i64 {
        self.now_us().div_euclid(1_000_000)
    }
    fn set_time_zone(&self, posix: &str) {
        let mut s = lock(&self.state);
        s.rule = parse_tz(posix).unwrap_or(TzRule::UTC);
        s.zones.push(posix.to_string());
    }
    fn local_time(&self, epoch: i64) -> Option<LocalTime> {
        let rule = lock(&self.state).rule;
        Some(local(&rule, epoch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CET: &str = "CET-1CEST,M3.5.0,M10.5.0/3";

    fn at(rule: &str, epoch: i64) -> LocalTime {
        local(&parse_tz(rule).unwrap(), epoch)
    }

    #[test]
    fn civil_round_trip() {
        for (y, m, d) in [
            (1970, 1, 1),
            (2000, 2, 29),
            (2026, 10, 25),
            (2100, 3, 1),
            (1969, 12, 31),
        ] {
            assert_eq!(civil_from_days(days_from_civil(y, m, d)), (y, m, d));
        }
        assert_eq!(days_from_civil(2026, 1, 1), 20_454);
    }

    #[test]
    fn cet_switches_at_the_last_sundays() {
        // 2026-03-29 01:00 UTC: 02:00 CET becomes 03:00 CEST
        let t = at(CET, 1_774_746_000 - 1);
        assert_eq!((t.hour, t.minute, t.second), (1, 59, 59));
        let t = at(CET, 1_774_746_000);
        assert_eq!((t.month, t.mday, t.hour, t.wday), (3, 29, 3, 0));
        // 2026-10-25 01:00 UTC: 03:00 CEST becomes 02:00 CET
        let t = at(CET, 1_792_890_000 - 1);
        assert_eq!((t.month, t.mday, t.hour, t.minute), (10, 25, 2, 59));
        let t = at(CET, 1_792_890_000);
        assert_eq!((t.hour, t.minute), (2, 0));
        let t = at(CET, 1_767_225_600); // 2026-01-01T00:00:00Z
        assert_eq!(
            (t.year, t.month, t.mday, t.hour, t.wday),
            (2026, 1, 1, 1, 4)
        );
    }

    #[test]
    fn southern_rules_and_formats() {
        // Sydney: DST from the first Sunday of October to the first Sunday of April
        let syd = "AEST-10AEDT,M10.1.0,M4.1.0/3";
        assert_eq!(at(syd, 1_767_225_600).hour, 11); // January: AEDT
        assert_eq!(at(syd, 1_782_864_000).hour, 10); // July: AEST
        assert_eq!(at("<+0330>-3:30", 0).hour, 3);
        assert_eq!(at("<+0330>-3:30", 0).minute, 30);
        assert_eq!(at("EST5", 0).hour, 19);
        assert_eq!(at("EST5EDT", 1_782_864_000).hour, 20); // US default rule: July is EDT
        assert_eq!(at("XXX0YYY,J60/0,J300/0", 1_772_323_200).hour, 1); // 2026-03-01 (J60)
        assert_eq!(at("XXX0YYY,59/0,300/0", 1_772_323_200).hour, 1);
        assert!(parse_tz("C").is_none());
        assert!(parse_tz("CET-1CEST,M13.5.0,M10.5.0").is_none());
        assert!(parse_tz("CET-1CEST,M3.5.0").is_none());
        assert!(parse_tz("CET-1CEST,M3.5.0,M10.5.0x").is_none());
    }

    #[test]
    fn wall_follows_the_clock() {
        let clock = FakeClock::default();
        let wall = FakeWall::new(clock.clone());
        clock.advance_ms(2500);
        assert_eq!(wall.epoch(), 2);
        wall.set(1_767_225_600);
        clock.advance_ms(1500);
        assert_eq!(wall.epoch(), 1_767_225_601);
        assert_eq!(wall.now_us() % 1_000_000, 500_000);
        wall.set_time_zone(CET);
        wall.set_time_zone("garbage");
        assert_eq!(wall.local_time(0).unwrap().hour, 0);
        assert_eq!(wall.zones(), vec![CET.to_string(), "garbage".to_string()]);
    }
}
