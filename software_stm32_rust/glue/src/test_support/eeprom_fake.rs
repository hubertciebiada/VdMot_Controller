//! The 24LC64 behind the block API (port of the fake `I2C_eeprom` of `fake_board.cpp`): 8 KiB,
//! erased 0xFF, the transfers logged, failures injected by transfer number.

use crate::eeprom24::EepromDevice;

pub const EEPROM_SIZE: usize = 8192;

/// One transfer (fake::EepromOp).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EepromOp {
    pub write: bool,
    pub address: u16,
    pub length: u16,
    pub ok: bool,
}

/// Transfers are numbered from 1 per direction; write #n fails from `fail_writes_from` for
/// `fail_writes_count` transfers (0: every later one) with `write_error`; a failing read
/// returns 0 and leaves the buffer as it was.
pub struct FakeEeprom {
    pub bytes: Vec<u8>,
    pub ops: Vec<EepromOp>,
    pub writes: u32,
    pub reads: u32,
    pub fail_writes_from: u32,
    pub fail_writes_count: u32,
    pub fail_reads_from: u32,
    pub fail_reads_count: u32,
    pub write_error: i32,
}

impl Default for FakeEeprom {
    /// a new controller: erased
    fn default() -> Self {
        FakeEeprom {
            bytes: vec![0xFF; EEPROM_SIZE],
            ops: Vec::new(),
            writes: 0,
            reads: 0,
            fail_writes_from: 0,
            fail_writes_count: 0,
            fail_reads_from: 0,
            fail_reads_count: 0,
            // Wire: NACK on the address
            write_error: 2,
        }
    }
}

fn failing(n: u32, from: u32, count: u32) -> bool {
    from != 0 && n >= from && (count == 0 || n < from + count)
}

impl FakeEeprom {
    pub fn stored(&self, address: u16, length: usize) -> Vec<u8> {
        let a = usize::from(address);
        self.bytes[a..a + length].to_vec()
    }
}

impl EepromDevice for FakeEeprom {
    fn write_block(&mut self, memory_address: u16, buffer: &[u8]) -> i32 {
        self.writes += 1;
        let fail = failing(self.writes, self.fail_writes_from, self.fail_writes_count);
        self.ops.push(EepromOp {
            write: true,
            address: memory_address,
            length: buffer.len() as u16,
            ok: !fail,
        });
        if fail {
            return self.write_error;
        }
        // the 24LC64 decodes 13 address bits
        for (i, &b) in buffer.iter().enumerate() {
            self.bytes[(usize::from(memory_address) + i) % EEPROM_SIZE] = b;
        }
        0
    }

    fn read_block(&mut self, memory_address: u16, buffer: &mut [u8]) -> u16 {
        self.reads += 1;
        let fail = failing(self.reads, self.fail_reads_from, self.fail_reads_count);
        self.ops.push(EepromOp {
            write: false,
            address: memory_address,
            length: buffer.len() as u16,
            ok: !fail,
        });
        if fail {
            return 0;
        }
        for (i, b) in buffer.iter_mut().enumerate() {
            *b = self.bytes[(usize::from(memory_address) + i) % EEPROM_SIZE];
        }
        buffer.len() as u16
    }
}
