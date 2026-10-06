//! Reply formatters of the protocol 3 commands (see PROTOCOL_V2.md). Like the v2
//! replies: one line of space separated integers without a trailing space, the
//! caller appends CR LF, each formatter writes all or nothing. The ok/err
//! replies (slcfg, sfspo, sstop, ssafe, slhbt err) use format_result() and
//! format_indexed_result() of replies_v2. A later firmware may append
//! values to these replies; gvlvx and gstat never change.

use crate::buf_writer::{BufWriter, Storage};
use crate::legacy_layout::VALVE_COUNT;
use crate::replies_v2::{
    append_stat_fields, append_valve_ext_fields, ReplyLine, StatReply, ValveExtReply,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValveExtV3Reply {
    /// values 1..19, the same as gvlvx
    pub base: ValveExtReply,
    /// VLV_FLAG_* (valve_codes)
    pub flags: u16,
    /// ValveFault
    pub fault: u8,
    /// 0..100, FAILSAFE_HOLD
    pub failsafe_pct: u8,
    /// drive target 0..100
    pub drive: u8,
    /// seconds to the next automatic retry, 0 = none
    pub retry_s: u32,
    /// automatic retries since the fault began
    pub retries: u8,
}

/// "gvlvy" + 25 numbers of up to 11 characters, separated by spaces.
pub const VALVE_EXT_V3_REPLY_MAX_LEN: usize = 5 + 25 * 12;

pub fn format_valve_ext_v3<B: Storage>(out: &mut BufWriter<B>, r: &ValveExtV3Reply) -> bool {
    let mut line = ReplyLine::new(out, b"gvlvy");
    append_valve_ext_fields(&mut line, &r.base)
        .u(u32::from(r.flags))
        .u(u32::from(r.fault))
        .u(u32::from(r.failsafe_pct))
        .u(u32::from(r.drive))
        .u(r.retry_s)
        .u(u32::from(r.retries))
        .done()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatV3Reply {
    /// values 1..6, the same as gstat
    pub base: StatReply,
    /// LeaseState
    pub lease: u8,
    /// while running, else 0
    pub lease_remain_s: u32,
    /// 1 while a lease command arrived within LEASE_CLIENT_IDLE_S
    pub lease_client: u8,
    /// 0, 5..1440
    pub lease_timeout_min: u16,
    /// bit v: valve v is at its failsafe position because the lease expired
    pub failsafe_mask: u16,
    pub safe_mode: u8,
    /// watchdog resets in the current window
    pub wdg_resets: u8,
    /// USART errors and dropped bytes since start-up
    pub uart_ore: u32,
    pub uart_fe: u32,
    pub uart_ne: u32,
    pub rx_dropped: u32,
    /// CFG_* of the last EEPROM load (config_store)
    pub cfg_flags: u8,
    /// EEPROM loads since start-up that repaired or defaulted a block
    pub cfg_events: u32,
    /// successful EEPROM write steps since start-up
    pub eep_writes: u32,
    /// seconds since the last complete temperature cycle
    pub temp_age_s: u32,
    /// seconds since the last 1-Wire enumeration
    pub ow_scan_age_s: u32,
    /// SYS_FLAG_* (valve_codes)
    pub sys_flags: u8,
}

/// "gstax" + 23 numbers of up to 11 characters, separated by spaces.
pub const STAT_V3_REPLY_MAX_LEN: usize = 5 + 23 * 12;

pub fn format_stat_v3<B: Storage>(out: &mut BufWriter<B>, r: &StatV3Reply) -> bool {
    let mut line = ReplyLine::new(out, b"gstax");
    append_stat_fields(&mut line, &r.base)
        .u(u32::from(r.lease))
        .u(r.lease_remain_s)
        .u(u32::from(r.lease_client))
        .u(u32::from(r.lease_timeout_min))
        .u(u32::from(r.failsafe_mask))
        .u(u32::from(r.safe_mode))
        .u(u32::from(r.wdg_resets))
        .u(r.uart_ore)
        .u(r.uart_fe)
        .u(r.uart_ne)
        .u(r.rx_dropped)
        .u(u32::from(r.cfg_flags))
        .u(r.cfg_events)
        .u(r.eep_writes)
        .u(r.temp_age_s)
        .u(r.ow_scan_age_s)
        .u(u32::from(r.sys_flags))
        .done()
}

/// "slhbt <lease> <remainS>"
pub fn format_heartbeat<B: Storage>(out: &mut BufWriter<B>, lease: u8, remain_s: u32) -> bool {
    ReplyLine::new(out, b"slhbt")
        .u(u32::from(lease))
        .u(remain_s)
        .done()
}

/// "glcfg <timeoutMin> <fs0> ... <fs11>"
pub fn format_lease_config<B: Storage>(
    out: &mut BufWriter<B>,
    timeout_min: u16,
    failsafe_pct: &[u8; VALVE_COUNT as usize],
) -> bool {
    let mut line = ReplyLine::new(out, b"glcfg");
    line.u(u32::from(timeout_min));
    for &pct in failsafe_pct {
        line.u(u32::from(pct));
    }
    line.done()
}

/// "gtlnt <seconds>"
pub fn format_learn_time<B: Storage>(out: &mut BufWriter<B>, seconds: u32) -> bool {
    ReplyLine::new(out, b"gtlnt").u(seconds).done()
}

#[cfg(test)]
mod tests;
