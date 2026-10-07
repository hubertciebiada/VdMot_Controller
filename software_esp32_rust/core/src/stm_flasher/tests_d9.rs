//! Decision D9 (docs/rust/GLUE-DESIGN-STM.md section 8), no C++ counterpart: an image above
//! 16 KiB is erased, written and verified in two passes, sectors 1..n first and sector 0 last,
//! so the old vector table and boot stage survive until the rest of the new image is verified.

use super::rig::*;
use super::*;
use crate::test_support::sim_stm::{SimEvent, SimLcg, WriteRec, BASE, KIB};
use std::cell::Cell;
use std::format;
use std::vec;
use std::vec::Vec;

const S0: usize = 16 * KIB as usize;

fn program(blocks: impl IntoIterator<Item = u32>) -> impl Iterator<Item = SimEvent> {
    blocks
        .into_iter()
        .map(|b| SimEvent::Program(BASE + b * 256))
}

fn read(blocks: impl IntoIterator<Item = u32>) -> impl Iterator<Item = SimEvent> {
    blocks.into_iter().map(|b| SimEvent::Read(BASE + b * 256))
}

/// The events of a successful run of an image of `count` blocks (more than 64): sectors 1..n
/// erased, their blocks written and read back upwards, then sector 0 erased, blocks 1..63 and
/// block 0 written, blocks 0..63 read back.
fn two_pass_events(count: u32, upper_sectors: &[u16]) -> Vec<SimEvent> {
    let mut e = vec![SimEvent::Erase(upper_sectors.to_vec())];
    e.extend(program(64..count));
    e.extend(read(64..count));
    e.push(SimEvent::Erase(vec![0]));
    e.extend(program((1..64).chain([0])));
    e.extend(read(0..64));
    e
}

/// Distinct phases in order (the rig records a phase once per change).
fn phase_order(rig: &Rig) -> Vec<FlashPhase> {
    let mut seen = vec![FlashPhase::Validating];
    for &p in &rig.phases {
        if Some(&p) != seen.last() {
            seen.push(p);
        }
    }
    seen
}

fn sector0_intact(rig: &Rig) -> bool {
    rig.sim.flash[..S0] == rig.sim.original[..S0]
}

fn has_event(rig: &Rig, e: &SimEvent) -> bool {
    rig.sim.events.contains(e)
}

#[test]
fn sector_0_is_erased_written_and_verified_last() {
    let mut rig = Rig::new(make_image(53760)); // 210 blocks, sectors 0..3
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.sim.events, two_pass_events(210, &[1, 2, 3]));
    assert_eq!(
        rig.sim.erase_frames,
        [
            vec![0x00, 0x02, 0x00, 0x01, 0x00, 0x02, 0x00, 0x03, 0x02],
            vec![0x00, 0x00, 0x00, 0x00, 0x00]
        ]
    );
    use FlashPhase::*;
    assert_eq!(
        phase_order(&rig),
        [
            Validating, Resetting, Handshake, Sync, GetId, Erasing, Writing, Verifying, Erasing,
            Writing, Verifying, Starting, WaitingApp, Done
        ]
    );
    assert!(rig.flash_matches_image());
    assert!(rig.percent_monotonic());
    assert_eq!(rig.f.status().attempt, 0);
    assert_eq!(rig.sim.resets.len(), 4); // no extra reset between the passes
}

#[test]
fn the_smallest_two_pass_image_has_one_block_above_sector_0() {
    let mut rig = Rig::new(make_image(S0 + 4)); // 65 blocks, the last one 4 bytes
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(rig.sim.events, two_pass_events(65, &[1]));
    assert_eq!(
        rig.sim.erase_frames,
        [
            vec![0x00, 0x00, 0x00, 0x01, 0x01],
            vec![0x00, 0x00, 0x00, 0x00, 0x00]
        ]
    );
    let first = WriteRec {
        addr: BASE + 0x4000,
        len: 4,
    };
    assert_eq!(rig.sim.programmed[0], first);
    assert!(rig.flash_matches_image());
    assert_eq!(rig.f.status().bytes_done, S0 as u32 + 4);
}

