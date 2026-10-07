// New case (design §5.5, no C++ counterpart): the line of the fault record after the banner.

use super::*;
use crate::test_support::io_fakes::FakeSerial;
use vdm_stm_boot::fault_record::{KIND_PANIC, KIND_USAGE_FAULT};

#[test]
fn the_fault_record_is_one_line_with_the_registers_in_hex() {
    let mut dbg = FakeSerial::new();
    let r = FaultRecord {
        kind: KIND_USAGE_FAULT,
        pc: 0x2000_0100,
        lr: 0xFFFF_FFF9,
        xpsr: 0x0100_0000,
        cfsr: 0x0001_0000,
        hfsr: 0,
        bfar: 0xE000_ED38,
        count: 2,
    };
    print_fault_record(&mut dbg, &r);
    assert_eq!(
        dbg.take_tx(),
        "last fault: UsageFault pc 0x20000100 lr 0xFFFFFFF9 xpsr 0x1000000 cfsr 0x10000 \
         hfsr 0x0 bfar 0xE000ED38 count 2\r\n"
    );
    print_fault_record(
        &mut dbg,
        &FaultRecord {
            kind: KIND_PANIC,
            count: 17,
            ..r
        },
    );
    assert!(dbg.take_tx().starts_with("last fault: panic pc "));
}
