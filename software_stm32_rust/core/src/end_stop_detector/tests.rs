//! Port of test/native/test_end_stop_detector.cpp.

use super::*;

/// Firmware 1.x TimerHandler0, kept as the reference for the filter and the
/// bound check; its overcurrent counter summed over the whole move.
#[derive(Default)]
struct LegacyDetector {
    current: i32,
    old: i32,
    debounce: i32,
    overcnt: i32,
    low: i32,
    high: i32,
}

impl LegacyDetector {
    fn sample(&mut self, raw: i32) -> bool {
        if self.debounce < 255 {
            self.debounce += 1;
        }
        if self.debounce > 250 {
            self.current = (self.old * 9800 + raw * 200) / 10000;
            self.old = self.current;
        }
        if (self.current > 600 || self.current < -600) && self.overcnt < 255 {
            self.overcnt += 1;
        }
        self.current > 1000
            || self.current < -1000
            || self.overcnt > 10
            || (self.debounce > 250 && (self.current > self.high || self.current < self.low))
    }
}

/// Runs n samples of `raw`, returns the index of the first trip or -1.
fn run_until_trip(d: &mut EndStopDetector, raw: i32, n: i32) -> i32 {
    for i in 0..n {
        if d.sample(raw) != Trip::None {
            return i;
        }
    }
    -1
}

#[test]
fn filter_held_at_0_during_the_inrush_time() {
    let mut d = EndStopDetector::default();
    d.arm(-340, 340, InrushMode::Off);
    for i in 0..250 {
        assert_eq!(d.sample(900), Trip::None, "sample {i}");
        assert_eq!(d.current(), 0, "sample {i}");
    }
    assert_eq!(d.sample(900), Trip::None);
    assert_eq!(d.current(), 18); // 900 * 0.02
}

#[test]
fn no_limit_is_active_during_the_inrush_time() {
    // documented in PROTOCOL_V2.md: safety and hard limit use the filtered current
    let mut d = EndStopDetector::default();
    d.arm(-340, 340, InrushMode::Off);
    for i in 0..250 {
        assert_eq!(d.sample(100_000), Trip::None, "sample {i}");
        assert_eq!(d.over_count(), 0, "sample {i}");
    }
    assert_eq!(d.trip(), Trip::None);
    assert_eq!(d.peak(), 0);
    // the first filtered sample: 2000 (0.02 x 100000) is above both limits
    assert_eq!(d.sample(100_000), Trip::Hard);
}

#[test]
fn bound_trip_after_the_filter_crosses_the_threshold() {
    let mut d = EndStopDetector::default();
    d.arm(-340, 340, InrushMode::Off);
    let idx = run_until_trip(&mut d, 500, 2000);
    assert!(idx > 250, "idx {idx}");
    assert_eq!(d.trip(), Trip::Bound);
    assert!(d.trip_current() > 340);
    assert!(d.current() > 340);
    assert_eq!(d.peak(), d.current());
}

#[test]
fn negative_direction_trips_on_the_low_bound() {
    let mut d = EndStopDetector::default();
    d.arm(-340, 340, InrushMode::Off);
    assert!(run_until_trip(&mut d, -500, 2000) > 250);
    assert_eq!(d.trip(), Trip::Bound);
    assert!(d.trip_current() < -340);
    assert_eq!(d.peak(), -d.current());
}

#[test]
fn current_inside_the_bounds_never_trips() {
    let mut d = EndStopDetector::default();
    d.arm(-340, 340, InrushMode::Off);
    assert_eq!(run_until_trip(&mut d, 300, 5000), -1);
    assert_eq!(d.trip(), Trip::None);
    // integer IIR dead band of firmware 1.x: settles below 300 - 49
    assert!(d.current() >= 251, "current {}", d.current());
    assert!(d.current() <= 300, "current {}", d.current());
}

#[test]
fn equal_to_the_bound_is_not_beyond_it() {
    let mut d = EndStopDetector::default();
    d.arm(-100, 100, InrushMode::Off);
    // settles at exactly 100 from a raw value of 149 (dead band 49)
    assert_eq!(run_until_trip(&mut d, 149, 5000), -1);
    assert_eq!(d.current(), 100);
    assert_eq!(d.sample(151), Trip::Bound); // 100*0.98 + 151*0.02 = 101.02 -> 101 > 100
    assert_eq!(d.current(), 101);
}

#[test]
fn matches_firmware_1_x_while_no_spike_occurs() {
    let raws = [0, 120, 350, 480, -200, -700, 20];
    for raw in raws {
        let mut d = EndStopDetector::default();
        let mut l = LegacyDetector::default();
        d.arm(-340, 340, InrushMode::Off);
        l.low = -340;
        l.high = 340;
        for i in 0..1500 {
            let legacy_trip = l.sample(raw);
            let trip = d.sample(raw) != Trip::None;
            assert_eq!(trip, legacy_trip, "raw {raw} sample {i}");
            assert_eq!(d.current(), l.current, "raw {raw} sample {i}");
            if trip {
                break;
            }
        }
    }
}