#[test]
fn an_image_of_16_kib_is_flashed_in_one_pass_as_in_cpp() {
    for size in [S0, S0 - 3] {
        let mut rig = Rig::new(make_image(size));
        assert_eq!(rig.begin_and_run(), FlashPhase::Done, "{size}");
        let mut e = vec![SimEvent::Erase(vec![0])];
        e.extend(program((1..64).chain([0])));
        e.extend(read(0..64));
        assert_eq!(rig.sim.events, e, "{size}");
        assert_eq!(rig.sim.erase_frames, [vec![0, 0, 0, 0, 0]]);
        let erasing = phase_order(&rig)
            .iter()
            .filter(|&&p| p == FlashPhase::Erasing)
            .count();
        assert_eq!(erasing, 1);
        assert!(rig.flash_matches_image());
    }
}

#[test]
fn sector_0_keeps_the_old_image_until_the_upper_sectors_are_verified() {
    let mut rig = Rig::new(make_image(40000));
    assert!(rig.begin());
    let mut checked_at_erase = false;
    rig.run_hook(|r| {
        if !has_event(r, &SimEvent::Erase(vec![0])) {
            assert!(sector0_intact(r));
        } else if !checked_at_erase {
            // the step that erased sector 0: the rest of the new image is in place
            checked_at_erase = true;
            let d = &r.img.data;
            assert!(r.sim.flash[S0..d.len()] == d[S0..]);
            assert!(r.sim.flash[..S0].iter().all(|&b| b == 0xFF));
        }
    });
    assert!(checked_at_erase);
    assert_eq!(rig.f.status().phase, FlashPhase::Done);
    assert!(rig.flash_matches_image());
}

#[test]
fn a_verify_failure_above_sector_0_leaves_sector_0_intact() {
    let mut rig = Rig::new(make_image(53760));
    rig.sim.stuck.insert(BASE + 0x5000);
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    let st = rig.f.status();
    assert_eq!(st.error, FlashError::VerifyMismatch);
    assert_eq!(st.error_phase, FlashPhase::Verifying);
    assert_eq!(st.error_address, BASE + 0x5000);
    assert_eq!(st.attempt, 2);
    // three passes of sectors 1..3 (the first one and two retries), sector 0 never erased
    let upper = vec![0x00, 0x02, 0x00, 0x01, 0x00, 0x02, 0x00, 0x03, 0x02];
    assert_eq!(rig.sim.erase_frames, [upper.clone(), upper.clone(), upper]);
    assert!(rig.sim.programmed.iter().all(|p| p.addr >= BASE + 0x4000));
    assert!(sector0_intact(&rig));
    assert!(rig.left_clean());
}

#[test]
fn an_erase_failure_above_sector_0_reports_its_first_address() {
    let mut rig = Rig::new(make_image(53760));
    rig.sim.nack_erase = 100;
    assert_eq!(rig.begin_and_run(), FlashPhase::Failed);
    let st = rig.f.status();
    assert_eq!(st.error, FlashError::Nack);
    assert_eq!(st.error_phase, FlashPhase::Erasing);
    assert_eq!(st.error_address, BASE + 0x4000);
    assert_eq!(st.attempt, 2);
    assert_eq!(rig.sim.writes_equal(&[0x44, 0xBB]), 3);
    assert!(sector0_intact(&rig));
    assert!(rig.left_clean());
}

/// Erasing after the first read-back above sector 0: without faults above sector 0 this is the
/// pass of sector 0, its erase command sent and not answered yet.
fn in_sector0_pass(rig: &Rig) -> bool {
    rig.f.status().phase == FlashPhase::Erasing && has_event(rig, &SimEvent::Read(BASE + 0x4000))
}

