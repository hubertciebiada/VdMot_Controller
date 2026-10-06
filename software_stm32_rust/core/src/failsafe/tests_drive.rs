//! Port of test/native/test_failsafe__drive.cpp: drive_target, where a valve is driven to and
//! why (contracts 2.5).

use super::*;
use std::format;

fn check_drive(
    target: u8,
    fs: u8,
    status: u8,
    expired: bool,
    hold: bool,
    pos: u8,
    src: DriveSource,
) {
    let d = drive_target(target, fs, status, expired, hold);
    let ctx = format!("target {target} fs {fs} status {status} expired {expired} hold {hold}");
    assert_eq!(d.position, pos, "{ctx}");
    assert_eq!(d.source, src, "{ctx}");
}

#[test]
fn drive_target_truth_table_over_status_failsafe_lease_and_assembly_hold() {
    assert_eq!(DriveSource::Target as i32, 0);
    assert_eq!(DriveSource::LeaseFailsafe as i32, 1);
    assert_eq!(DriveSource::BlockedFailsafe as i32, 2);
    let statuses: [u8; 5] = [1, 4, 6, 8, 9];
    let positions: [u8; 4] = [0, 50, 100, 255];
    for status in statuses {
        for fs in positions {
            for e in 0..2 {
                for h in 0..2 {
                    let expired = e != 0;
                    let hold = h != 0;
                    if fs == 255 {
                        check_drive(30, fs, status, expired, hold, 30, DriveSource::Target);
                    } else if status == 9 {
                        check_drive(
                            30,
                            fs,
                            status,
                            expired,
                            hold,
                            fs,
                            DriveSource::BlockedFailsafe,
                        );
                    } else if status == 4 || status == 6 {
                        check_drive(30, fs, status, expired, hold, 30, DriveSource::Target);
                    } else if expired && !hold {
                        check_drive(
                            30,
                            fs,
                            status,
                            expired,
                            hold,
                            fs,
                            DriveSource::LeaseFailsafe,
                        );
                    } else {
                        check_drive(30, fs, status, expired, hold, 30, DriveSource::Target);
                    }
                }
            }
        }
    }
    // the other states follow the lease like idle
    check_drive(70, 20, 2, true, false, 20, DriveSource::LeaseFailsafe);
    check_drive(70, 20, 3, true, false, 20, DriveSource::LeaseFailsafe);
    check_drive(70, 20, 5, true, false, 20, DriveSource::LeaseFailsafe);
    check_drive(70, 20, 7, true, true, 70, DriveSource::Target);
    check_drive(70, 20, 7, false, false, 70, DriveSource::Target);
    check_drive(70, 254, 9, false, false, 254, DriveSource::BlockedFailsafe);
}
