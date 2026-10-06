//! Validation of settings received over the UART protocol. Hardware-free.

/// learn-after-movements: 0 disables the movement trigger, otherwise
/// MIN_LEARN_MOVEMENTS..MAX_LEARN_MOVEMENTS. The same range applies to `stlnm`
/// and to the value loaded from the EEPROM (16 bit field, 0xFFFF = erased).
pub const MIN_LEARN_MOVEMENTS: u16 = 50;
pub const MAX_LEARN_MOVEMENTS: u16 = 65534;
pub const LEARN_MOVEMENTS_DEFAULT: u16 = 2000;

/// learn-after-time (`stlnt`) in seconds, 0 disables the time trigger;
/// the default is one week (== LEARN_AFTER_TIME_DEFAULT of the glue)
pub const LEARN_TIME_DEFAULT_S: u32 = 7 * 24 * 3600;
/// a stored learn time 0 (the ESP runs its own schedule) is only honoured while
/// a lease client was seen within this time: a rolled-back ESP without a
/// schedule never leaves the valves without the time trigger
pub const LEARN_TIME_CLIENT_WINDOW_S: u32 = 24 * 3600;

/// learn time the countdown runs with: stored, or the default for a stored 0
/// without a recent lease client
pub fn effective_learn_time(stored_s: u32, client_seen: bool) -> u32 {
    if stored_s == 0 && !client_seen {
        LEARN_TIME_DEFAULT_S
    } else {
        stored_s
    }
}

/// One countdown step: true when remaining <= elapsed_s (the caller reloads),
/// otherwise remaining -= elapsed_s.
pub fn countdown(remaining: &mut u32, elapsed_s: u32) -> bool {
    if *remaining <= elapsed_s {
        return true;
    }
    *remaining -= elapsed_s;
    false
}

/// `stlnm` takes any 32-bit count (the ESP stores it as uint32) and 1.x always
/// replied, so a value outside the range is moved to its nearest bound
/// instead of being rejected: 1..49 -> 50, above 65534 -> 65534 (never wrapped).
pub fn learn_movements_from_request(movements: u32) -> u16 {
    if movements == 0 {
        return 0;
    }
    // the clamped value fits 16 bits
    movements.clamp(
        u32::from(MIN_LEARN_MOVEMENTS),
        u32::from(MAX_LEARN_MOVEMENTS),
    ) as u16
}

/// Stored value at start-up: anything `stlnm` can store is kept, anything else
/// (erased EEPROM, 1..49 written by 1.x) loads the default.
pub fn sanitize_learn_movements(stored: u16) -> u16 {
    if stored == 0 || (MIN_LEARN_MOVEMENTS..=MAX_LEARN_MOVEMENTS).contains(&stored) {
        stored
    } else {
        LEARN_MOVEMENTS_DEFAULT
    }
}