#[test]
fn an_erase_failure_of_sector_0_retries_only_sector_0_and_reports_the_flash_base() {
    let mut rig = Rig::new(make_image(53760));
    assert!(rig.begin());
    let mut armed = false;
    rig.run_hook(|r| {
        // the last block above sector 0 is read back: the next command erases sector 0
        if !armed && has_event(r, &SimEvent::Read(BASE + 209 * 256)) {
            r.sim.nack_erase = 100;
            armed = true;
        }
    });
    assert!(armed);
    let st = rig.f.status();
    assert_eq!(st.phase, FlashPhase::Failed);
    assert_eq!(st.error, FlashError::Nack);
    assert_eq!(st.error_phase, FlashPhase::Erasing);
    assert_eq!(st.error_address, BASE);
    assert_eq!(st.attempt, 2);
    // one erase of sectors 1..3, then three refused erase commands for sector 0
    assert_eq!(rig.sim.writes_equal(&[0x44, 0xBB]), 4);
    assert_eq!(rig.sim.erase_frames.len(), 1);
    assert!(sector0_intact(&rig));
    assert!(rig.left_clean());
}

#[test]
fn a_session_retry_in_the_sector_0_pass_repeats_only_that_pass() {
    let mut rig = Rig::new(make_image(53760));
    assert!(rig.begin());
    let mut armed = false;
    rig.run_hook(|r| {
        if !armed && in_sector0_pass(r) {
            r.sim.nack_write_data = 4; // the first block of sector 0 fails 1 + 3 times
            armed = true;
        }
    });
    assert_eq!(rig.f.status().phase, FlashPhase::Done);
    assert_eq!(rig.f.status().attempt, 1);
    let erases: Vec<&SimEvent> = rig
        .sim
        .events
        .iter()
        .filter(|e| matches!(e, SimEvent::Erase(_)))
        .collect();
    assert_eq!(
        erases,
        [
            &SimEvent::Erase(vec![1, 2, 3]),
            &SimEvent::Erase(vec![0]),
            &SimEvent::Erase(vec![0])
        ]
    );
    // the blocks above sector 0 were written once
    for blk in 64..210 {
        let n = rig
            .sim
            .programmed
            .iter()
            .filter(|p| p.addr == BASE + blk * 256)
            .count();
        assert_eq!(n, 1, "{blk}");
    }
    assert_eq!(rig.sim.resets.len(), 4);
    assert!(rig.flash_matches_image());
    assert!(rig.percent_monotonic());
}

#[test]
fn each_pass_has_session_retries_of_its_own() {
    // the first block above sector 0 takes one session retry, then the first block of the
    // sector-0 pass fails 4 times (one retry) or 8 times (two: the whole budget of that pass,
    // which shared with the first pass would have ended the run)
    for sector0_nacks in [4, 8] {
        let mut rig = Rig::new(make_image(53760));
        rig.sim.nack_write_data = 4;
        assert!(rig.begin());
        let mut armed = false;
        let mut upper_attempts = 0;
        rig.run_hook(|r| {
            if !armed && in_sector0_pass(r) {
                r.sim.nack_write_data = sector0_nacks;
                armed = true;
            } else if !armed {
                upper_attempts = upper_attempts.max(r.f.status().attempt);
            }
        });
        assert_eq!(upper_attempts, 1);
        let st = rig.f.status();
        assert_eq!(st.phase, FlashPhase::Done, "{sector0_nacks}");
        assert_eq!(st.attempt, (sector0_nacks / 4) as u8, "{sector0_nacks}");
        assert!(rig.flash_matches_image());
        assert_eq!(rig.sim.resets.len(), 4);
    }
}

#[test]
fn the_upper_pass_checks_its_crc_before_sector_0_is_erased() {
    let mut rig = Rig::new(make_image(53760));
    assert!(rig.begin());
    let mut changed = false;
    rig.run_hook(|r| {
        if !changed && r.f.status().phase == FlashPhase::Verifying {
            // Same change on disk and in flash: bytes compare equal, CRC does not.
            r.img.data[30000] ^= 0x10;
            r.sim.flash[30000] = r.img.data[30000];
            changed = true;
        }
    });
    let st = rig.f.status();
    assert_eq!(st.phase, FlashPhase::Failed);
    assert_eq!(st.error, FlashError::ImageRead);
    assert_eq!(st.error_phase, FlashPhase::Verifying);
    assert_eq!(st.error_address, 0);
    assert_eq!(rig.sim.erase_frames.len(), 1);
    assert!(sector0_intact(&rig));
    assert!(rig.left_clean());
}

