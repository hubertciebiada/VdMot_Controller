// No C++ counterpart: the C++ glue builds one executable per board revision (glue_motor and
// glue_motor_c1, hardware.h MUX_ON/MUX_OFF and HARDWARE_REVISION_TAG).
use super::BoardRev;

#[test]
fn mux_on_is_high_on_c1_and_low_on_c2() {
    assert!(BoardRev::C1.mux_on_high());
    assert!(!BoardRev::C2.mux_on_high());
}

#[test]
fn the_tag_names_the_revision() {
    assert_eq!(BoardRev::C1.tag(), b"C1");
    assert_eq!(BoardRev::C2.tag(), b"C2");
}
