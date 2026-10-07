// Port of test/native/glue/test_system_io.cpp (glue_system): the EEPROM is written only for a
// changed value (S13), the stored blocks load without flags at the next start (W11). Every case
// also compares its whole transcript with the C++ golden.

use super::bench::{Boot, Case, PLAIN};
use super::golden::Reset;

/// value n (1-based) of the gstax reply (`gstaxField`)
fn gstax_field(b: &mut Boot<'_>, n: usize) -> i64 {
    let reply = b.exchange("gstax\n");
    assert!(reply.starts_with("gstax "), "{reply:?}");
    Boot::field(&reply, n)
}

#[test]
fn repeated_configuration_requests_write_the_eeprom_only_for_a_new_value() {
    let mut case = Case::new(
        "io__repeated_configuration_requests_write_the_eeprom_only_for_a_new",
        "system: repeated configuration requests write the EEPROM only for a new value",
    );
    let mut b = case.boot(PLAIN, |_| {});
    // the first start's configuration write
    b.run_main(5000);
    assert_eq!(b.exchange("eepst\n"), "eepst 1 \r\n");
    assert_eq!(b.exchange("stlnm 2000\n"), "stlnm\r\n");
    b.run_main(5000);
    let writes = gstax_field(&mut b, 20);
    for _ in 0..10 {
        assert_eq!(b.exchange("stlnm 2000\n"), "stlnm\r\n");
    }
    b.run_main(5000);
    assert_eq!(gstax_field(&mut b, 20), writes);
    assert_eq!(b.exchange("eepst\n"), "eepst 1 \r\n");
    assert_eq!(b.exchange("stlnt 3600\n"), "stlnt\r\n");
    assert_eq!(b.exchange("stlnt 3600\n"), "stlnt\r\n");
    assert_eq!(b.exchange("eepst\n"), "eepst 0 \r\n");
    b.run_main(5000);
    assert_eq!(gstax_field(&mut b, 20), writes + 1);
    assert_eq!(b.exchange("gtlnt\n"), "gtlnt 3600\r\n");
    assert_eq!(b.exchange("sfspo 255 40\n"), "sfspo 255 ok\r\n");
    assert_eq!(b.exchange("slcfg 60\n"), "slcfg ok\r\n");
    b.run_main(5000);
    assert_eq!(gstax_field(&mut b, 20), writes + 2);
    // cfgFlags of the start-up load: unverified (a new chip)
    assert_eq!(gstax_field(&mut b, 18), 64);
    b.finish();
    case.check_golden();
}

#[test]
fn after_the_first_start_and_changes_of_every_block_the_next_start_loads_without_flags() {
    let mut case = Case::new(
        "io__after_the_first_start_and_changes_of_every_block_the_next_start",
        "system: after the first start and changes of every block the next start loads without flags",
    );
    let mut b = case.boot(PLAIN, |_| {});
    b.run_main(5000);
    b.exchange("stlnt 3600\n");
    b.exchange("slcfg 90\n");
    b.exchange("sfspo 3 20\n");
    b.run_main(5000);
    b.reset_controller();

    assert_eq!(case.boot_index(), 1);
    let mut b = case.boot(PLAIN, |_| {});
    assert_eq!(b.last_reset(), Reset::Software);
    assert_eq!(gstax_field(&mut b, 18), 0);
    assert_eq!(gstax_field(&mut b, 19), 0);
    assert_eq!(b.exchange("eepst\n"), "eepst 1 \r\n");
    b.finish();
    case.check_golden();
}
