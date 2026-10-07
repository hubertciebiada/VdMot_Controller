// New cases (design §7.3): the PSC/ARR values of §2.4 for both chips, and the edges of the
// HardwareTimer formula.

use super::*;

#[test]
fn f401_at_84_mhz_tim1_1_ms_and_tim2_9_99994_ms() {
    assert_eq!(
        overflow(84_000_000, 1000),
        Overflow {
            psc: 1,
            arr: 41_999
        }
    );
    assert_eq!(
        overflow(84_000_000, 10_000),
        Overflow {
            psc: 12,
            arr: 64_614
        }
    );
    // 13 x 64615 timer clocks at 84 MHz
    let o = overflow(84_000_000, 10_000);
    assert_eq!((o.psc + 1) * (o.arr + 1), 839_995);
}

#[test]
fn f411_at_96_mhz_tim1_1_ms_and_tim2_10_ms() {
    assert_eq!(
        overflow(96_000_000, 1000),
        Overflow {
            psc: 1,
            arr: 47_999
        }
    );
    assert_eq!(
        overflow(96_000_000, 10_000),
        Overflow {
            psc: 14,
            arr: 63_999
        }
    );
}

#[test]
fn the_clock_counts_in_whole_mhz_and_short_periods_keep_the_prescaler_at_1() {
    // 84.9 MHz is taken as 84 MHz, as getTimerClkFreq() / 1000000 in the core
    assert_eq!(overflow(84_900_000, 1000), overflow(84_000_000, 1000));
    assert_eq!(overflow(16_000_000, 100), Overflow { psc: 0, arr: 1599 });
    // exactly 65536 clocks need the prescaler 2
    assert_eq!(
        overflow(1_000_000, 65_536),
        Overflow {
            psc: 1,
            arr: 32_767
        }
    );
    assert_eq!(
        overflow(1_000_000, 65_535),
        Overflow {
            psc: 0,
            arr: 65_534
        }
    );
    // a period of 0 clocks: ARR is not decremented below 0
    assert_eq!(overflow(84_000_000, 0), Overflow { psc: 0, arr: 0 });
}
