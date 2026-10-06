// New tests (docs/rust/GLUE-DESIGN-STM.md §7.3): the 30-byte and 32-byte page splits of a write,
// the ready polling, the error propagation, chunked and short reads, on a 24LC64 at the I2C
// level.

use super::*;
use crate::hal::{WIRE_ERROR, WIRE_NACK, WIRE_OK, WIRE_TIMEOUT};
use crate::test_support::io_fakes::FakeBoard;

/// One I2C transfer as the chip saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Xfer {
    /// an address-only write (ready probe) and its status
    Probe(WireStatus),
    /// a write with the memory address and the data length
    Write(u16, usize, WireStatus),
    /// a read and the bytes returned
    Read(usize),
}

/// A 24LC64: 13 address bits, 32-byte pages (a write wraps inside its page), no acknowledge
/// during the write cycle of `cycle_us`; every transfer takes 90 us per byte on the bus.
struct Chip {
    board: FakeBoard,
    mem: Vec<u8>,
    pointer: u16,
    busy_until: u64,
    cycle_us: u64,
    log: Vec<Xfer>,
    /// status of the next data writes (0 = ok)
    fail_writes: Vec<WireStatus>,
    /// the next reads return only this many bytes
    short_reads: Vec<usize>,
    /// the next address writes of a read are not acknowledged
    fail_read_addresses: u32,
    restarts: u32,
}

impl Chip {
    fn new(board: &FakeBoard) -> Self {
        Chip {
            board: board.clone(),
            mem: vec![0xFF; 8192],
            pointer: 0,
            busy_until: 0,
            cycle_us: 3000,
            log: Vec::new(),
            fail_writes: Vec::new(),
            short_reads: Vec::new(),
            fail_read_addresses: 0,
            restarts: 0,
        }
    }

    fn busy(&self) -> bool {
        self.board.now_us() < self.busy_until
    }

    fn data_writes(&self) -> Vec<(u16, usize)> {
        self.log
            .iter()
            .filter_map(|x| match x {
                Xfer::Write(a, n, _) if *n > 0 => Some((*a, *n)),
                _ => None,
            })
            .collect()
    }

    fn probes(&self) -> usize {
        self.log
            .iter()
            .filter(|x| matches!(x, Xfer::Probe(_)))
            .count()
    }
}

impl I2cMaster for Chip {
    fn write(&mut self, addr: u8, bytes: &[u8]) -> WireStatus {
        self.board.advance_us(90 * (1 + bytes.len() as u64));
        if addr != DEVICEADDRESS || self.busy() {
            if bytes.is_empty() {
                self.log.push(Xfer::Probe(WIRE_NACK));
            } else {
                self.log.push(Xfer::Write(
                    0xFFFF,
                    bytes.len().saturating_sub(2),
                    WIRE_NACK,
                ));
            }
            return WIRE_NACK;
        }
        if bytes.is_empty() {
            self.log.push(Xfer::Probe(WIRE_OK));
            return WIRE_OK;
        }
        let address = u16::from_be_bytes([bytes[0], bytes[1]]) & 0x1FFF;
        let data = &bytes[2..];
        if data.is_empty() {
            // the address of a read
            self.pointer = address;
            if self.fail_read_addresses > 0 {
                self.fail_read_addresses -= 1;
                self.log.push(Xfer::Write(address, 0, WIRE_NACK));
                return WIRE_NACK;
            }
            self.log.push(Xfer::Write(address, 0, WIRE_OK));
            return WIRE_OK;
        }
        let status = if self.fail_writes.is_empty() {
            WIRE_OK
        } else {
            self.fail_writes.remove(0)
        };
        self.log.push(Xfer::Write(address, data.len(), status));
        if status != WIRE_OK {
            return status;
        }
        for (i, &b) in data.iter().enumerate() {
            let a = (address & !31) | ((address + i as u16) & 31);
            self.mem[usize::from(a)] = b;
        }
        self.busy_until = self.board.now_us() + self.cycle_us;
        WIRE_OK
    }

    fn read(&mut self, addr: u8, buf: &mut [u8]) -> usize {
        self.board.advance_us(90 * (1 + buf.len() as u64));
        if addr != DEVICEADDRESS || self.busy() {
            self.log.push(Xfer::Read(0));
            return 0;
        }
        let n = if self.short_reads.is_empty() {
            buf.len()
        } else {
            self.short_reads.remove(0).min(buf.len())
        };
        for b in buf.iter_mut().take(n) {
            *b = self.mem[usize::from(self.pointer)];
            self.pointer = (self.pointer + 1) & 0x1FFF;
        }
        self.log.push(Xfer::Read(n));
        n
    }

    fn restart(&mut self) {
        self.restarts += 1;
    }
}

