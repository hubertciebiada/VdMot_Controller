//! Minimal stand-ins for core items whose modules are not ported yet. Each item names the core
//! item that replaces it; the replacement is a `use` change, the contract is the C++ one.

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
