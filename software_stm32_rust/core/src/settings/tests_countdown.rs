//! Port of test/native/test_settings__countdown.cpp: countdown and effective_learn_time (S3,
//! C-7).

use super::*;

#[test]
fn countdown_fires_when_the_rest_is_not_more_than_the_elapsed_time_subtracts_otherwise() {
    let mut rest: u32 = 30;
    assert!(!countdown(&mut rest, 10));
    assert_eq!(rest, 20);
    assert!(!countdown(&mut rest, 19));
    assert_eq!(rest, 1);
    assert!(countdown(&mut rest, 1));
    rest = 5;
    assert!(countdown(&mut rest, 11));
    assert_eq!(rest, 5); // the caller reloads
    rest = 0;
    assert!(countdown(&mut rest, 0));
    rest = 7;
    assert!(!countdown(&mut rest, 0));
    assert_eq!(rest, 7);
}

#[test]
fn effective_learn_time_a_stored_0_is_honoured_only_with_a_recent_lease_client() {
    assert_eq!(LEARN_TIME_CLIENT_WINDOW_S, 86400);
    assert_eq!(effective_learn_time(0, true), 0);
    assert_eq!(effective_learn_time(0, false), 604_800);
    assert_eq!(effective_learn_time(3600, false), 3600);
    assert_eq!(effective_learn_time(3600, true), 3600);
    assert_eq!(effective_learn_time(1, false), 1);
}