#[test]
fn the_bytes_of_sector_0_are_read_and_checked_before_its_erase() {
    // changed in the file during the upper pass, not readable, or no memory for them: the run
    // fails before the erase of sector 0 is even announced, the old sector 0 stays
    for case in 0..3 {
        let mut rig = Rig::new(make_image(53760));
        assert!(rig.begin());
        let mut armed = false;
        rig.run_hook(|r| {
            if !armed && r.f.status().phase == FlashPhase::Writing {
                match case {
                    0 => r.img.data[1500] ^= 0x10,
                    1 => r.img.fail_at = 1500,
                    _ => r.img.fail_hold = true,
                }
                armed = true;
            }
        });
        let st = rig.f.status();
        assert_eq!(st.phase, FlashPhase::Failed, "{case}");
        assert_eq!(st.error, FlashError::ImageRead, "{case}");
        assert_eq!(st.error_phase, FlashPhase::Erasing, "{case}");
        assert_eq!(st.error_address, 0, "{case}");
        assert!(!st.sector0_at_risk, "{case}");
        assert_eq!(rig.img.holds, 1, "{case}");
        assert_eq!(rig.sim.writes_equal(&ERASE_CMD), 1, "{case}");
        assert_eq!(rig.sim.erase_frames.len(), 1, "{case}");
        assert!(sector0_intact(&rig), "{case}");
        assert!(rig.left_clean(), "{case}");
    }
}

#[test]
fn the_sector_0_pass_writes_the_bytes_read_before_its_erase() {
    let original = make_image(53760);
    let mut rig = Rig::new(original.clone());
    assert!(rig.begin());
    let mut changed = false;
    rig.run_hook(|r| {
        if !changed && has_event(r, &SimEvent::Erase(vec![0])) {
            // the file changes after its sector-0 bytes were read and cannot be read any more
            r.img.data[1500] ^= 0x10;
            r.img.fail_at = 1500;
            changed = true;
        }
    });
    assert!(changed);
    assert_eq!(rig.f.status().phase, FlashPhase::Done);
    assert_eq!(rig.img.holds, 1);
    assert!(rig.sim.flash[..original.len()] == original[..]);
}

#[test]
fn an_exhausted_sector_0_pass_waits_in_the_rom_bootloader_without_a_reset() {
    let mut rig = Rig::new(make_image(53760));
    assert!(rig.begin());
    let mut at_erase = None;
    let end = rig.run_to(FlashPhase::Sector0Pending, |r| {
        if at_erase.is_none() && in_sector0_pass(r) {
            r.sim.nack_write_data = 1000; // every write of the pass is refused
            at_erase = Some((r.sim.resets.len(), r.sim.configs.len()));
        }
    });
    let (resets, configs) = at_erase.expect("a sector-0 pass");
    assert_eq!(end, FlashPhase::Sector0Pending);
    let pending_at = rig.now;
    let st = rig.f.status().clone();
    assert!(st.sector0_at_risk);
    assert!(rig.f.sector0_at_risk());
    assert!(rig.f.sector0_pending());
    assert!(rig.f.active());
    assert_eq!(st.error, FlashError::Nack);
    assert_eq!(st.error_phase, FlashPhase::Writing);
    assert_eq!(st.error_address, BASE + 256); // block 1 is the first one of the pass
    assert_eq!(st.attempt, 2);
    assert_eq!(st.finished_ms, 0);
    // the pass and its two retries erased sector 0, no NRST pulse, the UART still at 8E1
    assert_eq!(rig.sim.erase_frames.len(), 4);
    assert_eq!(rig.sim.resets.len(), resets);
    assert_eq!(rig.sim.configs.len(), configs);
    assert_eq!(rig.sim.configs.last(), Some(&(115_200, true)));
    // an abort is ignored, nothing goes out while it waits
    rig.f.abort();
    let writes = rig.sim.writes.len();
    rig.run_with(|_| {}, 29_990, 2);
    assert_eq!(rig.f.status().phase, FlashPhase::Sector0Pending);
    assert_eq!(rig.sim.writes.len(), writes);
    // 30 s after it began to wait the next round erases sector 0 again; the fault is gone
    rig.sim.nack_write_data = 0;
    assert_eq!(rig.run(), FlashPhase::Done);
    assert_eq!(
        rig.sim.writes[writes],
        (pending_at + 30_000, ERASE_CMD.to_vec())
    );
    let st = rig.f.status();
    assert!(!st.sector0_at_risk);
    assert_eq!(st.error, FlashError::None);
    assert_eq!(st.error_phase, FlashPhase::Idle);
    assert_eq!(st.error_address, 0);
    assert_eq!(st.attempt, 0);
    assert!(rig.flash_matches_image());
    assert!(rig.left_clean());
    assert!(rig.percent_monotonic());
    // the pulses into the bootloader and into the new image, no other
    assert_eq!(rig.sim.resets.len(), 4);
}

