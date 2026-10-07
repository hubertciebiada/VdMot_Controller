//! The record of the last fault (docs/rust/GLUE-DESIGN-STM.md §5.5, D4): the fault handlers of
//! the firmware (`firmware/src/fault.rs`) write it into cortex-m-rt's `.uninit` before the IWDG
//! resets the chip, and the debug terminal prints a valid one at the next start. Rust only, no
//! C++ counterpart: its own magic and check word, outside the C++ no-init cells. It belongs to
//! the boot crate because the fault handlers lie in sector 0 with the boot stage (D9) and must
//! not panic (B3).

/// "VDFR"
pub const RECORD_MAGIC: u32 = 0x5644_4652;
/// magic, kind, pc, lr, xpsr, cfsr, hfsr, bfar, count, check
pub const RECORD_WORDS: usize = 10;

/// What stopped the firmware (word 1 of the record).
pub const KIND_HARD_FAULT: u32 = 1;
pub const KIND_NMI: u32 = 2;
pub const KIND_MEM_MANAGE: u32 = 3;
pub const KIND_BUS_FAULT: u32 = 4;
pub const KIND_USAGE_FAULT: u32 = 5;
/// an interrupt without a handler (the IRQ number is in the pc word)
pub const KIND_UNEXPECTED: u32 = 6;
pub const KIND_PANIC: u32 = 7;

/// One fault: the exception frame words the handler had, the fault status registers and the
/// faults since the record was last invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultRecord {
    pub kind: u32,
    pub pc: u32,
    pub lr: u32,
    pub xpsr: u32,
    pub cfsr: u32,
    pub hfsr: u32,
    pub bfar: u32,
    pub count: u32,
}

/// The check word: the inverted XOR of the first nine words.
pub fn check(words: &[u32; RECORD_WORDS]) -> u32 {
    let [a, b, c, d, e, f, g, h, i, _] = *words;
    !(a ^ b ^ c ^ d ^ e ^ f ^ g ^ h ^ i)
}

impl FaultRecord {
    /// The record the words hold, if magic and check word are right (after a power-on the
    /// words are random).
    pub fn from_words(words: &[u32; RECORD_WORDS]) -> Option<FaultRecord> {
        let [magic, kind, pc, lr, xpsr, cfsr, hfsr, bfar, count, chk] = *words;
        if magic != RECORD_MAGIC || chk != check(words) {
            return None;
        }
        Some(FaultRecord {
            kind,
            pc,
            lr,
            xpsr,
            cfsr,
            hfsr,
            bfar,
            count,
        })
    }

    /// The words with magic and check word.
    pub fn to_words(&self) -> [u32; RECORD_WORDS] {
        let mut words = [
            RECORD_MAGIC,
            self.kind,
            self.pc,
            self.lr,
            self.xpsr,
            self.cfsr,
            self.hfsr,
            self.bfar,
            self.count,
            0,
        ];
        let chk = check(&words);
        if let Some(last) = words.last_mut() {
            *last = chk;
        }
        words
    }

    /// The count of a new fault: one more than a valid record in `old`, else 1.
    pub fn next_count(old: &[u32; RECORD_WORDS]) -> u32 {
        FaultRecord::from_words(old).map_or(1, |r| r.count.wrapping_add(1))
    }

    /// The name of the kind for the terminal.
    pub fn kind_name(&self) -> &'static [u8] {
        match self.kind {
            KIND_HARD_FAULT => b"HardFault",
            KIND_NMI => b"NMI",
            KIND_MEM_MANAGE => b"MemManage",
            KIND_BUS_FAULT => b"BusFault",
            KIND_USAGE_FAULT => b"UsageFault",
            KIND_UNEXPECTED => b"unexpected interrupt",
            KIND_PANIC => b"panic",
            _ => b"unknown",
        }
    }
}

#[cfg(test)]
mod tests;