#[test]
fn safety_limit_needs_more_than_10_consecutive_samples() {
    let mut d = EndStopDetector::default();
    // bounds out of the way
    d.arm(-100_000, 100_000, InrushMode::Off);
    // bring the filter above 60 mA
    let mut i = 0;
    while d.current() <= 600 {
        assert!(i < 5000);
        i += 1;
        d.sample(990);
    }
    // the sample above was the first one over the limit
    assert_eq!(d.over_count(), 1);
    for k in 2..=10u8 {
        assert_eq!(d.sample(990), Trip::None, "k {k}");
        assert_eq!(d.over_count(), k);
    }
    assert_eq!(d.sample(990), Trip::Safety);
    assert_eq!(d.trip(), Trip::Safety);
    assert!(d.trip_current() > 600);
}

#[test]
fn short_spikes_no_longer_add_up_consecutive_counter() {
    let mut d = EndStopDetector::default();
    let mut l = LegacyDetector::default();
    d.arm(-100_000, 100_000, InrushMode::Off);
    l.low = -100_000;
    l.high = 100_000;
    // settle both filters just below 60 mA
    for _ in 0..2000 {
        d.sample(640);
        l.sample(640);
    }
    assert!(d.current() <= 600);
    assert!(d.current() > 590);

    // repeated bursts over the limit, each shorter than 10 samples
    let mut legacy_tripped = false;
    for burst in 0..20 {
        if legacy_tripped {
            break;
        }
        for k in 0..6 {
            assert_eq!(d.sample(1100), Trip::None, "burst {burst} k {k}");
            legacy_tripped = legacy_tripped || l.sample(1100);
        }
        // back below the limit, then settle just below it again (from below)
        for k in 0..100 {
            assert_eq!(d.sample(0), Trip::None, "burst {burst} k {k}");
            legacy_tripped = legacy_tripped || l.sample(0);
        }
        for k in 0..800 {
            assert_eq!(d.sample(640), Trip::None, "burst {burst} k {k}");
            legacy_tripped = legacy_tripped || l.sample(640);
        }
    }
    assert!(legacy_tripped); // 1.x stopped the motor on the sum of the bursts
    assert_eq!(d.trip(), Trip::None);
}

#[test]
fn counter_resets_when_the_current_drops_to_the_limit() {
    let mut d = EndStopDetector::default();
    d.arm(-100_000, 100_000, InrushMode::Off);
    while d.current() <= 600 {
        d.sample(900);
    }
    assert_eq!(d.over_count(), 1);
    while d.current() > 600 {
        d.sample(0);
    }
    assert_eq!(d.over_count(), 0);
}

#[test]
fn hard_limit_trips_on_the_first_sample_above_it() {
    let mut d = EndStopDetector::default();
    d.arm(-100_000, 100_000, InrushMode::Off);
    for _ in 0..250 {
        d.sample(0);
    }
    // drive the filter over 100 mA as fast as possible
    let mut t = Trip::None;
    let mut n = 0;
    while t == Trip::None {
        assert!(n < 1000);
        n += 1;
        t = d.sample(100_000);
    }
    assert_eq!(t, Trip::Hard);
    assert!(d.current() > 1000);
    assert_eq!(d.trip(), Trip::Hard);
}

#[test]
fn hard_beats_safety_beats_bound() {
    let mut d = EndStopDetector::default();
    d.arm(-10, 10, InrushMode::Off);
    for _ in 0..250 {
        d.sample(0);
    }
    let mut first = Trip::None;
    for i in 0..5000 {
        let t = d.sample(1200);
        if first == Trip::None {
            first = t;
        }
        if t == Trip::Hard {
            break;
        }
        if d.current() > 600 && d.over_count() > 10 {
            assert_eq!(t, Trip::Safety, "sample {i}");
        }
    }
    assert_eq!(first, Trip::Bound);
    assert_eq!(d.trip(), Trip::Bound); // latched first trip
}

#[test]
fn extreme_raw_values_stay_in_range() {
    let mut d = EndStopDetector::default();
    d.arm(-340, 340, InrushMode::Off);
    for _ in 0..400 {
        d.sample(i32::MAX);
    }
    assert!(d.current() > 1000);
    let mut e = EndStopDetector::default();
    e.arm(-340, 340, InrushMode::Off);
    for _ in 0..400 {
        e.sample(i32::MIN);
    }
    assert!(e.current() < -1000);
    assert_eq!(e.peak(), -e.current());
}

