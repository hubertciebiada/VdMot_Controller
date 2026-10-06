//! RX ring of the boot window: the `Serial1` RX ring of the C++ (STM32duino, built with
//! `SERIAL_RX_BUFFER_SIZE=1024`). The C++ fills it from the USART1 interrupt; the Rust boot
//! stage fills it from the polled USART1 while it waits for SysTick (window::wait_ms).

/// Slots of the ring; one stays free, so 1023 bytes fit (STM32duino head/tail rule).
pub const FIFO_SLOTS: usize = 1024;

const MASK: usize = FIFO_SLOTS - 1;

/// Bytes of one handshake block: the C++ reads `sizeof buffer2` = 8 bytes at a time.
pub const BLOCK_LEN: usize = 8;

/// The ring buffer. A byte that finds it full is dropped (`HardwareSerial::_rx_complete_irq`).
#[derive(Clone, Debug)]
pub struct Fifo {
    buf: [u8; FIFO_SLOTS],
    head: usize,
    tail: usize,
    dropped: u32,
}

impl Default for Fifo {
    fn default() -> Self {
        Self::new()
    }
}

fn next(i: usize) -> usize {
    i.wrapping_add(1) & MASK
}

impl Fifo {
    pub const fn new() -> Self {
        Fifo {
            buf: [0; FIFO_SLOTS],
            head: 0,
            tail: 0,
            dropped: 0,
        }
    }

    /// Stores a received byte; false (byte dropped and counted) when the ring is full.
    pub fn push(&mut self, byte: u8) -> bool {
        let next = next(self.head);
        if next == self.tail {
            self.dropped = self.dropped.wrapping_add(1);
            return false;
        }
        if let Some(slot) = self.buf.get_mut(self.head) {
            *slot = byte;
        }
        self.head = next;
        true
    }

    /// `Serial1.available()`
    pub fn len(&self) -> usize {
        self.head.wrapping_sub(self.tail) & MASK
    }

    pub fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    /// `Serial1.read()`
    pub fn pop(&mut self) -> Option<u8> {
        if self.is_empty() {
            return None;
        }
        let byte = self.buf.get(self.tail).copied();
        self.tail = next(self.tail);
        byte
    }

    /// `if (available() >= 8) readBytes(buffer2, 8)`: the next 8 bytes, or `None` (nothing
    /// taken) while fewer wait.
    pub fn take_block(&mut self) -> Option<[u8; BLOCK_LEN]> {
        if self.len() < BLOCK_LEN {
            return None;
        }
        let mut block = [0u8; BLOCK_LEN];
        for slot in block.iter_mut() {
            *slot = self.pop().unwrap_or(0);
        }
        Some(block)
    }

    /// `while (Serial1.available()) Serial1.read();`
    pub fn clear(&mut self) {
        self.tail = self.head;
    }

    /// Bytes dropped because the ring was full (wraps).
    pub fn dropped(&self) -> u32 {
        self.dropped
    }
}

#[cfg(test)]
mod tests;
