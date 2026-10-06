//! Port of test/native/test_profile_recorder.cpp.

use super::*;
use std::vec::Vec;

fn counts(p: &ProfileRecorder) -> Vec<u16> {
    (0..p.size()).map(|i| p.at(i).count).collect()
}

#[test]
fn empty_after_reset() {
    let mut p = ProfileRecorder::default();
    assert_eq!(p.size(), 0);
    assert_eq!(p.spacing(), 1);
    p.add(5, 100);
    p.reset();
    assert_eq!(p.size(), 0);
    assert_eq!(p.spacing(), 1);
    assert_eq!(p.at(0).count, 0);
    assert_eq!(p.at(0).current, 0);
}

#[test]
fn short_move_keeps_every_pulse() {
    let mut p = ProfileRecorder::default();
    p.reset();
    for c in 0..10u32 {
        p.add(c, (c * 10) as i32);
    }
    assert_eq!(p.size(), 10);
    for i in 0..10u8 {
        assert_eq!(p.at(i).count, u16::from(i));
        assert_eq!(p.at(i).current, u16::from(i) * 10);
    }
    assert_eq!(p.at(10).count, 0);
}

#[test]
fn repeated_polls_with_the_same_count_record_once() {
    let mut p = ProfileRecorder::default();
    p.reset();
    p.add(0, 1);
    p.add(0, 2);
    p.add(0, 3);
    assert_eq!(p.size(), 1);
    assert_eq!(p.at(0).current, 1);
}

#[test]
fn never_more_than_32_samples_equally_spaced() {
    for total in [33u32, 64, 100, 1000, 4000, 65535] {
        let mut p = ProfileRecorder::default();
        p.reset();
        for c in 0..=total {
            p.add(c, 50);
        }
        assert!(p.size() <= PROFILE_SAMPLES, "total {total}");
        assert!(p.size() >= PROFILE_SAMPLES / 2, "total {total}");
        let v = counts(&p);
        for i in 1..v.len() {
            assert_eq!(v[i] - v[i - 1], p.spacing(), "total {total} i {i}");
        }
        assert_eq!(v[0], 0, "total {total}");
        // the samples cover the move
        assert!(
            u32::from(v[v.len() - 1]) + 2 * u32::from(p.spacing()) > total,
            "total {total}"
        );
    }
}

#[test]
fn coarse_polling_several_pulses_per_poll_stays_on_the_grid() {
    for step in [3u32, 7, 50, 333] {
        let mut p = ProfileRecorder::default();
        p.reset();
        for c in (0..=5000u32).step_by(step as usize) {
            p.add(c, 42);
        }
        assert!(p.size() <= PROFILE_SAMPLES, "step {step}");
        assert!(p.size() >= 2, "step {step}");
        let v = counts(&p);
        let s = u32::from(p.spacing());
        for i in 0..v.len() {
            // the first poll at or after a grid point: less than one poll step behind it
            assert!(u32::from(v[i]) % s < step, "step {step} i {i}");
            if i > 0 {
                assert!(
                    u32::from(v[i]) / s > u32::from(v[i - 1]) / s,
                    "step {step} i {i}"
                );
            }
        }
        // no grid cell that a poll hit is missing
        for c in (0..=5000u32).step_by(step as usize) {
            let mut found = false;
            for &x in &v {
                found = found || (u32::from(x) / s == c / s);
            }
            if c % s < step {
                assert!(found, "step {step} c {c}");
            }
        }
    }
}

#[test]
fn current_is_stored_as_magnitude_saturated() {
    let mut p = ProfileRecorder::default();
    p.reset();
    p.add(0, -345);
    p.add(1, 70000);
    p.add(2, i32::MIN);
    assert_eq!(p.at(0).current, 345);
    assert_eq!(p.at(1).current, 65535);
    assert_eq!(p.at(2).current, 65535);
}

#[test]
fn counts_saturate_at_65535() {
    let mut p = ProfileRecorder::default();
    p.reset();
    p.add(70000, 1);
    assert_eq!(p.size(), 1);
    assert_eq!(p.at(0).count, 65535);
    p.add(80000, 2); // same saturated count: not a new sampling point
    assert_eq!(p.size(), 1);
}

#[test]
fn decreasing_counts_are_ignored() {
    let mut p = ProfileRecorder::default();
    p.reset();
    p.add(10, 1);
    p.add(5, 2);
    assert_eq!(p.size(), 1);
    p.finish(3, 9);
    assert_eq!(p.size(), 1);
    assert_eq!(p.at(0).current, 1);
}

#[test]
fn finish_appends_the_stop_point() {
    let mut p = ProfileRecorder::default();
    p.reset();
    p.finish(0, 12); // move that never turned
    assert_eq!(p.size(), 1);
    assert_eq!(p.at(0).count, 0);
    assert_eq!(p.at(0).current, 12);

    p.reset();
    for c in 0..5 {
        p.add(c, 100);
    }
    p.finish(4, -520); // same count as the last sample: current replaced
    assert_eq!(p.size(), 5);
    assert_eq!(p.at(4).current, 520);
    p.finish(9, 610);
    assert_eq!(p.size(), 6);
    assert_eq!(p.at(5).count, 9);
    assert_eq!(p.at(5).current, 610);
}

#[test]
fn finish_on_a_full_buffer_compacts_first() {
    let mut p = ProfileRecorder::default();
    p.reset();
    for c in 0..32 {
        p.add(c, 1);
    }
    assert_eq!(p.size(), 32);
    p.finish(40, 700);
    assert_eq!(p.size(), 17);
    assert_eq!(p.at(16).count, 40);
    assert_eq!(p.at(16).current, 700);
    assert_eq!(p.at(15).count, 30);
    assert_eq!(p.spacing(), 2);
}

#[test]
fn spacing_saturates_on_absurdly_long_moves() {
    let mut p = ProfileRecorder::default();
    p.reset();
    // 65535 pulses with 32 samples need a spacing of 4096 at most
    for c in 0..=0xFFFFu32 {
        p.add(c, 1);
    }
    assert!(p.spacing() <= 4096, "spacing {}", p.spacing());
    for _ in 0..40 {
        p.finish(0xFFFF, 2);
    }
    assert!(p.size() <= PROFILE_SAMPLES);
}

#[test]
fn copy_keeps_the_samples() {
    let mut p = ProfileRecorder::default();
    p.reset();
    for c in 0..3 {
        p.add(c, 5);
    }
    let q = p;
    p.reset();
    assert_eq!(q.size(), 3);
    assert_eq!(q.at(2).count, 2);
}

#[test]
fn after_compaction_the_next_grid_point_follows_the_last_kept_sample() {
    let mut p = ProfileRecorder::default();
    p.reset();
    for c in 0..=30 {
        p.add(c, 1);
    }
    p.add(32, 1); // 32 samples, full, last count 32
    assert_eq!(p.size(), 32);
    // 33 is on the spacing-1 grid, but after compaction to spacing 2 the next point is 34
    p.add(33, 2);
    assert_eq!(p.spacing(), 2);
    assert_eq!(p.size(), 17);
    assert_eq!(p.at(16).count, 32);
    p.add(34, 3);
    assert_eq!(p.size(), 18);
    assert_eq!(p.at(17).count, 34);
    assert_eq!(p.at(17).current, 3);
}
