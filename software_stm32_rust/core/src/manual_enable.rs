//! Limits of a motor output switched on from the debug terminal (sena): it is switched off after
//! MANUAL_ENABLE_MAX_MS or as soon as the filtered motor current exceeds the safety limit.
//! Hardware-free.

pub const MANUAL_ENABLE_MAX_MS: u32 = 2000;
/// 0.1 mA: the 60 mA safety limit
pub const MANUAL_ENABLE_LIMIT: i32 = 600;

/// true when the output enabled at start_ms must be switched off; filtered_current in 0.1 mA,
/// either sign; millis() differences, so the wrap does not matter
pub fn manual_enable_expired(start_ms: u32, now_ms: u32, filtered_current: i32) -> bool {
    now_ms.wrapping_sub(start_ms) >= MANUAL_ENABLE_MAX_MS
        || !(-MANUAL_ENABLE_LIMIT..=MANUAL_ENABLE_LIMIT).contains(&filtered_current)
}

#[cfg(test)]
mod tests;