/// An EEPROM 1 s after the start (as after the boot window of the firmware).
fn setup() -> (FakeBoard, I2cEeprom<Chip, FakeBoard>) {
    let board = FakeBoard::new();
    board.set_now_us(1_000_000);
    let chip = Chip::new(&board);
    let ee = I2cEeprom::new(chip, board.clone(), DEVICEADDRESS);
    (board, ee)
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 7 + 3) as u8).collect()
}

#[test]
fn write_block_splits_at_30_bytes_and_at_32_byte_pages() {
    let (_board, mut ee) = setup();
    let data = pattern(70);
    assert_eq!(ee.write_block(0x0007, &data), 0);
    assert_eq!(
        ee.i2c().data_writes(),
        vec![(0x0007, 25), (0x0020, 30), (0x003E, 2), (0x0040, 13)]
    );
    assert_eq!(ee.i2c().mem[0x0007..0x0007 + 70], data[..]);
    assert_eq!(ee.i2c().mem[0x0006], 0xFF);
    assert_eq!(ee.i2c().mem[0x0007 + 70], 0xFF);
    // the first chunk came 1 s after the last write (0): no probe before it
    assert!(matches!(ee.i2c().log[0], Xfer::Write(0x0007, 25, 0)));
}

#[test]
fn write_block_on_a_page_boundary_and_inside_one_page() {
    let (_board, mut ee) = setup();
    assert_eq!(ee.write_block(0x0040, &pattern(64)), 0);
    assert_eq!(
        ee.i2c().data_writes(),
        vec![(0x0040, 30), (0x005E, 2), (0x0060, 30), (0x007E, 2)]
    );
    let (_board, mut ee) = setup();
    assert_eq!(ee.write_block(0x0101, &pattern(30)), 0);
    assert_eq!(ee.i2c().data_writes(), vec![(0x0101, 30)]);
    let (_board, mut ee) = setup();
    assert_eq!(ee.write_block(0x011F, &pattern(3)), 0);
    assert_eq!(ee.i2c().data_writes(), vec![(0x011F, 1), (0x0120, 2)]);
    // nothing to write: no transfer
    let (_board, mut ee) = setup();
    assert_eq!(ee.write_block(0x0100, &[]), 0);
    assert!(ee.i2c().log.is_empty());
}

#[test]
fn write_block_polls_the_chip_until_its_write_cycle_ends() {
    let (board, mut ee) = setup();
    assert_eq!(ee.write_block(0x0000, &pattern(60)), 0);
    // two chunks; before the second the chip is busy for 3 ms: probes until it answers
    assert_eq!(
        ee.i2c().data_writes(),
        vec![(0x0000, 30), (0x001E, 2), (0x0020, 28)]
    );
    let probes = ee.i2c().probes();
    assert!(probes > 2, "{probes}");
    // every probe but the last one of each wait was not acknowledged
    let acked = ee
        .i2c()
        .log
        .iter()
        .filter(|x| **x == Xfer::Probe(WIRE_OK))
        .count();
    assert_eq!(acked, 2);
    assert!(board.now_us() > 1_000_000 + 2 * 3000);
}

#[test]
fn write_block_waits_at_most_5_ms_for_a_chip_that_does_not_answer() {
    let (board, mut ee) = setup();
    ee.i2c().cycle_us = 1_000_000;
    let start = board.now_us();
    assert_eq!(ee.write_block(0x0000, &pattern(40)), i32::from(WIRE_NACK));
    // the first chunk written, then probes for 5 ms, then the second chunk (2 bytes to the page
    // boundary) is tried anyway
    let log = &ee.i2c().log;
    assert!(matches!(log[0], Xfer::Write(0x0000, 30, 0)));
    assert!(matches!(
        log.last(),
        Some(Xfer::Write(0xFFFF, 2, WIRE_NACK))
    ));
    let waited = board.now_us() - start;
    assert!((5000..5000 + 3 * 90 * 33).contains(&waited), "{waited}");
    assert_eq!(I2C_WRITEDELAY, 5000);
}

#[test]
fn write_block_stops_at_the_first_failed_chunk_and_returns_its_status() {
    for status in [WIRE_NACK, WIRE_ERROR, WIRE_TIMEOUT] {
        let (_board, mut ee) = setup();
        ee.i2c().fail_writes = vec![WIRE_OK, status];
        assert_eq!(ee.write_block(0x0000, &pattern(90)), i32::from(status));
        let writes: Vec<_> = ee
            .i2c()
            .log
            .iter()
            .filter(|x| matches!(x, Xfer::Write(..)))
            .cloned()
            .collect();
        assert_eq!(
            writes,
            vec![
                Xfer::Write(0x0000, 30, WIRE_OK),
                Xfer::Write(0x001E, 2, status)
            ]
        );
    }
}

