//! Port of test/native/test_motor_params.cpp.

use super::*;

/// the C++ aggregate initializer MotorParams{lowFac, highFac, startOnPower, minCounts, maxRetries}
fn mp(
    low_fac: u8,
    high_fac: u8,
    start_on_power: u8,
    min_counts: u16,
    max_retries: u8,
) -> MotorParams {
    MotorParams {
        low_fac,
        high_fac,
        start_on_power,
        min_counts,
        max_retries,
    }
}

fn valid() -> MotorParams {
    mp(17, 23, 40, 3000, 1)
}

fn same(a: &MotorParams, b: &MotorParams) -> bool {
    same_motor_params(a, b)
}

#[test]
fn param_range_contains_is_inclusive() {
    let r = ParamRange {
        min: 5,
        max: 50,
        def: 17,
    };
    assert!(!r.contains(4));
    assert!(r.contains(5));
    assert!(r.contains(50));
    assert!(!r.contains(51));
    assert!(!r.contains(0xFFFF_FFFF));
}

#[test]
fn range_table_matches_the_1_x_start_up_limits_and_defaults() {
    assert_eq!(LOW_FAC_RANGE.min, 10);
    assert_eq!(LOW_FAC_RANGE.max, 40);
    assert_eq!(HIGH_FAC_RANGE.min, 10);
    assert_eq!(HIGH_FAC_RANGE.max, 40);
    assert_eq!(START_ON_POWER_RANGE.max, 100);
    assert_eq!(MIN_COUNTS_RANGE.max, 60000);
    assert_eq!(MAX_RETRIES_RANGE.max, 2);
    let d = MOTOR_PARAMS_DEFAULT;
    assert!(same(&d, &mp(17, 17, 30, 3000, 2)));
    assert!(motor_params_valid(&d));
}

#[test]
fn motor_params_valid_checks_every_field_at_both_ends() {
    assert!(motor_params_valid(&valid()));
    assert!(motor_params_valid(&mp(10, 10, 0, 0, 0)));
    assert!(motor_params_valid(&mp(40, 40, 100, 60000, 2)));

    let mut p = valid();
    p.low_fac = 9;
    assert!(!motor_params_valid(&p));
    p = valid();
    p.low_fac = 41;
    assert!(!motor_params_valid(&p));
    p = valid();
    p.high_fac = 9;
    assert!(!motor_params_valid(&p));
    p = valid();
    p.high_fac = 41;
    assert!(!motor_params_valid(&p));
    p = valid();
    p.start_on_power = 101;
    assert!(!motor_params_valid(&p));
    p = valid();
    p.min_counts = 60001;
    assert!(!motor_params_valid(&p));
    p = valid();
    p.max_retries = 3;
    assert!(!motor_params_valid(&p));
}

#[test]
fn sanitize_motor_params_replaces_only_out_of_range_fields() {
    assert!(same(&sanitize_motor_params(&valid()), &valid()));

    // erased EEPROM
    let erased = mp(0xFF, 0xFF, 0xFF, 0xFFFF, 0xFF);
    assert!(same(&sanitize_motor_params(&erased), &MOTOR_PARAMS_DEFAULT));

    // 1.x smotc stored 5..9 and 41..50 but 1.x dropped them at the next start;
    // an upgrade must not bring them into effect (factor 8 would stop every move)
    for f in [5u8, 8, 9, 41, 50] {
        let r = sanitize_motor_params(&mp(f, f, 30, 3000, 2));
        assert_eq!(r.low_fac, 17, "f {f}");
        assert_eq!(r.high_fac, 17, "f {f}");
    }
    assert_eq!(sanitize_motor_params(&mp(10, 40, 30, 3000, 2)).low_fac, 10);
    assert_eq!(sanitize_motor_params(&mp(10, 40, 30, 3000, 2)).high_fac, 40);

    let mixed = mp(4, 30, 101, 60000, 3);
    let s = sanitize_motor_params(&mixed);
    assert_eq!(s.low_fac, 17);
    assert_eq!(s.high_fac, 30);
    assert_eq!(s.start_on_power, 30);
    assert_eq!(s.min_counts, 60000);
    assert_eq!(s.max_retries, 2);

    assert_eq!(sanitize_motor_params(&mp(17, 51, 0, 60001, 0)).high_fac, 17);
    assert_eq!(
        sanitize_motor_params(&mp(17, 51, 0, 60001, 0)).start_on_power,
        0
    );
    assert_eq!(
        sanitize_motor_params(&mp(17, 51, 0, 60001, 0)).min_counts,
        3000
    );
    assert_eq!(
        sanitize_motor_params(&mp(17, 51, 0, 60001, 0)).max_retries,
        0
    );
}

#[test]
fn apply_motor_params_request_three_mandatory_values() {
    let mut p = valid();
    let v = [10, 20, 55, 0, 0];
    assert_eq!(
        apply_motor_params_request(&mut p, 3, &v),
        ParamsRequest::Applied
    );
    assert!(same(&p, &mp(10, 20, 55, 3000, 1)));
}

#[test]
fn apply_motor_params_request_optional_min_counts_and_retries() {
    let mut p = valid();
    let v4 = [10, 20, 55, 100, 99];
    assert_eq!(
        apply_motor_params_request(&mut p, 4, &v4),
        ParamsRequest::Applied
    );
    assert!(same(&p, &mp(10, 20, 55, 100, 1)));

    let v5 = [10, 40, 100, 60000, 2];
    assert_eq!(
        apply_motor_params_request(&mut p, 5, &v5),
        ParamsRequest::Applied
    );
    assert!(same(&p, &mp(10, 40, 100, 60000, 2)));

    let v0 = [10, 10, 0, 0, 0];
    assert_eq!(
        apply_motor_params_request(&mut p, 5, &v0),
        ParamsRequest::Applied
    );
    assert!(same(&p, &mp(10, 10, 0, 0, 0)));
}

