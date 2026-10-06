//! The ID block at 0x08000200 (docs/rust/GLUE-DESIGN-STM.md §6.2): the strings the ESP
//! flasher looks for in an image and the boot window uses, NUL-delimited and ahead of
//! everything else in the image:
//!
//! ```text
//! \0 <version> \0 VDM-HW:<tag> \0 DEADBEEF \0 BEEFIT \0
//! ```
//!
//! The ESP's `validateImage` takes the first NUL-terminated printable run that parses as a
//! version, the board tag of a run ending in `VDM-HW:C<n>` (two different ones are a
//! conflict) and requires `DEADBEEF` and `BEEFIT` somewhere in the image. Rust string
//! literals carry no NUL and the linker packs them, so the block is written as bytes by
//! `firmware/build.rs` with [`write`], and the boot stage reads its pattern and reply back
//! from flash: the bytes the ESP checks are the bytes the code uses.

/// Address of the block: behind the vector table (0x194 / 0x198 bytes), before `.text`.
pub const ID_BLOCK_ADDR: u32 = 0x0800_0200;
/// Room of the block (the linker script asserts it).
pub const ID_BLOCK_MAX: usize = 64;
/// Board marker prefix (`HARDWARE_MARKER_PREFIX`).
pub const MARKER_PREFIX: [u8; 7] = *b"VDM-HW:";
/// The handshake pattern of the ESP.
pub const PATTERN: [u8; 8] = *b"DEADBEEF";
/// The reply of the boot window (sent with CR LF).
pub const REPLY: [u8; 6] = *b"BEEFIT";
/// The ESP scan keeps runs of at most 31 characters (`ImageScan::run`).
pub const VERSION_MAX: usize = 31;

/// Pattern and reply of the boot window, as read from the ID block in flash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootId {
    pub pattern: [u8; 8],
    pub reply: [u8; 6],
}

impl BootId {
    /// The values [`write`] puts into every block.
    pub const STANDARD: BootId = BootId {
        pattern: PATTERN,
        reply: REPLY,
    };
}

/// Offsets of the fields inside the block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdLayout {
    pub version: usize,
    pub version_len: usize,
    /// the tag behind `VDM-HW:`
    pub tag: usize,
    pub tag_len: usize,
    pub pattern: usize,
    pub reply: usize,
    /// bytes of the block, the final NUL included
    pub len: usize,
}

impl IdLayout {
    /// The layout for a version and a tag of these lengths (no check of the lengths).
    pub const fn new(version_len: usize, tag_len: usize) -> IdLayout {
        let version: usize = 1;
        let marker = version.wrapping_add(version_len).wrapping_add(1);
        let tag = marker.wrapping_add(MARKER_PREFIX.len());
        let pattern = tag.wrapping_add(tag_len).wrapping_add(1);
        let reply = pattern.wrapping_add(PATTERN.len()).wrapping_add(1);
        let len = reply.wrapping_add(REPLY.len()).wrapping_add(1);
        IdLayout {
            version,
            version_len,
            tag,
            tag_len,
            pattern,
            reply,
            len,
        }
    }
}

// the longest version and tag fit, so a valid block never exceeds its room
const _: () = assert!(IdLayout::new(VERSION_MAX, 3).len <= ID_BLOCK_MAX);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdError {
    /// empty, longer than [`VERSION_MAX`], or a byte that is not printable ASCII without space
    Version,
    /// not `C` and 1 or 2 digits (the ESP's `boardTagValid`)
    Tag,
}

/// A version as the ESP scan can find it: 1..=31 printable ASCII characters, no space.
pub fn version_ok(version: &[u8]) -> bool {
    !version.is_empty()
        && version.len() <= VERSION_MAX
        && version.iter().all(|&c| (0x21..=0x7E).contains(&c))
}

/// `C` followed by 1 or 2 digits.
pub fn tag_ok(tag: &[u8]) -> bool {
    match tag.split_first() {
        Some((b'C', digits)) => {
            (1..=2).contains(&digits.len()) && digits.iter().all(u8::is_ascii_digit)
        }
        _ => false,
    }
}

/// Writes the block for `version` and `tag` into `out` (bytes behind the block are 0).
pub fn write(
    version: &[u8],
    tag: &[u8],
    out: &mut [u8; ID_BLOCK_MAX],
) -> Result<IdLayout, IdError> {
    if !version_ok(version) {
        return Err(IdError::Version);
    }
    if !tag_ok(tag) {
        return Err(IdError::Tag);
    }
    let layout = IdLayout::new(version.len(), tag.len());
    *out = [0; ID_BLOCK_MAX];
    put(out, layout.version, version);
    put(
        out,
        layout.tag.wrapping_sub(MARKER_PREFIX.len()),
        &MARKER_PREFIX,
    );
    put(out, layout.tag, tag);
    put(out, layout.pattern, &PATTERN);
    put(out, layout.reply, &REPLY);
    Ok(layout)
}

fn put(out: &mut [u8], at: usize, bytes: &[u8]) {
    if let Some(dst) = out.get_mut(at..at.wrapping_add(bytes.len())) {
        dst.copy_from_slice(bytes);
    }
}

#[cfg(test)]
mod tests;