#[test]
fn retry_sector0_starts_the_next_round_at_once() {
    let mut rig = Rig::new(make_image(53760));
    rig.opt.erase_timeout_ms = 2000;
    assert!(!rig.f.retry_sector0(rig.now)); // nothing runs
    assert!(rig.begin());
    assert!(!rig.f.retry_sector0(rig.now)); // nothing pending
    assert_eq!(rig.f.status().phase, FlashPhase::Validating);
    let mut armed = false;
    rig.run_to(FlashPhase::Sector0Pending, |r| {
        if !armed && in_sector0_pass(r) {
            r.sim.drop_erase_ack = 3; // sector 0 is erased, the ACK never comes
            armed = true;
        }
    });
    let st = rig.f.status();
    assert_eq!(st.phase, FlashPhase::Sector0Pending);
    assert_eq!(st.error, FlashError::Timeout);
    assert_eq!(st.error_phase, FlashPhase::Erasing);
    assert_eq!(st.error_address, BASE);
    rig.run_with(|_| {}, 10_000, 2);
    let writes = rig.sim.writes.len();
    let at = rig.now;
    assert!(rig.f.retry_sector0(at));
    assert_eq!(rig.f.status().phase, FlashPhase::Erasing);
    assert_eq!(rig.f.status().attempt, 0);
    assert_eq!(rig.run(), FlashPhase::Done);
    assert_eq!(rig.sim.writes[writes], (at + 2, ERASE_CMD.to_vec()));
    assert!(rig.flash_matches_image());
}

#[test]
fn a_failed_uart_write_after_the_sector_0_erase_keeps_the_rom_session() {
    let mut rig = Rig::new(make_image(53760));
    assert!(rig.begin());
    let mut resets = None;
    rig.run_to(FlashPhase::Sector0Pending, |r| {
        if resets.is_none() && has_event(r, &SimEvent::Erase(vec![0])) {
            r.sim.write_limit = 3; // the next write command fits, its address frame does not
            resets = Some(r.sim.resets.len());
        }
    });
    let st = rig.f.status();
    assert_eq!(st.phase, FlashPhase::Sector0Pending);
    assert_eq!(st.error, FlashError::TransportWrite);
    assert_eq!(st.error_phase, FlashPhase::Writing);
    assert_eq!(rig.sim.resets.len(), resets.expect("sector 0 erased"));
    rig.sim.write_limit = usize::MAX;
    assert_eq!(rig.run(), FlashPhase::Done);
    assert!(rig.flash_matches_image());
    assert!(rig.left_clean());
}

