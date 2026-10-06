//! Minimal stand-ins for core items whose modules are not ported yet. Each item names the core
//! item that replaces it; the replacement is a `use` change, the contract is the C++ one.

/// Local civil time of one instant (C++ `vdm::LocalTime`, `common.h`).
// TODO(core): replace with vdm_esp_core::common::LocalTime
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalTime {
    /// The conversion succeeded.
    pub valid: bool,
    /// For example 2026.
    pub year: u16,
    /// 1..12.
    pub month: u8,
    /// 1..31.
    pub mday: u8,
    /// 0 = Sunday .. 6 = Saturday (`tm_wday`).
    pub wday: u8,
    /// 0..23.
    pub hour: u8,
    /// 0..59.
    pub minute: u8,
    /// 0..60.
    pub second: u8,
    /// UTC seconds since 1970 of the same instant.
    pub epoch: i64,
}

/// HTTP method as the router sees it (C++ `vdm::HttpMethod`, `json_api.h`).
// TODO(core): replace with vdm_esp_core::json_api::HttpMethod
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HttpMethod {
    /// GET.
    Get = 0,
    /// POST.
    Post = 1,
    /// DELETE.
    Delete = 2,
    /// Every other method (HEAD, PUT, ...).
    Other = 3,
}

/// CRC-32 (IEEE 802.3, reflected, init and xorout 0xFFFF_FFFF; zlib `crc32()`), continued from
/// `crc` (0 to start) like C++ `vdm::crc32` (`config.h`).
// TODO(core): replace with vdm_esp_core::config::crc32
pub fn crc32(data: &[u8], crc: u32) -> u32 {
    let mut c = !crc;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_zlib() {
        assert_eq!(crc32(b"", 0), 0);
        assert_eq!(crc32(b"123456789", 0), 0xCBF4_3926);
        assert_eq!(crc32(b"a", 0), 0xE8B7_BE43);
        // continued over two parts = one pass over the whole
        assert_eq!(crc32(b"56789", crc32(b"1234", 0)), 0xCBF4_3926);
    }
}
