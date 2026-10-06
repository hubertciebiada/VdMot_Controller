//! Valve status codes, the gvlvy flag and fault codes and the gstax system flags
//! (see PROTOCOL_V2.md). The ESP mirrors the values in vdm/stm_codec.h, so they
//! never change. Hardware-free.

// Valve status (gvlvd field 3, gvlst, gvlvx/gvlvy field 2); bit 7 of the status
// byte is the calibration flag (STATUS_CALIBRATION_BIT in replies_v2).

/// at its target
pub const ST_IDLE: u8 = 1;
pub const ST_OPENING: u8 = 2;
pub const ST_CLOSING: u8 = 3;
/// a move or calibration stroke timed out, the presence test measured a short, the inrush
/// limit tripped
pub const ST_FAILED: u8 = 4;
/// not tested yet: the presence test runs next
pub const ST_UNKNOWN: u8 = 5;
/// the presence test measured no current
pub const ST_OPEN_CIRCUIT: u8 = 6;
/// staop: opens to the end stop
pub const ST_FULL_OPEN: u8 = 7;
/// found by the presence test, calibration pending
pub const ST_PRESENT: u8 = 8;
/// the calibration strokes stayed too short
pub const ST_BLOCKED: u8 = 9;

// gvlvy field 20 `flags`

/// drive = failsafe position because the lease expired
pub const VLV_FLAG_FS_LEASE: u16 = 0x0001;
/// drive = failsafe position because the valve is blocked
pub const VLV_FLAG_FS_BLOCKED: u16 = 0x0002;
/// no valid calibration counts
pub const VLV_FLAG_UNCALIBRATED: u16 = 0x0004;
/// position not referenced: the next move runs to an end stop first
pub const VLV_FLAG_NEEDS_REF: u16 = 0x0008;
/// full calibration required once the valve is present
pub const VLV_FLAG_RECAL: u16 = 0x0010;
/// counts restored from the EEPROM, no calibration since start-up
pub const VLV_FLAG_CAL_RESTORED: u16 = 0x0020;
/// automatic calibration retry scheduled
pub const VLV_FLAG_RETRY: u16 = 0x0040;
/// one early partial stop at the current drive target
pub const VLV_FLAG_EARLY_PENDING: u16 = 0x0080;
/// staop hold: the lease failsafe skips the valve
pub const VLV_FLAG_ASSEMBLY: u16 = 0x0100;
/// left where a service move or sstop ended
pub const VLV_FLAG_SVC_HOLD: u16 = 0x0200;

/// gvlvy field 21 `fault`, cleared by a successful calibration and by a presence
/// test that finds the valve present or absent
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum ValveFault {
    #[default]
    None = 0,
    /// status 4: a move ran 120 s without an end stop
    MoveTimeout = 1,
    /// status 4: a calibration stroke timed out
    StrokeTimeout = 2,
    /// status 4: the presence test measured a short
    Short = 3,
    /// status 9: the calibration strokes stayed too short
    StrokesTooShort = 4,
    /// status 4: the inrush limit tripped at a motor start (never blocks)
    InrushTrip = 5,
}

// gstax field 23 `sysFlags`

/// short and inrush limits off until the next start
pub const SYS_FLAG_PROTECT_SUSPENDED: u8 = 0x01;