#[test]
fn a_failed_write_also_starts_the_ready_wait() {
    let (board, mut ee) = setup();
    ee.i2c().fail_writes = vec![WIRE_ERROR];
    assert_eq!(ee.write_block(0x0000, &[1]), i32::from(WIRE_ERROR));
    // the next transfer within 5 ms probes first
    let mut b = [0u8; 1];
    assert_eq!(ee.read_block(0x0000, &mut b), 1);
    assert!(matches!(ee.i2c().log[1], Xfer::Probe(WIRE_OK)));
    // more than 5 ms later: no probe
    board.advance_us(6000);
    ee.i2c().log.clear();
    assert_eq!(ee.read_block(0x0000, &mut b), 1);
    assert!(matches!(ee.i2c().log[0], Xfer::Write(0, 0, WIRE_OK)));
}

#[test]
fn the_first_transfer_after_the_start_probes_within_5_ms_of_micros_0() {
    let board = FakeBoard::new();
    let chip = Chip::new(&board);
    let mut ee = I2cEeprom::new(chip, board.clone(), DEVICEADDRESS);
    let mut b = [0u8; 2];
    assert_eq!(ee.read_block(0x0010, &mut b), 2);
    assert_eq!(ee.i2c().log[0], Xfer::Probe(WIRE_OK));
}

#[test]
fn the_ready_wait_works_across_the_wrap_of_micros() {
    let (board, mut ee) = setup();
    board.set_now_us(u64::from(u32::MAX) - 100);
    assert_eq!(ee.write_block(0x0000, &[1]), 0);
    board.advance_us(1000);
    let mut b = [0u8; 1];
    ee.i2c().log.clear();
    assert_eq!(ee.read_block(0x0000, &mut b), 1);
    // micros() wrapped: still within 5 ms of the write, the chip is polled first
    assert!(matches!(ee.i2c().log[0], Xfer::Probe(_)));
    assert_eq!(b[0], 1);
}

#[test]
fn read_block_reads_in_30_byte_chunks_and_counts_the_bytes() {
    let (_board, mut ee) = setup();
    let data = pattern(309);
    ee.i2c().mem[0x0007..0x0007 + 309].copy_from_slice(&data);
    let mut buf = vec![0u8; 309];
    assert_eq!(ee.read_block(0x0007, &mut buf), 309);
    assert_eq!(buf, data);
    let reads: Vec<usize> = ee
        .i2c()
        .log
        .iter()
        .filter_map(|x| match x {
            Xfer::Read(n) => Some(*n),
            _ => None,
        })
        .collect();
    assert_eq!(reads, vec![30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 9]);
    let addresses: Vec<u16> = ee
        .i2c()
        .log
        .iter()
        .filter_map(|x| match x {
            Xfer::Write(a, 0, _) => Some(*a),
            _ => None,
        })
        .collect();
    assert_eq!(
        addresses,
        (0..11).map(|k| 0x0007 + 30 * k).collect::<Vec<u16>>()
    );
}

#[test]
fn read_block_goes_on_after_a_failed_chunk_and_reports_the_bytes_read() {
    let (_board, mut ee) = setup();
    ee.i2c().mem[0..90].copy_from_slice(&pattern(90));
    ee.i2c().fail_read_addresses = 1;
    let mut buf = vec![0xEEu8; 90];
    assert_eq!(ee.read_block(0x0000, &mut buf), 60);
    // the failed chunk keeps what the buffer held
    assert_eq!(buf[..30], [0xEE; 30]);
    assert_eq!(buf[30..], pattern(90)[30..]);
    // a short read adds what came
    let (_board, mut ee) = setup();
    ee.i2c().short_reads = vec![30, 12];
    let mut buf = vec![0u8; 60];
    assert_eq!(ee.read_block(0x0000, &mut buf), 42);
    assert_eq!(ee.read_block(0x0000, &mut []), 0);
}

#[test]
fn reads_and_writes_go_to_the_device_address_with_two_address_bytes() {
    let (_board, mut ee) = setup();
    assert!(ee.is_connected());
    let mut other = I2cEeprom::new(Chip::new(&FakeBoard::new()), FakeBoard::new(), 0x51);
    assert!(!other.is_connected());
    assert_eq!(ee.write_block(0x1234, &[0xAB]), 0);
    assert_eq!(ee.i2c().mem[0x1234], 0xAB);
    assert_eq!(DEVICEADDRESS, 0x50);
    assert_eq!(I2C_BUFFERSIZE, 30);
    assert_eq!(I2C_DEVICESIZE_24LC64, 8192);
}
