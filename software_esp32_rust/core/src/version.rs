//! Firmware version strings: this ESP build, and parsing of STM `gvers` versions (port of
//! `vdm/version.h`). Hardware-free.

use core::fmt::Write as _;

use crate::common::{contains_bytes, parse_uint, Text, TextBuf};

/// Longest accepted version string.
const VERSION_MAX_LEN: usize = 31;

/// A parsed version such as "1.4.9_Dev_C2", "2.0.0-revamped_C2", "2.0.0-revamped-dev" or
/// "1.4.12+hc2".
///
/// Grammar (the whole string, 1..31 chars, printable ASCII without spaces):
/// ```text
///   version := major '.' minor '.' patch [suffix] [hw]
///   major/minor/patch := 1..5 decimal digits, value 0..65535
///   hw     := '_' 'C' 1..2 digits          (only when it is the LAST '_' part)
///   suffix := one of '-', '_', '+' followed by [A-Za-z0-9._+-]*  (may be "")
/// ```
/// Examples:
/// ```text
///   "1.4.9_Dev_C2"       -> 1.4.9 suffix "_Dev"      hw "C2"
///   "1.4.9_C1"           -> 1.4.9 suffix ""          hw "C1"
///   "2.0.0-revamped_C2"  -> 2.0.0 suffix "-revamped" hw "C2"
///   "2.0.0-revamped-dev" -> 2.0.0 suffix "-revamped-dev" hw ""
///   "1.4"                -> invalid (patch missing)
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Version {
    pub valid: bool,
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
    /// Verbatim including its leading separator. 31 chars minus the shortest numeric part
    /// "0.0.0" leaves at most 26 chars, so every grammatical version fits (C++ `char[27]`).
    pub suffix: Text<26>,
    /// "C1".."C99" or "" (C++ `char[4]`).
    pub hw: Text<3>,
}

fn is_suffix_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'+' | b'-')
}

/// Reads "<digits>" at `s[*pos..]`, stops at the first non-digit; 1..5 digits, 0..65535.
fn read_component(s: &[u8], pos: &mut usize) -> Option<u16> {
    let rest = s.get(*pos..).unwrap_or_default();
    let n = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    *pos += n;
    if n > 5 {
        return None;
    }
    u16::try_from(parse_uint(rest.get(..n)?, 65535)?).ok()
}

/// `s[*pos]` is '.'; steps over it.
fn read_dot(s: &[u8], pos: &mut usize) -> Option<()> {
    if s.get(*pos) != Some(&b'.') {
        return None;
    }
    *pos += 1;
    Some(())
}

/// "_C<1..2 digits>" exactly.
fn is_hw_part(p: &[u8]) -> bool {
    match p {
        [b'_', b'C', digits @ ..] => {
            (1..=2).contains(&digits.len()) && digits.iter().all(u8::is_ascii_digit)
        }
        _ => false,
    }
}

fn parse(s: &[u8]) -> Option<Version> {
    if s.len() > VERSION_MAX_LEN {
        return None; // len 0: no major
    }
    let mut pos = 0;
    let major = read_component(s, &mut pos)?;
    read_dot(s, &mut pos)?;
    let minor = read_component(s, &mut pos)?;
    read_dot(s, &mut pos)?;
    let patch = read_component(s, &mut pos)?;

    let tail = s.get(pos..).unwrap_or_default();
    // The suffix starts with a separator; the rest are suffix characters.
    if let [first, rest @ ..] = tail {
        if !matches!(first, b'-' | b'_' | b'+') || !rest.iter().all(|&c| is_suffix_char(c)) {
            return None;
        }
    }
    // hw = last '_' part when it is "_C<n>".
    let mut rest = tail.len();
    let mut hw = Text::new();
    if let Some(u) = tail.iter().rposition(|&c| c == b'_') {
        let part = tail.get(u..).unwrap_or_default();
        if is_hw_part(part) {
            hw = Text::from_slice(part.get(1..)?).ok()?;
            rest = u;
        }
    }
    let suffix = Text::from_slice(tail.get(..rest)?).ok()?;
    Some(Version {
        valid: true,
        major,
        minor,
        patch,
        suffix,
        hw,
    })
}