#[test]
fn apply_motor_params_request_an_out_of_range_value_keeps_its_field_the_others_are_applied() {
    // (v, expected); valid() is {17, 23, 40, 3000, 1}; the in-range values of each request
    // are 18, 24, 50, 2500, 2
    let cases: [([u32; 5], MotorParams); 12] = [
        ([9, 24, 50, 2500, 2], mp(17, 24, 50, 2500, 2)),
        ([51, 24, 50, 2500, 2], mp(17, 24, 50, 2500, 2)), // above what the legacy web page offers
        ([5, 24, 50, 2500, 2], mp(17, 24, 50, 2500, 2)),  // legacy web page: factor 0.5
        ([18, 9, 50, 2500, 2], mp(18, 23, 50, 2500, 2)),
        ([18, 51, 50, 2500, 2], mp(18, 23, 50, 2500, 2)),
        ([18, 24, 101, 2500, 2], mp(18, 24, 40, 2500, 2)),
        ([18, 24, 50, 60001, 2], mp(18, 24, 50, 3000, 2)),
        ([18, 24, 50, 2500, 3], mp(18, 24, 50, 2500, 1)),
        ([0xFFFF_FFFF, 24, 50, 2500, 2], mp(17, 24, 50, 2500, 2)),
        ([8, 60, 50, 2500, 2], mp(17, 23, 50, 2500, 2)), // both factors out of range
        ([8, 45, 50, 2500, 2], mp(17, 40, 50, 2500, 2)), // 4.5 is applied as 4.0, 0.8 is not
        ([0, 0, 200, 70000, 9], mp(17, 23, 40, 3000, 1)), // nothing in range
    ];
    for (v, expected) in &cases {
        let mut p = valid();
        assert_eq!(
            apply_motor_params_request(&mut p, 5, v),
            ParamsRequest::Partial,
            "{v:?}"
        );
        assert!(same(&p, expected), "{v:?}: {p:?}");
    }
}

#[test]
fn apply_motor_params_request_factors_41_50_of_the_legacy_web_page_are_applied_as_40() {
    // review finding: the legacy ESP sends smotc 17 45 30 3000 2 for close factor 4.5
    assert_eq!(FAC_REQUEST_MAX, 50);
    for f in [41u32, 45, 50] {
        let mut p = valid();
        let v = [f, f, 30, 3000, 2];
        assert_eq!(
            apply_motor_params_request(&mut p, 5, &v),
            ParamsRequest::Applied,
            "f {f}"
        );
        assert!(same(&p, &mp(40, 40, 30, 3000, 2)), "f {f}");
        assert!(motor_params_valid(&p), "f {f}");
    }
    let mut p = valid();
    let low = [50, 17, 30, 0, 0];
    assert_eq!(
        apply_motor_params_request(&mut p, 3, &low),
        ParamsRequest::Applied
    );
    assert_eq!(p.low_fac, 40);
    assert_eq!(p.high_fac, 17);
    // 51 and above are still refused, the EEPROM load keeps loading the default
    let high = [17, 51, 30, 0, 0];
    assert_eq!(
        apply_motor_params_request(&mut p, 3, &high),
        ParamsRequest::Partial
    );
    assert_eq!(p.high_fac, 17);
    assert_eq!(sanitize_motor_params(&mp(45, 45, 30, 3000, 2)).high_fac, 17);
}

#[test]
fn apply_motor_params_request_legacy_esp_saves_factor_0_8_together_with_a_new_start() {
    // smotc 8 17 60 3000 2: the factor stays, start % and the rest are taken as by 1.x
    let mut p = mp(17, 17, 30, 3000, 2);
    let v = [8, 17, 60, 2500, 0];
    assert_eq!(
        apply_motor_params_request(&mut p, 5, &v),
        ParamsRequest::Partial
    );
    assert!(same(&p, &mp(17, 17, 60, 2500, 0)));
}

#[test]
fn apply_motor_params_request_unused_optional_values_are_not_checked() {
    let mut p = valid();
    let v = [17, 17, 50, 99999, 99];
    assert_eq!(
        apply_motor_params_request(&mut p, 3, &v),
        ParamsRequest::Applied
    );
    assert_eq!(p.min_counts, 3000);
    assert_eq!(p.max_retries, 1);

    let w = [17, 17, 50, 400, 99];
    assert_eq!(
        apply_motor_params_request(&mut p, 4, &w),
        ParamsRequest::Applied
    );
    assert_eq!(p.min_counts, 400);
}

#[test]
fn apply_motor_params_request_argument_count_outside_3_5_applies_nothing() {
    let v = [18, 18, 50, 3000, 2];
    for argc in [0u8, 1, 2, 6, 255] {
        let mut p = valid();
        assert_eq!(
            apply_motor_params_request(&mut p, argc, &v),
            ParamsRequest::Rejected,
            "argc {argc}"
        );
        assert!(same(&p, &valid()), "argc {argc}");
    }
}

#[test]
fn same_motor_params_compares_every_field() {
    let a = valid();
    assert!(same_motor_params(&a, &a));
    let mut b = a;
    b.low_fac += 1;
    assert!(!same_motor_params(&a, &b));
    b = a;
    b.high_fac += 1;
    assert!(!same_motor_params(&a, &b));
    b = a;
    b.start_on_power += 1;
    assert!(!same_motor_params(&a, &b));
    b = a;
    b.min_counts += 1;
    assert!(!same_motor_params(&a, &b));
    b = a;
    b.max_retries += 1;
    assert!(!same_motor_params(&a, &b));
}
