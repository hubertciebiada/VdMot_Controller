//! Names and paths of the STM image store on LittleFS (/stm/<name>.bin), port of
//! `vdm/image_store.h`. Hardware-free; storage glue keeps the index and does the file I/O.

use crate::common::{c_str, TextBuf};

pub const IMAGE_NAME_MAX: usize = 31;

fn valid_name_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')
}

/// Accepts "x" or "x.bin" (all of `input`) and writes the bare name to `out`: 1..31 of
/// [A-Za-z0-9._-] without a leading '.' (the {name} rule of the HTTP routes). `out` needs room
/// for the name and the C++ NUL. Returns the name length (None: the C++ false).
pub fn normalize_image_name(input: &[u8], out: &mut [u8]) -> Option<usize> {
    let name = input.strip_suffix(b".bin").unwrap_or(input);
    let len = name.len();
    if len == 0 || len > IMAGE_NAME_MAX || len >= out.len() || name.first() == Some(&b'.') {
        return None;
    }
    if !name.iter().all(|&c| valid_name_char(c)) {
        return None;
    }
    out.get_mut(..len)?.copy_from_slice(name);
    Some(len)
}

/// "/stm/<name>.bin" (+ ".part") for the C string `name`; None when it does not fit (the C++
/// false), else the path length.
pub fn image_path(name: &[u8], part: bool, out: &mut [u8]) -> Option<usize> {
    let mut w = TextBuf::new(out);
    w.push_bytes(b"/stm/");
    w.push_bytes(c_str(name));
    w.push_bytes(b".bin");
    if part {
        w.push_bytes(b".part");
    }
    w.fit_opt()
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