/// Parses all of `s`. On any grammar violation returns `Version::default()` (valid == false),
/// the C++ `parseVersion` returning false with `out = Version{}`.
pub fn parse_version(s: &[u8]) -> Version {
    parse(s).unwrap_or_default()
}

/// Compares the numeric part only: <0, 0, >0 like strcmp (exactly -1, 0, 1). Suffix and hw are
/// ignored ("2.0.0-revamped" == "2.0.0"). Invalid versions compare lowest.
pub fn compare_version(a: &Version, b: &Version) -> i32 {
    if !a.valid || !b.valid {
        return i32::from(a.valid) - i32::from(b.valid);
    }
    match (a.major, a.minor, a.patch).cmp(&(b.major, b.minor, b.patch)) {
        core::cmp::Ordering::Less => -1,
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Greater => 1,
    }
}

/// True when the suffix contains "revamped" (case-sensitive), i.e. the STM runs the revamped
/// firmware. Protocol v2 is still detected with `gproto`, never from the version string.
pub fn is_revamped(v: &Version) -> bool {
    v.valid && contains_bytes(&v.suffix, b"revamped")
}

/// Writes "M.m.p<suffix>[_<hw>]" (the canonical form of the input). Returns the number of chars
/// written, 0 if the version is invalid or does not fit.
pub fn format_version(v: &Version, out: &mut [u8]) -> usize {
    if !v.valid {
        return 0;
    }
    let mut w = TextBuf::new(out);
    let _ = write!(w, "{}.{}.{}", v.major, v.minor, v.patch);
    w.push_bytes(&v.suffix);
    if !v.hw.is_empty() {
        w.push(b'_');
        w.push_bytes(&v.hw);
    }
    w.fit()
}

const FIRMWARE_VERSION: &str = match option_env!("VDM_VERSION") {
    Some(v) => v,
    None => "0.0.0-native",
};

const MIN_STM_VERSION: &str = match option_env!("VDM_MIN_STM_VERSION") {
    Some(v) => v,
    None => "1.4.0",
};

/// Version of this ESP build: the `VDM_VERSION` build environment variable ("2.1.0-revamped" or
/// "2.1.0-revamped-dev"). Host builds without it report "0.0.0-native".
pub fn firmware_version() -> &'static str {
    FIRMWARE_VERSION
}

/// Minimum STM version the ESP accepts without the "incompatible" banner: the
/// `VDM_MIN_STM_VERSION` build environment variable, default "1.4.0".
pub fn min_stm_version() -> &'static str {
    MIN_STM_VERSION
}

/// Whether the ESP supports the STM firmware it talks to.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StmSupport {
    #[default]
    Unknown = 0,
    Supported = 1,
    TooOld = 2,
}

impl StmSupport {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Unknown),
            1 => Some(Self::Supported),
            2 => Some(Self::TooOld),
            _ => None,
        }
    }
}

/// "unknown", "ok", "too_old".
pub fn stm_support_name(s: StmSupport) -> &'static str {
    match s {
        StmSupport::Unknown => "unknown",
        StmSupport::Supported => "ok",
        StmSupport::TooOld => "too_old",
    }
}

/// Invalid version -> Unknown; numeric part below min_stm_version() -> TooOld; else Supported.
pub fn stm_support(v: &Version) -> StmSupport {
    if !v.valid {
        return StmSupport::Unknown;
    }
    let min = parse_version(min_stm_version().as_bytes());
    if compare_version(v, &min) < 0 {
        StmSupport::TooOld
    } else {
        StmSupport::Supported
    }
}

#[cfg(test)]
mod tests;
