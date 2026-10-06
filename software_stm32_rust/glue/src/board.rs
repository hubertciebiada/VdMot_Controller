//! Board revision of the controller (C++ `hardware.h`, `HARDWARE_REVISION_C1` /
//! `HARDWARE_REVISION_C2`): the C1 sample switches the valve MUX relay with the other level. The
//! firmware passes its revision (cargo feature `c1` / `c2`) to the modules that need it.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoardRev {
    C1,
    C2,
}

impl BoardRev {
    /// The level of `MUX_ON()`, which selects the even valve of an L293 channel: high on C1, low
    /// on C2. `MUX_OFF()` writes the other level.
    pub const fn mux_on_high(self) -> bool {
        matches!(self, BoardRev::C1)
    }

    /// `HARDWARE_REVISION_TAG` (gvers, the `VDM-HW:` marker of the image).
    pub const fn tag(self) -> &'static [u8] {
        match self {
            BoardRev::C1 => b"C1",
            BoardRev::C2 => b"C2",
        }
    }
}

#[cfg(test)]
mod tests;
