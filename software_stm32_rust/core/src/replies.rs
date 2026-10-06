//! Formatters for UART replies whose length depends on runtime values.
//! Output is byte-identical to protocol v1; the caller appends CR LF.

use crate::buf_writer::{BufWriter, Storage};

/// Fields of the `gvlvd` reply, in wire order after the valve index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValveDataReply {
    pub index: u32,
    pub actual_position: i32,
    pub mean_current: i32,
    /// status | 0x80 while calibrating
    pub status: i32,
    pub temperature1: i32,
    pub temperature2: i32,
    pub movements: i32,
    pub opening_count: i32,
    pub closing_count: i32,
    pub deadzone_count: i32,
    pub calib_retries: i32,
}

/// Worst case: 5-char prefix, 11 numbers of up to 11 characters, 12 spaces.
pub const VALVE_DATA_REPLY_MAX_LEN: usize = 5 + 11 * 11 + 12;

/// "<prefix> idx actual mean status t1 t2 movements open close dead retries "
/// Writes nothing and returns false if `out` has too little room.
pub fn format_valve_data<B: Storage>(
    out: &mut BufWriter<B>,
    prefix: &[u8],
    r: &ValveDataReply,
) -> bool {
    let start = out.length();
    let fields = [
        r.actual_position,
        r.mean_current,
        r.status,
        r.temperature1,
        r.temperature2,
        r.movements,
        r.opening_count,
        r.closing_count,
        r.deadzone_count,
        r.calib_retries,
    ];
    let mut ok = out.append(prefix) && out.append_char(b' ') && out.append_unsigned(r.index);
    for f in fields {
        ok = ok && out.append_char(b' ') && out.append_signed(f);
    }
    ok = ok && out.append_char(b' ');
    if !ok {
        out.truncate(start);
    }
    ok
}

/// "<prefix> n s0,s1,...,s(n-1) " (for n == 0: "<prefix> 0  "), n = status.len().
/// Writes nothing and returns false if `out` has too little room.
pub fn format_status_list<B: Storage>(
    out: &mut BufWriter<B>,
    prefix: &[u8],
    status: &[u8],
) -> bool {
    let start = out.length();
    let n = u32::try_from(status.len()).unwrap_or(u32::MAX);
    let mut ok = out.append(prefix)
        && out.append_char(b' ')
        && out.append_unsigned(n)
        && out.append_char(b' ');
    for (i, &s) in status.iter().enumerate() {
        ok = ok && (i == 0 || out.append_char(b',')) && out.append_unsigned(u32::from(s));
    }
    ok = ok && out.append_char(b' ');
    if !ok {
        out.truncate(start);
    }
    ok
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_fuzz;