#[test]
fn a_sector_0_copy_changed_in_ram_is_read_again_before_the_next_erase() {
    // the copy changes during the verify of the sector-0 pass (the same change in flash: the
    // bytes compare equal, the CRC of sector 0 does not); with the file intact the next round
    // reads it again, with the file changed too the round waits again without an erase
    for file_changed in [false, true] {
        let mut rig = Rig::new(make_image(53760));
        assert!(rig.begin());
        let mut changed = false;
        rig.run_to(FlashPhase::Sector0Pending, |r| {
            let verifying = r.f.status().phase == FlashPhase::Verifying;
            if !changed && verifying && has_event(r, &SimEvent::Erase(vec![0])) {
                r.img.held[1500] ^= 0x10;
                r.sim.flash[1500] = r.img.held[1500];
                changed = true;
            }
        });
        let st = rig.f.status();
        assert_eq!(st.phase, FlashPhase::Sector0Pending, "{file_changed}");
        assert_eq!(st.error, FlashError::ImageRead);
        assert_eq!(st.error_phase, FlashPhase::Verifying);
        assert_eq!(rig.img.holds, 1);
        let erases = rig.sim.erase_frames.len();
        assert_eq!(erases, 2);
        if file_changed {
            rig.img.data[1500] ^= 0x10;
            rig.run_with(|_| {}, 30_100, 2);
            let st = rig.f.status();
            assert_eq!(st.phase, FlashPhase::Sector0Pending);
            assert_eq!(st.error, FlashError::ImageRead);
            assert_eq!(st.error_phase, FlashPhase::Erasing);
            assert_eq!(rig.img.holds, 2);
            assert_eq!(rig.sim.erase_frames.len(), erases);
            rig.img.data[1500] ^= 0x10; // the file is back
        }
        assert_eq!(rig.run(), FlashPhase::Done, "{file_changed}");
        assert_eq!(rig.img.holds, if file_changed { 3 } else { 2 });
        assert!(rig.flash_matches_image());
    }
}

#[test]
fn an_abort_is_taken_up_to_the_erase_of_sector_0_and_ignored_after_it() {
    // up to the erase command of sector 0 an abort resets the STM into its old sector 0; once
    // the sector list went out it is ignored and the run ends with the new image
    for after_erase in [false, true] {
        let mut rig = Rig::new(make_image(53760));
        assert!(rig.begin());
        let mut aborted = false;
        rig.run_hook(|r| {
            let due = if after_erase {
                has_event(r, &SimEvent::Erase(vec![0]))
            } else {
                in_sector0_pass(r)
            };
            if !aborted && due {
                assert_eq!(r.f.sector0_at_risk(), after_erase);
                r.f.abort();
                aborted = true;
            }
        });
        assert!(aborted);
        let st = rig.f.status();
        if after_erase {
            assert_eq!(st.phase, FlashPhase::Done);
            assert_eq!(st.error, FlashError::None);
            assert!(rig.flash_matches_image());
        } else {
            assert_eq!(st.phase, FlashPhase::Failed);
            assert_eq!(st.error, FlashError::Aborted);
            assert_eq!(st.error_phase, FlashPhase::Erasing);
            assert!(sector0_intact(&rig));
        }
        assert!(rig.left_clean(), "{after_erase}");
    }
}

#[test]
fn percent_and_byte_counters_run_over_both_passes() {
    // 53760 bytes: 37376 above sector 0, 16384 in it.
    let mut rig = Rig::new(make_image(53760));
    assert!(rig.begin());
    // per pass (0 upper, 1 sector 0) and phase: lowest/highest percent and bytes_done
    let mut pct = [[(255u8, 0u8); 3]; 2];
    let mut bytes = [[(u32::MAX, 0u32); 3]; 2];
    let mut pass = 0;
    let mut prev = FlashPhase::Idle;
    rig.run_hook(|r| {
        let st = r.f.status();
        if st.phase == FlashPhase::Erasing && prev == FlashPhase::Verifying {
            pass = 1;
        }
        prev = st.phase;
        let k = match st.phase {
            FlashPhase::Erasing => 0,
            FlashPhase::Writing => 1,
            FlashPhase::Verifying => 2,
            _ => return,
        };
        let p = &mut pct[pass][k];
        *p = (p.0.min(st.percent), p.1.max(st.percent));
        let b = &mut bytes[pass][k];
        *b = (b.0.min(st.bytes_done), b.1.max(st.bytes_done));
    });
    assert_eq!(rig.f.status().phase, FlashPhase::Done);
    assert!(rig.percent_monotonic());
    // Writing 15 + 60 * bytes / 53760, Verifying 75 + 20 * bytes / 53760 over both passes
    let at = |base: u32, span: u32, bytes: u32| (base + span * bytes / 53760) as u8;
    assert_eq!(pct[0][0], (5, 5));
    assert_eq!(pct[0][1], (15, at(15, 60, 37120)));
    assert_eq!(at(15, 60, 37120), 56);
    assert_eq!(bytes[0][1], (0, 37120));
    assert_eq!(pct[0][2], (75, at(75, 20, 37120)));
    assert_eq!(at(75, 20, 37376), 88);
    assert_eq!(bytes[0][2], (0, 37120));
    // sector 0: the percent waits where the first verify left it (88) until its verify
    // passes it; the byte counters go on from the 37376 bytes of the first pass
    assert_eq!(pct[1][0], (88, 88));
    assert_eq!(bytes[1][0], (37376, 37376));
    assert_eq!(pct[1][1], (88, 88));
    assert_eq!(bytes[1][1], (37376, 53760 - 256));
    assert_eq!(pct[1][2], (88, at(75, 20, 53760 - 256)));
    assert_eq!(at(75, 20, 53760 - 256), 94);
    assert_eq!(bytes[1][2], (37376, 53760 - 256));
    assert_eq!(rig.f.status().bytes_done, 53760);
}

