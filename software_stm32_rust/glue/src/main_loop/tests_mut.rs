// Port of software_stm32/test/native/glue/test_main__mut.cpp (glue_main): the exact LED ticks
// over two cycles, a released button.
use std::vec;

use crate::hal::{In, Out};

use super::stub_env::MainBench;

#[test]
fn loop_system_the_led_goes_off_exactly_at_the_30th_tick_and_on_at_the_31st_every_31_ticks() {
    let mut b = MainBench::new();
    let mut ms = 0;
    for tick in 1..=62 {
        ms += 101;
        b.loop_at(ms);
        let w = b.env.board.writes_of(Out::Led);
        if tick < 30 {
            assert!(w.is_empty(), "tick {tick}");
        } else if tick == 30 {
            assert_eq!(w, vec![false], "tick {tick}");
        } else if tick < 61 {
            assert_eq!(w.len(), 2, "tick {tick}");
        } else if tick == 61 {
            assert_eq!(w.len(), 3, "tick {tick}");
            assert_eq!(w.last(), Some(&false), "tick {tick}");
        } else {
            assert_eq!(w.len(), 4, "tick {tick}");
            assert_eq!(w.last(), Some(&true), "tick {tick}");
        }
    }
}

#[test]
fn loop_system_a_released_button_is_never_reported() {
    let mut b = MainBench::new();
    b.env.board.set_input(In::Button, false);
    b.loop_at(101);
    b.loop_at(202);
    assert!(b.env.take_tx().is_empty());
}
