//! The C++ 2.1.7 `.noinit` cells (D3, docs/rust/GLUE-DESIGN-STM.md §3.2): 212 bytes at
//! 0x20003234 (linker region NOINIT, symbol `__vdm_noinit`) that no start-up code clears. No
//! Rust object lives there: the region is copied word by word with volatile accesses, like a
//! peripheral, so no uninitialised Rust memory is ever read. vdm_stm_boot::capture (and the
//! glue later) sees byte images with the C++ offsets.

use vdm_stm_boot::NOINIT_LEN;

extern "C" {
    /// ORIGIN(NOINIT), memory/vdm.x
    static __vdm_noinit: u32;
}

fn base() -> *mut u32 {
    core::ptr::addr_of!(__vdm_noinit) as *mut u32
}

/// The bytes of the region (little endian, as the C++ structs lie in RAM).
pub fn read() -> [u8; NOINIT_LEN] {
    let mut out = [0u8; NOINIT_LEN];
    for (i, word) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        // SAFETY: word i of the 53 words of the NOINIT region, which only this module accesses
        *word = unsafe { core::ptr::read_volatile(base().add(i)) }.to_le_bytes();
    }
    out
}

/// Writes the region back.
pub fn write(cells: &[u8; NOINIT_LEN]) {
    for (i, word) in cells.as_chunks::<4>().0.iter().enumerate() {
        // SAFETY: as in read()
        unsafe { core::ptr::write_volatile(base().add(i), u32::from_le_bytes(*word)) };
    }
}