#[test]
fn idle_resets_filter_keeps_statistics_until_arm() {
    let mut d = EndStopDetector::default();
    d.arm(-340, 340, InrushMode::Off);
    run_until_trip(&mut d, 500, 2000);
    let peak = d.peak();
    assert!(peak > 340);
    d.idle();
    assert_eq!(d.current(), 0);
    assert_eq!(d.over_count(), 0);
    assert_eq!(d.peak(), peak);
    assert_eq!(d.trip(), Trip::Bound);

    // inrush restarts after idle
    for i in 0..250 {
        assert_eq!(d.sample(900), Trip::None, "sample {i}");
    }
    assert_eq!(d.current(), 0);

    d.arm(-340, 340, InrushMode::Off);
    assert_eq!(d.peak(), 0);
    assert_eq!(d.trip(), Trip::None);
    assert_eq!(d.trip_current(), 0);
}

/// Drives the filter with `raw` until it reads exactly `target` (after the inrush time).
/// Returns the trip of the sample that first reached target.
fn settle_at(d: &mut EndStopDetector, raw: i32, target: i32) -> Trip {
    for _ in 0..250 {
        d.sample(0);
    }
    let mut t = Trip::None;
    let mut n = 0;
    while d.current() != target {
        assert!(n < 5000);
        assert!(d.current() < target, "current {}", d.current());
        t = d.sample(raw);
        n += 1;
    }
    t
}

#[test]
fn a_filtered_current_of_exactly_the_safety_limit_does_not_count() {
    // raw 649 settles the filter at exactly 600 from below: (9800 * 600 + 200 * 649) / 10000 == 600
    let mut d = EndStopDetector::default();
    d.arm(-100_000, 100_000, InrushMode::Off);
    assert_eq!(settle_at(&mut d, 649, 600), Trip::None);
    for i in 0..50 {
        assert_eq!(d.sample(649), Trip::None, "sample {i}");
        assert_eq!(d.current(), 600, "sample {i}");
    }
    assert_eq!(d.over_count(), 0);
    assert_eq!(d.trip(), Trip::None);
}

#[test]
fn a_filtered_current_of_exactly_the_hard_limit_is_not_a_hard_trip() {
    // raw 1049 settles the filter at exactly 1000 (never above it)
    let mut d = EndStopDetector::default();
    d.arm(-100_000, 100_000, InrushMode::Off);
    assert_eq!(settle_at(&mut d, 1049, 1000), Trip::Safety); // long above 600 on the way up
    for i in 0..50 {
        assert_eq!(d.sample(1049), Trip::Safety, "sample {i}");
        assert_eq!(d.current(), 1000, "sample {i}");
    }
    assert_eq!(d.trip(), Trip::Safety);
    assert_eq!(d.peak(), 1000);
}

#[test]
fn a_filtered_current_equal_to_the_low_bound_is_inside() {
    let mut d = EndStopDetector::default();
    d.arm(600, 100_000, InrushMode::Off);
    // on the way up the filter is below the low bound: Bound; exactly at it: None
    assert_eq!(settle_at(&mut d, 649, 600), Trip::None);
    for i in 0..20 {
        assert_eq!(d.sample(649), Trip::None, "sample {i}");
    }
    assert_eq!(d.trip(), Trip::Bound);
    assert_eq!(d.trip_current(), 12); // first settled sample: 649 * 200 / 10000 == 12 < 600
}

#[test]
fn the_consecutive_counter_saturates_at_255() {
    let mut d = EndStopDetector::default();
    d.arm(-100_000, 100_000, InrushMode::Off);
    for _ in 0..250 + 1000 {
        d.sample(900); // settles near 900: above 600, below 1000
    }
    assert!(d.current() > 600);
    assert!(d.current() <= 1000);
    assert_eq!(d.over_count(), 255);
    d.sample(900);
    assert_eq!(d.over_count(), 255);
}

#[test]
fn raw_samples_are_clamped_to_exactly_100000() {
    // reference: the same filter fed the clamp value itself
    let reference = |clamped: i32, n: i32| -> i32 {
        let mut c = 0;
        for _ in 0..n {
            c = (c * 9800 + clamped * 200) / 10000;
        }
        c
    };
    for n in [1, 40, 2000] {
        let mut hi = EndStopDetector::default();
        hi.arm(-1_000_000, 1_000_000, InrushMode::Off);
        let mut lo = EndStopDetector::default();
        lo.arm(-1_000_000, 1_000_000, InrushMode::Off);
        for _ in 0..250 {
            hi.sample(i32::MAX);
            lo.sample(i32::MIN);
        }
        for _ in 0..n {
            hi.sample(i32::MAX);
            lo.sample(i32::MIN);
        }
        assert_eq!(hi.current(), reference(100_000, n), "n {n}");
        assert_eq!(lo.current(), reference(-100_000, n), "n {n}");
    }
    // one step: 99999 would give 1999, 100000 gives 2000; after 40 steps 100001 differs as well
    assert_eq!(reference(100_000, 1), 2000);
    assert_ne!(reference(100_000, 40), reference(100_001, 40));
}