#[test]
fn abort_in_the_upper_pass_leaves_sector_0_intact() {
    let mut rig = Rig::new(make_image(53760));
    assert!(rig.begin());
    let mut aborted = false;
    rig.run_hook(|r| {
        if !aborted && r.sim.programmed.len() == 10 {
            r.f.abort();
            aborted = true;
        }
    });
    let st = rig.f.status();
    assert_eq!(st.phase, FlashPhase::Failed);
    assert_eq!(st.error, FlashError::Aborted);
    assert_eq!(st.error_phase, FlashPhase::Writing);
    assert!(sector0_intact(&rig));
    assert!(rig.left_clean());
}

#[test]
fn blank_mode_writes_sector_0_last_too() {
    let mut rig = Rig::new(make_image(53760));
    rig.opt.blank = true;
    rig.sim.has_boot_loop = false;
    rig.sim.boot_pin_resets = 100;
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert!(rig.f.status().manual_reset);
    assert_eq!(rig.sim.events, two_pass_events(210, &[1, 2, 3]));
    assert!(rig.flash_matches_image());
}

#[test]
fn the_largest_image_keeps_sector_0_for_last() {
    let mut rig = Rig::new(make_image(512 * KIB as usize));
    assert_eq!(rig.begin_and_run(), FlashPhase::Done);
    assert_eq!(
        rig.sim.events,
        two_pass_events(2048, &[1, 2, 3, 4, 5, 6, 7])
    );
}

#[test]
fn fault_fuzz_never_touches_sector_0_before_the_rest_is_verified() {
    let mut r = SimLcg::new(0xD9);
    let (mut done, mut failed, mut pending) = (0, 0, 0);
    for iter in 0..60u32 {
        let size = S0 + 4 + 4 * r.below(6000) as usize;
        let mut rig = Rig::new(make_image_seeded(
            size,
            0x2002_0000,
            BASE + 1,
            true,
            "1.4.9_Dev",
            iter + 1,
        ));
        rig.opt.ack_timeout_ms = 200;
        rig.opt.erase_timeout_ms = 2000;
        rig.sim.nack_erase = r.below(3) as i32;
        rig.sim.drop_erase_ack = r.below(2) as i32;
        rig.sim.nack_write_data = r.below(6) as i32;
        rig.sim.drop_write_ack = r.below(3) as i32;
        rig.sim.corrupt_reads = r.below(3) as i32;
        rig.sim.corrupt_offset = 0;
        rig.sim.noise_replies = r.below(20) as i32;
        rig.sim.erase_delay_ms = 50 + r.below(500);
        if r.below(6) == 0 {
            rig.sim.stuck.insert(BASE + r.below(size as u32));
        }
        // faults that start with the pass of sector 0
        let late_nacks = r.below(6) as i32;
        let late_corrupt = r.below(3) as i32;
        assert!(rig.begin());
        let armed = Cell::new(false);
        let checked_at_erase = Cell::new(false);
        // resets and UART configurations when sector 0 was erased
        let at_erase: Cell<Option<(usize, usize)>> = Cell::new(None);
        let mut hook = |g: &mut Rig| {
            if !armed.get() && in_sector0_pass(g) {
                g.sim.nack_write_data += late_nacks;
                g.sim.corrupt_reads += late_corrupt;
                armed.set(true);
            }
            if !has_event(g, &SimEvent::Erase(vec![0])) {
                assert!(sector0_intact(g), "{iter}");
            } else if !checked_at_erase.get() {
                checked_at_erase.set(true);
                at_erase.set(Some((g.sim.resets.len(), g.sim.configs.len())));
                let d = &g.img.data;
                assert!(g.sim.flash[S0..d.len()] == d[S0..], "{iter}");
            }
        };
        let mut end = rig.run_to(FlashPhase::Sector0Pending, &mut hook);
        if end == FlashPhase::Sector0Pending {
            // no reset and no UART change since sector 0 was erased: the STM waits in its
            // bootloader; once the faults are gone a request ends the run
            pending += 1;
            assert!(rig.f.sector0_at_risk(), "{iter}");
            let (resets, configs) = at_erase.get().expect("sector 0 erased");
            assert_eq!(rig.sim.resets.len(), resets, "{iter}");
            assert_eq!(rig.sim.configs.len(), configs, "{iter}");
            rig.sim.nack_write_data = 0;
            rig.sim.drop_write_ack = 0;
            rig.sim.corrupt_reads = 0;
            rig.sim.noise_replies = 0;
            rig.sim.drop_erase_ack = 0;
            rig.sim.nack_erase = 0;
            rig.sim.stuck.clear();
            assert!(rig.f.retry_sector0(rig.now), "{iter}");
            end = rig.run_hook(&mut hook);
            assert_eq!(end, FlashPhase::Done, "{iter}");
        }
        let st = rig.f.status();
        let why = format!(
            "{iter}: {} in {}",
            flash_error_name(st.error),
            flash_phase_name(st.error_phase)
        );
        assert_eq!(end, st.phase, "{why}");
        assert!(rig.left_clean(), "{why}");
        assert!(rig.percent_monotonic(), "{why}");
        assert_eq!(st.finished_ms, rig.now, "{why}");
        let events = &rig.sim.events;
        let s0 = events
            .iter()
            .position(|e| *e == SimEvent::Erase(vec![0]))
            .unwrap_or(events.len());
        for (i, e) in events.iter().enumerate() {
            match e {
                SimEvent::Erase(list) if i > s0 => assert_eq!(list, &[0], "{why}"),
                SimEvent::Erase(list) if i < s0 => assert!(!list.contains(&0), "{why}"),
                SimEvent::Program(a) if i < s0 => assert!(*a >= BASE + 0x4000, "{why}"),
                SimEvent::Program(a) | SimEvent::Read(a) if i > s0 => {
                    assert!(*a < BASE + 0x4000, "{why}")
                }
                _ => {}
            }
        }
        // block 0 is the last block of every pass of sector 0 that wrote it
        for pass in events[s0.min(events.len())..].split(|e| *e == SimEvent::Erase(vec![0])) {
            let programs: Vec<u32> = pass
                .iter()
                .filter_map(|e| match e {
                    SimEvent::Program(a) => Some(*a),
                    _ => None,
                })
                .collect();
            if let Some(i) = programs.iter().position(|&a| a == BASE) {
                assert_eq!(i, programs.len() - 1, "{why}");
            }
        }
        if st.phase == FlashPhase::Done {
            done += 1;
            assert!(rig.flash_matches_image(), "{why}");
        } else {
            // only a run that never erased sector 0 resets the STM after a failure
            failed += 1;
            assert_ne!(st.error, FlashError::None, "{why}");
            assert_eq!(s0, events.len(), "{why}");
        }
    }
    assert!(done > 0);
    assert!(failed > 0);
    assert!(pending > 0);
}
