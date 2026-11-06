//! The flow layer (Rust 2.2.8): configuration rules, blob and JSON forms, the curve mapping
//! both ways and the positioning state machine against a valve model fed with gvlvy data.

use std::string::String;
use std::vec;
use std::vec::Vec;

use super::*;
use crate::stm_codec::{MoveResult, ValveEx, STM_FLAG_ASSEMBLY, STM_FLAG_SVC_HOLD};
use crate::valve_model::TargetSource;

/// An invented curve: 630 pulses without flow, 2.3 l/min at 1070, qmax 2.6; 37 % of the stroke is
/// 888 pulses.
fn curve() -> FlowConfig {
    let mut c = FlowConfig {
        enabled: true,
        count: 5,
        qmax_ml: 2600,
        stroke_counts: 2400,
        ..FlowConfig::DEFAULT
    };
    c.points[0] = FlowPoint {
        counts: 630,
        flow_ml: 0,
    };
    c.points[1] = FlowPoint {
        counts: 760,
        flow_ml: 300,
    };
    c.points[2] = FlowPoint {
        counts: 888,
        flow_ml: 1000,
    };
    c.points[3] = FlowPoint {
        counts: 1070,
        flow_ml: 2300,
    };
    c.points[4] = FlowPoint {
        counts: 1180,
        flow_ml: 2600,
    };
    c
}

/// The first positioning of an open reference: the end stop, the closing move of `close`, the
/// keep.
fn position_open(stm: &mut Stm, f: &mut FlowLayer, v: u8, close: u16) {
    assert_eq!(stm.next(f), mv(v, MoveDir::Open, 10000, 30));
    stm.result(f, v, FlowOutcome::Ok);
    stm.tick(f);
    stm.moved(v, MoveDir::Open, 10000, 4000, StopReason::EndStop);
    stm.tick(f);
    assert_eq!(stm.next(f), mv(v, MoveDir::Close, close, 34));
    stm.result(f, v, FlowOutcome::Ok);
    stm.tick(f);
    stm.moved(v, MoveDir::Close, close, close, StopReason::Target);
    stm.tick(f);
    assert_eq!(stm.next(f), keep(v, true));
    stm.result(f, v, FlowOutcome::Ok);
    stm.flags(v, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
}

/// The same loop measured from the open end stop: 4641 pulses of stroke.
fn open_curve() -> FlowConfig {
    let mut c = curve();
    c.ref_side = RefSide::Open;
    c.stroke_counts = 4641;
    c
}

/// A desired target in %.
fn served(pct: u8) -> Option<Served> {
    Some(Served { pct, flow_ml: None })
}

fn configs(c: &FlowConfig, valves: &[u8]) -> [FlowConfig; 12] {
    let mut all = [FlowConfig::DEFAULT; 12];
    for &v in valves {
        all[usize::from(v)] = *c;
    }
    all
}

fn json(c: &FlowConfig) -> String {
    let mut buf = [0u8; 1024];
    let mut jw = JsonWriter::new(&mut buf);
    write_flow_config_json(&mut jw, c);
    assert!(jw.ok());
    String::from_utf8(jw.as_bytes().to_vec()).expect("ASCII")
}

fn patched(c: &FlowConfig, body: &str) -> Result<FlowConfig, FlowError> {
    let mut out = *c;
    patch_flow_config_json(&mut out, body.as_bytes()).map(|()| out)
}

// ================================================================ configuration

#[test]
fn defaults_disabled_2_7_lpm_34_30_ma_fallback_15_60_s_dead_band_0_05() {
    let c = FlowConfig::default();
    assert_eq!(c, FlowConfig::DEFAULT);
    assert!(!c.enabled);
    assert_eq!(c.qmax_ml, 2700);
    assert_eq!((c.ref_ma, c.open_ma, c.fallback_pct), (34, 30, 15));
    assert_eq!((c.min_interval_s, c.deadband_ml), (60, 50));
    assert_eq!((c.offset, c.gain_milli), (0, 1000));
    assert_eq!(c.ref_side, RefSide::Closed);
    assert_eq!(c.stroke_counts, 0);
    assert!(!c.pre_close);
    assert_eq!((c.lift_counts, c.floor_counts), (0, 0));
    assert_eq!(c.count, 0);
    assert!(c.points().is_empty());
    assert_eq!(validate_flow_config(&c), Ok(()));
    // The limits.
    assert_eq!(FLOW_POINTS_MIN, 2);
    assert_eq!((FLOW_POINTS_MAX, FLOW_V1_POINTS), (32, 16));
    assert_eq!(core::mem::size_of::<FlowConfig>(), 152);
    assert_eq!(FLOW_COUNTS_MAX, 10000);
    assert_eq!((FLOW_QMAX_MIN_ML, FLOW_QMAX_MAX_ML), (10, 20000));
    assert_eq!(FLOW_DEADBAND_MAX_ML, 5000);
    assert_eq!(FLOW_INTERVAL_MAX_S, 3600);
    assert_eq!(FLOW_OFFSET_MAX, 10000);
    assert_eq!((FLOW_GAIN_MIN_MILLI, FLOW_GAIN_MAX_MILLI), (100, 10000));
    assert_eq!((FLOW_MA_MIN, FLOW_MA_MAX), (5, 60));
    assert_eq!((FLOW_LIFT_MAX, LIFT_MIN, LIFT_SAME_COUNTS), (1000, 60, 2));
    // event 417: -1 no reply, -2 timeout, -3 R2 at an end stop again
    assert_eq!(R2_END_STOP_AGAIN, -3);
    assert_eq!(RefSide::from_raw(0), Some(RefSide::Closed));
    assert_eq!(RefSide::from_raw(1), Some(RefSide::Open));
    assert_eq!(RefSide::from_raw(2), None);
    assert_eq!(ref_side_name(RefSide::Closed), "closed");
    assert_eq!(ref_side_name(RefSide::Open), "open");
}

#[test]
fn validation_every_rule_with_its_error() {
    let c = curve();
    assert_eq!(validate_flow_config(&c), Ok(()));
    type Case = (fn(&mut FlowConfig), FlowError);
    let cases: Vec<Case> = vec![
        (|c| c.qmax_ml = 9, FlowError::Qmax),
        (|c| c.qmax_ml = 20001, FlowError::Qmax),
        (|c| c.ref_ma = 4, FlowError::RefMa),
        (|c| c.ref_ma = 61, FlowError::RefMa),
        (|c| c.open_ma = 4, FlowError::OpenMa),
        (|c| c.open_ma = 61, FlowError::OpenMa),
        (|c| c.fallback_pct = 101, FlowError::FallbackPct),
        (|c| c.min_interval_s = 3601, FlowError::MinInterval),
        (|c| c.deadband_ml = 5001, FlowError::Deadband),
        (|c| c.offset = 10001, FlowError::Offset),
        (|c| c.offset = -10001, FlowError::Offset),
        (|c| c.gain_milli = 99, FlowError::Gain),
        (|c| c.gain_milli = 10001, FlowError::Gain),
        (|c| c.count = 1, FlowError::PointCount),
        (|c| c.count = 33, FlowError::PointCount),
        (|c| c.count = 17, FlowError::PointCounts),
        (|c| c.count = 0, FlowError::PointCount),
        (|c| c.points[2].counts = 760, FlowError::PointCounts),
        (|c| c.points[2].counts = 700, FlowError::PointCounts),
        (|c| c.points[4].counts = 10001, FlowError::PointCounts),
        (|c| c.points[2].flow_ml = 249, FlowError::PointFlow),
        (|c| c.lift_counts = 1001, FlowError::LiftCounts),
        (|c| c.lift_counts = 59, FlowError::LiftCounts),
        (|c| c.lift_counts = 1, FlowError::LiftCounts),
        (|c| c.floor_counts = 10001, FlowError::FloorCounts),
        (|c| c.floor_counts = 2400, FlowError::FloorCounts),
        (|c| c.lift_counts = 60, FlowError::LiftRefSide),
    ];
    for (i, (change, err)) in cases.into_iter().enumerate() {
        let mut bad = curve();
        change(&mut bad);
        assert_eq!(validate_flow_config(&bad), Err(err), "case {i}");
    }
    // The boundaries hold.
    let mut ok = curve();
    ok.qmax_ml = 10;
    ok.ref_ma = 5;
    ok.open_ma = 60;
    ok.fallback_pct = 100;
    ok.min_interval_s = 3600;
    ok.deadband_ml = 5000;
    ok.offset = -10000;
    ok.gain_milli = 100;
    ok.points[4].counts = 10000;
    ok.points[2].flow_ml = 300; // equal flows are allowed
    ok.floor_counts = 2399; // below the stroke of 2400
    assert_eq!(validate_flow_config(&ok), Ok(()));
    ok.offset = 10000;
    ok.gain_milli = 10000;
    ok.qmax_ml = 20000;
    assert_eq!(validate_flow_config(&ok), Ok(()));
    // No points while disabled; 2 points, the first at 0, are fine.
    let mut none = curve();
    none.enabled = false;
    none.count = 0;
    assert_eq!(validate_flow_config(&none), Ok(()));
    let mut two = curve();
    two.count = 2;
    two.points[0].counts = 0;
    assert_eq!(validate_flow_config(&two), Ok(()));
    // 32 points.
    let mut full = curve();
    full.count = 32;
    for (i, p) in full.points.iter_mut().enumerate() {
        *p = FlowPoint {
            counts: 100 * (i as u16 + 1),
            flow_ml: 100 * i as u16,
        };
    }
    assert_eq!(validate_flow_config(&full), Ok(()));
    assert_eq!(full.points().len(), 32);
    // An open reference and an enabled layer need the stroke; it is bounded on both sides.
    let open = open_curve();
    assert_eq!(validate_flow_config(&open), Ok(()));
    let mut no_stroke = open;
    no_stroke.stroke_counts = 0;
    assert_eq!(
        validate_flow_config(&no_stroke),
        Err(FlowError::StrokeCounts)
    );
    no_stroke.enabled = false;
    assert_eq!(
        validate_flow_config(&no_stroke),
        Err(FlowError::StrokeCounts)
    );
    let mut closed = curve();
    closed.stroke_counts = 0;
    assert_eq!(validate_flow_config(&closed), Err(FlowError::StrokeCounts));
    closed.enabled = false;
    assert_eq!(validate_flow_config(&closed), Ok(()));
    closed.enabled = true;
    closed.stroke_counts = 1;
    assert_eq!(validate_flow_config(&closed), Ok(()));
    let mut long = curve();
    long.stroke_counts = 10001;
    assert_eq!(validate_flow_config(&long), Err(FlowError::StrokeCounts));
    long.stroke_counts = 10000;
    assert_eq!(validate_flow_config(&long), Ok(()));
    // Without a stroke the floor is bounded by the range alone.
    closed.enabled = false;
    closed.stroke_counts = 0;
    closed.floor_counts = 10000;
    assert_eq!(validate_flow_config(&closed), Ok(()));
    closed.floor_counts = 10001;
    assert_eq!(validate_flow_config(&closed), Err(FlowError::FloorCounts));
    // A lift: 60..1000 with the open reference and a floor below the stroke.
    let mut r2 = open_curve();
    r2.floor_counts = 4640;
    assert_eq!(validate_flow_config(&r2), Ok(()));
    for lift in [60, 1000] {
        r2.lift_counts = lift;
        assert_eq!(validate_flow_config(&r2), Ok(()));
    }
    r2.floor_counts = 4641;
    assert_eq!(validate_flow_config(&r2), Err(FlowError::FloorCounts));
    r2.floor_counts = 0;
    assert_eq!(validate_flow_config(&r2), Err(FlowError::LiftFloor));
    r2.floor_counts = 1;
    assert_eq!(validate_flow_config(&r2), Ok(()));
    r2.ref_side = RefSide::Closed;
    assert_eq!(validate_flow_config(&r2), Err(FlowError::LiftRefSide));
}

#[test]
fn error_texts_name_the_key_and_its_range() {
    let all = [
        (FlowError::Malformed, "malformed JSON"),
        (FlowError::UnknownKey, "unknown key"),
        (FlowError::Enabled, "enabled true|false"),
        (FlowError::Qmax, "qmax 0.01..20 l/min"),
        (FlowError::RefMa, "refmA 5..60"),
        (FlowError::OpenMa, "openmA 5..60"),
        (FlowError::FallbackPct, "fallbackPct 0..100"),
        (FlowError::MinInterval, "minIntervalS 0..3600"),
        (FlowError::Deadband, "deadbandLpm 0..5"),
        (FlowError::Offset, "offset -10000..10000"),
        (FlowError::Gain, "gain 0.1..10"),
        (FlowError::Points, "points [[counts,lpm],...]"),
        (FlowError::PointCount, "points 2..32 (enabled needs them)"),
        (FlowError::PointCounts, "points counts 0..10000, increasing"),
        (FlowError::PointFlow, "points lpm 0..65.535, non-decreasing"),
        (FlowError::RefSide, "refSide closed|open"),
        (FlowError::PreClose, "preClose true|false"),
        (
            FlowError::StrokeCounts,
            "strokeCounts 0..10000 (enabled or refSide open needs 1..)",
        ),
        (FlowError::Lpm, "lpm a plain number from 0"),
        (FlowError::LiftCounts, "liftCounts 0 or 60..1000"),
        (
            FlowError::FloorCounts,
            "floorCounts 0..10000, below strokeCounts",
        ),
        (
            FlowError::LiftRefSide,
            "liftCounts above 0 needs refSide open",
        ),
        (
            FlowError::LiftFloor,
            "liftCounts above 0 needs floorCounts above 0",
        ),
    ];
    for (i, (e, text)) in all.iter().enumerate() {
        assert_eq!(*e as usize, i);
        assert_eq!(flow_error_text(*e), *text);
    }
}

// ================================================================ mapping

#[test]
fn mapping_closed_reference_first_position_where_the_curve_reaches_the_flow() {
    let c = curve();
    // Below the first flow: the first counts; on the segments: linear; above: the last.
    assert_eq!(curve_counts(&c, 0), 630);
    assert_eq!(curve_counts(&c, 300), 760);
    // Rust 2.2.15: between the point without flow and the first with one, that point.
    assert_eq!(curve_counts(&c, 150), 760);
    assert_eq!(curve_counts(&c, 1), 760);
    assert_eq!(curve_counts(&c, 299), 760);
    assert_eq!(curve_counts(&c, 301), 760); // 760 + 1/700 x 128 = 760.18
    assert_eq!(curve_counts(&c, 304), 761); // 760.73
    assert_eq!(curve_counts(&c, 1000), 888);
    assert_eq!(curve_counts(&c, 1025), 892); // 888 + 25/1300 x 182 = 891.5 -> 892
    assert_eq!(curve_counts(&c, 2600), 1180);
    assert_eq!(curve_counts(&c, 5000), 1180);
    // A flat segment: its first position.
    let mut flat = c;
    flat.points[2].flow_ml = 300;
    assert_eq!(curve_counts(&flat, 300), 760);
    assert_eq!(curve_counts(&flat, 301), 888); // 888 + 1/2000 x 182 -> 888.09 -> 888
    assert_eq!(curve_counts(&FlowConfig::DEFAULT, 100), 0);
    // The flow targets: 0 is closed, otherwise the corrected counts.
    assert_eq!(flow_counts(&c, 0), 0);
    assert_eq!(flow_counts(&c, 27), 760); // below the first flow
    assert_eq!(flow_counts(&c, 999), 888); // 887.82
    assert_eq!(flow_counts(&c, 2600), 1180);
    let mut corrected = c;
    corrected.offset = -9;
    corrected.gain_milli = 1010;
    assert_eq!(flow_counts(&corrected, 999), 888); // 888 x 1.01 = 896.88 -> 897 - 9
    corrected.offset = -10000;
    assert_eq!(flow_counts(&corrected, 999), 1); // never below 1
    corrected.offset = 10000;
    assert_eq!(flow_counts(&corrected, 999), 10000); // never above 10000
                                                     // qmax is the limit, whatever the curve goes on to.
    let mut low = c;
    low.qmax_ml = 1000;
    assert_eq!(flow_counts(&low, 2500), 888);
    assert_eq!(flow_counts(&low, 1000), 888);
    assert_eq!(flow_counts(&low, 999), 888);
}

#[test]
fn a_target_is_a_percentage_of_the_stroke_and_the_curve_has_no_part_in_it() {
    let mut c = curve();
    c.stroke_counts = 4641;
    assert_eq!(target_counts(&c, 0), 0);
    assert_eq!(target_counts(&c, 1), 46);
    assert_eq!(target_counts(&c, 37), 1717); // 1717.17
    assert_eq!(target_counts(&c, 38), 1764); // 1763.58
    assert_eq!(target_counts(&c, 50), 2321); // 2320.5
    assert_eq!(target_counts(&c, 100), 4641);
    c.stroke_counts = 10000;
    assert_eq!(target_counts(&c, 100), 10000);
    assert_eq!(target_counts(&c, 7), 700);
    let mut corrected = curve();
    corrected.offset = -9;
    corrected.gain_milli = 1010;
    assert_eq!(target_counts(&corrected, 37), 888);
    corrected.count = 0;
    assert_eq!(target_counts(&corrected, 37), 888);
    assert_eq!(target_counts(&FlowConfig::DEFAULT, 37), 0);
}

#[test]
fn a_position_in_percent_of_the_stroke_is_rounded_and_ends_at_100() {
    let mut c = curve();
    c.stroke_counts = 4641;
    assert_eq!(stroke_pct(&c, 0), 0);
    assert_eq!(stroke_pct(&c, 23), 0); // 0.496
    assert_eq!(stroke_pct(&c, 24), 1); // 0.517
    assert_eq!(stroke_pct(&c, 1717), 37);
    assert_eq!(stroke_pct(&c, 4641), 100);
    assert_eq!(stroke_pct(&c, 10000), 100); // past the stroke
    assert_eq!(stroke_pct(&FlowConfig::DEFAULT, 500), 0);
    for stroke in [100u16, 2400, 4170, 4950, 10000] {
        c.stroke_counts = stroke;
        for pct in 0..=100u8 {
            let counts = target_counts(&c, pct);
            assert_eq!(stroke_pct(&c, counts), pct, "{stroke} {pct}");
        }
    }
}

#[test]
fn mapping_closed_reference_inverse_flow() {
    let c = curve();
    assert_eq!(curve_flow(&c, 0), 0);
    assert_eq!(curve_flow(&c, 630), 0);
    assert_eq!(curve_flow(&c, 695), 150);
    assert_eq!(curve_flow(&c, 760), 300);
    assert_eq!(curve_flow(&c, 992), 1743); // 1000 + 104/182 x 1300 = 1742.9
    assert_eq!(curve_flow(&c, 1180), 2600);
    assert_eq!(curve_flow(&c, 5000), 2600);
    assert_eq!(curve_flow(&FlowConfig::DEFAULT, 500), 0);
    assert_eq!(counts_flow(&c, 0), 0);
    assert_eq!(counts_flow(&c, 888), 1000);
    assert_eq!(counts_flow(&c, 1), 0);
    let mut corrected = c;
    corrected.offset = -9;
    corrected.gain_milli = 1010;
    assert_eq!(counts_flow(&corrected, 888), 1000); // (888 + 9) / 1.01 = 888.1 -> 888
    corrected.offset = 2000;
    assert_eq!(counts_flow(&corrected, 100), 0); // behind the offset: the curve's start
}

#[test]
fn mapping_the_reference_side_changes_nothing_the_curve_counts_from_the_closed_stop() {
    let (c, mut o) = (curve(), open_curve());
    o.stroke_counts = c.stroke_counts;
    for pct in 0..=100u8 {
        assert_eq!(target_counts(&o, pct), target_counts(&c, pct), "{pct}");
    }
    for counts in [0u16, 1, 630, 700, 888, 1180, 4641, 10000] {
        assert_eq!(counts_flow(&o, counts), counts_flow(&c, counts), "{counts}");
    }
    assert_eq!(target_counts(&o, 37), 888);
    assert_eq!(flow_counts(&o, 999), 888);
}

// ================================================================ blob

/// The blob as Rust 2.2.8 .. 2.2.13 wrote it, built by hand.
fn blob_2_2_13(all: &[FlowConfig; 12]) -> Vec<u8> {
    let mut b = b"VDMF\x01\x0C".to_vec();
    for c in all {
        b.push(u8::from(c.enabled));
        b.extend(c.qmax_ml.to_le_bytes());
        b.extend([c.ref_ma, c.open_ma, c.fallback_pct]);
        b.extend(c.min_interval_s.to_le_bytes());
        b.extend(c.deadband_ml.to_le_bytes());
        b.extend(c.offset.to_le_bytes());
        b.extend(c.gain_milli.to_le_bytes());
        b.extend([c.ref_side as u8, u8::from(c.pre_close)]);
        b.extend(c.stroke_counts.to_le_bytes());
        b.push(c.count);
        for p in &c.points[..16] {
            b.extend(p.counts.to_le_bytes());
            b.extend(p.flow_ml.to_le_bytes());
        }
    }
    let crc = crc32(&b, 0);
    b.extend(crc.to_le_bytes());
    b
}

/// A set of every kind of record: curves on 0 and 11, an open reference with the warm-up and an
/// offset on 5; a lift and a floor on 5, a floor alone on 11.
fn blob_set() -> [FlowConfig; 12] {
    let mut all = configs(&curve(), &[0, 11]);
    all[5] = open_curve();
    all[5].pre_close = true;
    all[5].offset = -17;
    all[5].lift_counts = 100;
    all[5].floor_counts = 0x0302;
    all[11].floor_counts = 640;
    all
}

/// The set without its lifts and floors.
fn without_lifts(mut all: [FlowConfig; 12]) -> [FlowConfig; 12] {
    for c in &mut all {
        c.lift_counts = 0;
        c.floor_counts = 0;
    }
    all
}

/// The CRC of `b` (the last 4 bytes) made to fit the rest again.
fn fix_crc(b: &mut [u8]) {
    let crc_at = b.len() - 4;
    let crc = crc32(&b[..crc_at], 0);
    b[crc_at..].copy_from_slice(&crc.to_le_bytes());
}

#[test]
fn blob_is_the_blob_of_2_2_13_byte_for_byte_round_trip_and_rejections() {
    assert_eq!(FLOW_RECORD_SIZE, 83);
    assert_eq!(FLOW_BLOB_SIZE, 1006);
    let all = blob_set();
    let mut bytes = [0xEE; FLOW_BLOB_SIZE];
    assert_eq!(encode_flow_configs(&all, &mut bytes), FLOW_BLOB_SIZE);
    // What Rust 2.2.13 wrote and reads: the lifts and floors are not in it.
    assert_eq!(bytes.as_slice(), blob_2_2_13(&all).as_slice());
    assert_eq!(&bytes[..6], b"VDMF\x01\x0C");
    // valve 0: enabled, 2600 = 0x0A28, 34, 30, 15, 60 = 0x003C, 50, 0, 1000 = 0x03E8,
    // closed, no warm-up, 2400 = 0x0960, 5 points, the first 630 = 0x0276 at 0.
    assert_eq!(
        &bytes[6..29],
        &[
            1, 0x28, 0x0A, 34, 30, 15, 0x3C, 0x00, 50, 0, 0, 0, 0xE8, 0x03, 0, 0, 0x60, 0x09, 5,
            0x76, 0x02, 0, 0
        ]
    );
    let valve5 = 6 + 5 * FLOW_RECORD_SIZE;
    assert_eq!(bytes[valve5 + 14], 1); // open
    assert_eq!(bytes[valve5 + 15], 1); // preClose
    assert_eq!(&bytes[valve5 + 16..valve5 + 18], &4641u16.to_le_bytes()); // strokeCounts
    assert_eq!(&bytes[valve5 + 10..valve5 + 12], &(-17i16).to_le_bytes());
    // 2.2.14 reads it with no lift and no floor, as it reads the blob of 2.2.13.
    let mut back = blob_set();
    assert!(decode_flow_configs(&bytes, &mut back));
    assert_eq!(back, without_lifts(all));
    // Rejections: the length, each header byte alone (with a CRC that fits it), the CRC, a bad
    // record (enabled 2, refSide 2, preClose 2, refmA 4, count 1) on valve 0 and on valve 11.
    let rejected = |b: &[u8], what: &str| {
        let mut out = configs(&curve(), &[3]);
        assert!(!decode_flow_configs(b, &mut out), "{what}");
        assert_eq!(out, [FlowConfig::DEFAULT; 12], "{what}");
    };
    rejected(&bytes[..FLOW_BLOB_SIZE - 1], "short");
    let mut long = bytes.to_vec();
    long.push(0);
    rejected(&long, "long");
    for (at, value, what) in [(0, b'X', "magic"), (4, 2, "version"), (5, 11, "count")] {
        let mut bad = bytes;
        bad[at] = value;
        fix_crc(&mut bad);
        rejected(&bad, what);
    }
    let mut bad = bytes;
    bad[FLOW_BLOB_SIZE - 1] ^= 1;
    rejected(&bad, "crc");
    for (at, value) in [(0, 2u8), (14, 2), (15, 2), (3, 4), (18, 1), (18, 17)] {
        for valve in [0, 11] {
            let mut bad = bytes;
            bad[6 + valve * FLOW_RECORD_SIZE + at] = value;
            fix_crc(&mut bad);
            rejected(&bad, &std::format!("byte {at} of valve {valve}"));
        }
    }
    // Every valve at the defaults decodes to the defaults.
    let all = [FlowConfig::DEFAULT; 12];
    assert_eq!(encode_flow_configs(&all, &mut bytes), FLOW_BLOB_SIZE);
    let mut out = configs(&curve(), &[3]);
    assert!(decode_flow_configs(&bytes, &mut out));
    assert_eq!(out, all);
}

#[test]
fn a_blob_enabled_without_a_stroke_comes_back_disabled_with_the_rest_of_the_set() {
    let mut all = configs(&curve(), &[0, 1, 2]);
    all[1].stroke_counts = 0;
    all[2].count = 0;
    let mut bytes = [0; FLOW_BLOB_SIZE];
    encode_flow_configs(&all, &mut bytes);
    let mut out = [FlowConfig::DEFAULT; 12];
    // No points: that is no valid record, as ever.
    assert!(!decode_flow_configs(&bytes, &mut out));
    all[2].count = 5;
    encode_flow_configs(&all, &mut bytes);
    assert!(decode_flow_configs(&bytes, &mut out));
    assert_eq!(out[0], all[0]);
    assert_eq!(out[2], all[2]);
    assert!(!out[1].enabled);
    assert_eq!(out[1].points(), all[1].points());
}

/// The set as the `flow` blob gives it back, the lifts and floors of `lifts` added.
fn with_lifts(lifts: &[u8]) -> ([FlowConfig; 12], bool) {
    let mut bytes = [0; FLOW_BLOB_SIZE];
    encode_flow_configs(&blob_set(), &mut bytes);
    let mut out = [FlowConfig::DEFAULT; 12];
    assert!(decode_flow_configs(&bytes, &mut out));
    let ok = decode_flow_lifts(lifts, &mut out);
    (out, ok)
}

#[test]
fn the_lifts_and_floors_are_a_key_of_their_own_round_trip_and_rejections() {
    assert_eq!(FLOW_LIFT_BLOB_SIZE, 58);
    let all = blob_set();
    let mut bytes = [0xEE; FLOW_LIFT_BLOB_SIZE];
    encode_flow_lifts(&all, &mut bytes);
    assert_eq!(&bytes[..6], b"VDML\x01\x0C");
    // valve 0 none, valve 5: 100 and 0x0302, valve 11: none and 640 = 0x0280
    assert_eq!(&bytes[6..10], &[0, 0, 0, 0]);
    assert_eq!(&bytes[6 + 5 * 4..6 + 6 * 4], &[100, 0, 0x02, 0x03]);
    assert_eq!(&bytes[6 + 11 * 4..6 + 12 * 4], &[0, 0, 0x80, 0x02]);
    assert_eq!(
        &bytes[54..],
        &crc32(&bytes[..54], 0).to_le_bytes(),
        "the CRC of the rest"
    );
    assert_eq!(with_lifts(&bytes), (all, true));
    // Rejections: no key, the length, each header byte alone (with a CRC that fits it), the
    // CRC: every lift and floor 0, the rest of the set as the `flow` blob gave it.
    let rejected = |b: &[u8], what: &str| {
        assert_eq!(with_lifts(b), (without_lifts(all), false), "{what}");
    };
    rejected(&[], "no key");
    rejected(&bytes[..FLOW_LIFT_BLOB_SIZE - 1], "short");
    let mut long = bytes.to_vec();
    long.push(0);
    rejected(&long, "long");
    for (at, value, what) in [(0, b'X', "magic"), (4, 2, "version"), (5, 11, "count")] {
        let mut bad = bytes;
        bad[at] = value;
        fix_crc(&mut bad);
        rejected(&bad, what);
    }
    let mut bad = bytes;
    bad[FLOW_LIFT_BLOB_SIZE - 1] ^= 1;
    rejected(&bad, "crc");
    // A key read onto a set that has lifts already: they are replaced, not kept.
    let mut out = all;
    encode_flow_lifts(&without_lifts(all), &mut bytes);
    assert!(decode_flow_lifts(&bytes, &mut out));
    assert_eq!(out, without_lifts(all));
}

#[test]
fn a_pair_that_would_break_its_valve_is_left_out_the_others_are_read() {
    // Written by 2.2.14, read after an older version changed the `flow` blob (another stroke,
    // another reference side) or written with values no version allows.
    let mut pairs = blob_set();
    pairs[0].lift_counts = 60; // valve 0 has the closed reference
    pairs[0].floor_counts = 700;
    pairs[2].floor_counts = 1; // valve 2 has the defaults: a floor alone is fine
    pairs[11].lift_counts = 0;
    let mut bytes = [0; FLOW_LIFT_BLOB_SIZE];
    encode_flow_lifts(&pairs, &mut bytes);
    let (out, ok) = with_lifts(&bytes);
    assert!(ok);
    assert_eq!((out[0].lift_counts, out[0].floor_counts), (0, 0));
    assert_eq!((out[2].lift_counts, out[2].floor_counts), (0, 1));
    assert_eq!((out[5].lift_counts, out[5].floor_counts), (100, 0x0302));
    assert_eq!((out[11].lift_counts, out[11].floor_counts), (0, 640));
    // A lift below 60, a lift without a floor, a floor at the stroke: valve 5 only.
    for (lift, floor) in [(59, 770), (100, 0), (100, 4641), (1001, 770)] {
        let mut pairs = blob_set();
        pairs[5].lift_counts = lift;
        pairs[5].floor_counts = floor;
        encode_flow_lifts(&pairs, &mut bytes);
        let (out, ok) = with_lifts(&bytes);
        assert!(ok);
        assert_eq!(
            (out[5].lift_counts, out[5].floor_counts),
            (0, 0),
            "{lift} {floor}"
        );
        assert_eq!(out[11].floor_counts, 640);
        assert_eq!(out[5], without_lifts(blob_set())[5]);
    }
}

/// The indices [`v1_index`] keeps of a curve of `n` points.
fn kept(n: usize) -> Vec<usize> {
    (0..FLOW_V1_POINTS).map(|k| v1_index(n, k)).collect()
}

#[test]
fn flow_keeps_16_points_of_a_longer_curve_the_first_the_last_and_14_evenly_spaced() {
    // Up to 16 points: every cell as it is (the cells behind the points too).
    for n in 0..=16 {
        assert_eq!(kept(n), (0..16).collect::<Vec<_>>(), "{n}");
    }
    assert_eq!(
        kept(17),
        [0, 1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13, 14, 15, 16]
    );
    assert_eq!(
        kept(24),
        [0, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18, 20, 21, 23]
    );
    assert_eq!(
        kept(32),
        [0, 2, 4, 6, 8, 10, 12, 14, 17, 19, 21, 23, 25, 27, 29, 31]
    );
    for n in 17..=32 {
        let k = kept(n);
        assert_eq!((k[0], k[15]), (0, n - 1), "{n}");
        assert!(k.windows(2).all(|w| w[0] < w[1]), "{n}");
    }
}

/// The set of [`blob_set`] with a curve of 24 points on valve 3 (0.5 .. 2.7 l/min in steps of
/// 0.1 after [0, 0]) and one of 32 on valve 7 (the open reference with a lift).
fn long_set() -> [FlowConfig; 12] {
    let mut all = blob_set();
    all[3] = step_curve(24);
    all[7] = step_curve(32);
    all[7].ref_side = RefSide::Open;
    all[7].stroke_counts = 4641;
    all[7].lift_counts = 100;
    all[7].floor_counts = 1000;
    all
}

/// The set as NVS gives it back: `flow`, `flowlift` and `flowpts` (`points`, `None` without the
/// key) decoded, with what [`decode_flow_points`] returned.
fn decoded(
    flow: &[u8],
    all: &[FlowConfig; 12],
    points: Option<&[u8]>,
) -> ([FlowConfig; 12], (bool, u16)) {
    let mut lifts = [0; FLOW_LIFT_BLOB_SIZE];
    encode_flow_lifts(all, &mut lifts);
    let mut out = [FlowConfig::DEFAULT; 12];
    assert!(decode_flow_configs(flow, &mut out));
    assert!(decode_flow_lifts(&lifts, &mut out));
    let found = decode_flow_points(points, &mut out);
    (out, found)
}

/// The `flow` blob of `all`.
fn flow_of(all: &[FlowConfig; 12]) -> [u8; FLOW_BLOB_SIZE] {
    let mut flow = [0; FLOW_BLOB_SIZE];
    encode_flow_configs(all, &mut flow);
    flow
}

/// The `flowpts` blob of `all` (empty without a curve of more than 16 points).
fn points_of(all: &[FlowConfig; 12]) -> Vec<u8> {
    let mut buf = [0; FLOW_POINTS_BLOB_MAX];
    let n = encode_flow_points(all, &mut buf);
    buf[..n].to_vec()
}

/// One bit for each of these valves.
fn valves(v: &[usize]) -> u16 {
    v.iter().fold(0, |mask, &v| mask | 1 << v)
}

/// `all` as its `flow` blob holds it: 16 points of a longer curve.
fn in_flow(all: &[FlowConfig; 12]) -> [FlowConfig; 12] {
    let mut out = *all;
    for c in &mut out {
        if c.points().len() > FLOW_V1_POINTS {
            let cells = v1_cells(c);
            c.points = [FlowPoint::default(); FLOW_POINTS_MAX];
            c.points[..FLOW_V1_POINTS].copy_from_slice(&cells);
            c.count = 16;
        }
    }
    out
}

#[test]
fn a_curve_of_more_than_16_points_flow_keeps_16_of_them_flowpts_all_of_them() {
    let all = long_set();
    let mut flow = [0; FLOW_BLOB_SIZE];
    encode_flow_configs(&all, &mut flow);
    // `flow` as a version before 2.2.15 reads it: 16 points of the longer curves, valid there.
    let mut old = [FlowConfig::DEFAULT; 12];
    assert!(decode_flow_configs(&flow, &mut old));
    let v3 = 6 + 3 * FLOW_RECORD_SIZE;
    assert_eq!(flow[v3 + 18], 16);
    assert_eq!(&flow[v3 + 19..v3 + 23], &[0, 0, 0, 0]); // [0, 0]
    assert_eq!(&flow[v3 + 23..v3 + 27], &[0x38, 0x04, 0x58, 0x02]); // point 2: [1080, 0.6]
    assert_eq!(old[3].count, 16);
    let picked: Vec<FlowPoint> = kept(24).iter().map(|&i| all[3].points[i]).collect();
    assert_eq!(old[3].points(), picked.as_slice());
    assert_eq!(old[7].points()[15], all[7].points[31]);
    assert_eq!(old, without_lifts(in_flow(&all)));
    for c in &old {
        assert_eq!(validate_flow_config(c), Ok(()));
    }
    // `flowpts`: the CRC of every record of `flow`, valve 3 with 24 points, valve 7 with 32, the
    // rest none.
    let mut points = [0xEE; FLOW_POINTS_BLOB_MAX];
    let n = encode_flow_points(&all, &mut points);
    assert_eq!((FLOW_POINTS_BLOB_MAX, n), (1606, 6 + 48 + 12 + 4 * 56 + 4));
    assert_eq!(&points[..6], b"VDMP\x02\x0C");
    for (v, record) in flow[6..6 + 12 * FLOW_RECORD_SIZE]
        .chunks(FLOW_RECORD_SIZE)
        .enumerate()
    {
        assert_eq!(
            &points[6 + 4 * v..10 + 4 * v],
            &crc32(record, 0).to_le_bytes(),
            "{v}"
        );
    }
    assert_eq!(&points[54..66], &[0, 0, 0, 24, 0, 0, 0, 32, 0, 0, 0, 0]);
    assert_eq!(&points[66..74], &[0, 0, 0, 0, 0x2E, 0x04, 0xF4, 0x01]); // [0,0], [1070,0.5]
    let v7 = 66 + 4 * 24;
    assert_eq!(&points[v7 + 4..v7 + 8], &[0x2E, 0x04, 0xF4, 0x01]);
    assert_eq!(&points[n - 8..n - 4], &[0x5A, 0x05, 0xAC, 0x0D]); // [1370, 3.5]
    assert_eq!(&points[n - 4..n], &crc32(&points[..n - 4], 0).to_le_bytes());
    // Read back: the whole set, lifts and all; nothing may be cut.
    assert_eq!(decoded(&flow, &all, Some(&points[..n])), (all, (true, 0)));
    // No longer curve: no blob.
    assert_eq!(encode_flow_points(&blob_set(), &mut points), 0);
    assert_eq!(encode_flow_points(&in_flow(&all), &mut points), 0);
}

#[test]
fn flowpts_rejections_leave_the_16_points_of_flow_which_may_be_cut() {
    let all = long_set();
    let flow = flow_of(&all);
    let good = points_of(&all);
    let n = good.len();
    let cut = valves(&[3, 7]);
    let rejected = |b: &[u8], what: &str| {
        assert_eq!(
            decoded(&flow, &all, Some(b)),
            (in_flow(&all), (false, cut)),
            "{what}"
        );
    };
    // No key: nothing may be cut, since a save writes `flowpts` before `flow`.
    assert_eq!(decoded(&flow, &all, None), (in_flow(&all), (false, 0)));
    rejected(&[], "unreadable");
    rejected(&good[..3], "no CRC");
    rejected(&good[..21], "no records");
    rejected(&good[..60], "no numbers of points");
    rejected(&good[..n - 1], "short");
    let mut long = good.clone();
    long.push(0);
    rejected(&long, "long");
    let mut fixed_long = long.clone();
    fix_crc(&mut fixed_long);
    rejected(&fixed_long, "long with its CRC");
    for (at, value, what) in [
        (0, b'X', "magic"),
        (4, 1, "version 1"),
        (4, 3, "version 3"),
        (5, 11, "count"),
    ] {
        let mut bad = good.clone();
        bad[at] = value;
        fix_crc(&mut bad);
        rejected(&bad, what);
    }
    let mut bad = good.clone();
    bad[n - 1] ^= 1;
    rejected(&bad, "crc");
    // A number of points of 33 or 1, the length right for the sum of them.
    for (counts, what) in [([(3, 33), (7, 23)], "33"), ([(0, 1), (3, 23)], "1")] {
        let mut bad = good.clone();
        for (valve, count) in counts {
            bad[54 + valve] = count;
        }
        fix_crc(&mut bad);
        rejected(&bad, what);
    }
    // 16 points of valve 3 (the last 8 of its 24 cut out) next to a good valve 7: refused whole,
    // valve 7 too.
    let mut bad = good.clone();
    bad[54 + 3] = 16;
    bad.drain(66 + 16 * 4..66 + 24 * 4);
    fix_crc(&mut bad);
    rejected(&bad, "16");
    // No curve at all, the records of `flow` named: nothing changes, and the 16 points of valves 3
    // and 7 are their whole curves.
    let mut none = good[..54].to_vec();
    none.extend([0; 12]);
    none.extend(crc32(&none, 0).to_le_bytes());
    assert_eq!(
        decoded(&flow, &all, Some(&none)),
        (in_flow(&all), (true, 0))
    );
    // The same naming other records: those 16 points may be cut.
    none[6..54].fill(0);
    fix_crc(&mut none);
    assert_eq!(
        decoded(&flow, &all, Some(&none)),
        (in_flow(&all), (true, cut))
    );
}

#[test]
fn a_curve_that_flow_no_longer_holds_or_the_rules_refuse_stays_as_flow_has_it() {
    let all = long_set();
    let points = points_of(&all);
    // A version before 2.2.15 saved another curve on valve 3: that one stays, valve 7 has its 32.
    let mut older = all;
    older[3] = curve();
    let (out, found) = decoded(&flow_of(&older), &all, Some(&points));
    assert_eq!(found, (true, 0));
    assert_eq!(out[3], curve());
    assert_eq!(out[7], all[7]);
    // One of 16 points: they may be 16 of a longer curve.
    older[3] = step_curve(16);
    let (out, found) = decoded(&flow_of(&older), &all, Some(&points));
    assert_eq!(found, (true, valves(&[3])));
    assert_eq!(out[3], step_curve(16));
    assert_eq!(out[7], all[7]);
    // The 16 points it keeps, but 15 of them in `flow`: those 15.
    let mut flow = flow_of(&all);
    flow[6 + 3 * FLOW_RECORD_SIZE + 18] = 15;
    fix_crc(&mut flow);
    let (out, found) = decoded(&flow, &all, Some(&points));
    assert_eq!(found, (true, 0));
    assert_eq!(out[3].points(), &in_flow(&all)[3].points()[..15]);
    assert_eq!(out[7], all[7]);
    // A curve whose points are not increasing where `flow` does not look: refused alone, and the
    // record the blob names keeps 16 of its points.
    let mut bad = all;
    bad[3].points[1].counts = 0;
    let (out, found) = decoded(&flow_of(&all), &all, Some(&points_of(&bad)));
    assert_eq!(found, (true, valves(&[3])));
    assert_eq!(out[3], in_flow(&all)[3]);
    assert_eq!(out[7], all[7]);
}

#[test]
fn flowpts_of_a_save_that_did_not_write_flow_the_curves_in_ram_come_back() {
    // The save wrote `flowpts` of the new set and stopped before `flow`.
    let old = long_set();
    let mut new = old;
    new[3] = step_curve(20);
    new[7] = step_curve(16);
    new[0].enabled = false;
    let (out, found) = decoded(&flow_of(&old), &old, Some(&points_of(&new)));
    // Valves 3 and 7 keep 16 points of their old curves, which may be cut; valve 0 keeps its 5.
    assert_eq!(found, (true, valves(&[3, 7])));
    assert_eq!(out, in_flow(&old));
    // RAM still holds the old set: its curves come back where `flow` holds 16 points of them.
    let mut back = out;
    assert_eq!(keep_held_curves(&points_of(&old), &mut back, found.1), 0);
    assert_eq!(back, old);
    // RAM with the new set: 16 points of valve 3 of another curve, none of valve 7.
    let mut back = out;
    assert_eq!(
        keep_held_curves(&points_of(&new), &mut back, found.1),
        valves(&[3, 7])
    );
    assert_eq!(back, out);
    // Valve 7 only, its bit alone in the mask: the bit of valve 3 is not set by it.
    let mut ram = old;
    ram[3] = new[3];
    let mut back = out;
    assert_eq!(
        keep_held_curves(&points_of(&ram), &mut back, valves(&[7])),
        0
    );
    assert_eq!(back[7], old[7]);
    assert_eq!(back[3], out[3]);
}

#[test]
fn a_curve_of_24_or_32_points_every_flow_of_a_point_is_that_point() {
    for n in [24, 32] {
        let mut c = step_curve(n);
        c.qmax_ml = 3500;
        assert_eq!(validate_flow_config(&c), Ok(()));
        for p in &c.points()[1..] {
            let ml = u32::from(p.flow_ml);
            assert_eq!(flow_counts(&c, ml), p.counts, "{n} {ml}");
            assert_eq!(
                flow_target(&c, ml),
                FlowTarget {
                    ml: p.flow_ml,
                    pct: stroke_pct(&c, p.counts),
                    flow_ml: ml,
                    clipped: false
                },
                "{n} {ml}"
            );
        }
    }
    // 0.5 .. 2.7 l/min in steps of 0.1: 1070 .. 1290 pulses in steps of 10.
    let c = step_curve(24);
    assert_eq!(
        c.points()[23],
        FlowPoint {
            counts: 1290,
            flow_ml: 2700
        }
    );
    for step in 5..=27u16 {
        assert_eq!(
            flow_counts(&c, u32::from(step) * 100),
            1020 + 10 * step,
            "{step}"
        );
    }
    // The JSON of 32 points and back.
    let mut c = step_curve(32);
    c.qmax_ml = 3500;
    let text = json(&c);
    assert!(text.ends_with(r#"[1360,3.400],[1370,3.500]]}"#));
    assert_eq!(patched(&FlowConfig::DEFAULT, &text), Ok(c));
}

// ================================================================ JSON

#[test]
fn json_document_and_patch_round_trip() {
    let c = curve();
    let text = json(&c);
    assert_eq!(
        text,
        concat!(
            r#"{"enabled":true,"qmax":2.600,"refmA":34,"openmA":30,"fallbackPct":15,"#,
            r#""minIntervalS":60,"deadbandLpm":0.050,"offset":0,"gain":1.000,"#,
            r#""refSide":"closed","strokeCounts":2400,"preClose":false,"#,
            r#""liftCounts":0,"floorCounts":0,"#,
            r#""points":[[630,0.000],[760,0.300],[888,1.000],[1070,2.300],[1180,2.600]]}"#
        )
    );
    assert_eq!(patched(&FlowConfig::DEFAULT, &text), Ok(c));
    let mut open = open_curve();
    open.pre_close = true;
    open.offset = -17;
    open.gain_milli = 995;
    open.deadband_ml = 5;
    open.lift_counts = 1000;
    open.floor_counts = 4640;
    let text = json(&open);
    assert!(text.contains(r#""refSide":"open","strokeCounts":4641,"preClose":true,"#));
    assert!(text.contains(r#""offset":-17,"gain":0.995,"#));
    assert!(text.contains(r#""deadbandLpm":0.005,"#));
    assert!(text.contains(r#","liftCounts":1000,"floorCounts":4640,"points":"#));
    assert_eq!(patched(&FlowConfig::DEFAULT, &text), Ok(open));
    assert_eq!(json(&FlowConfig::DEFAULT).len(), 228);
    assert!(json(&FlowConfig::DEFAULT).ends_with(r#""points":[]}"#));
}

#[test]
fn json_patch_keeps_absent_members_and_takes_every_number_form() {
    let c = curve();
    let p = patched(&c, r#"{"offset":12,"gain":1.5}"#).expect("ok");
    assert_eq!((p.offset, p.gain_milli), (12, 1500));
    let p = patched(&c, r#"{"fallbackPct":20}"#).expect("ok");
    assert_eq!((p.fallback_pct, p.gain_milli), (20, c.gain_milli));
    assert_eq!(p.points(), c.points());
    let p = patched(&c, " { \"enabled\" : false , \"qmax\" : 2 } ").expect("ok");
    assert!(!p.enabled);
    assert_eq!(p.qmax_ml, 2000);
    let p = patched(&c, r#"{"qmax":0.01,"deadbandLpm":0,"minIntervalS":0}"#).expect("ok");
    assert_eq!((p.qmax_ml, p.deadband_ml, p.min_interval_s), (10, 0, 0));
    let p = patched(&c, r#"{"points":[[0,0],[10000,65.535]]}"#).expect("ok");
    assert_eq!(p.count, 2);
    assert_eq!(p.points[1].flow_ml, 65535);
    let p = patched(&c, r#"{"points":[[1,0.1],[2,0.12],[3,0.123]]}"#).expect("ok");
    assert_eq!(
        p.points[..3].iter().map(|p| p.flow_ml).collect::<Vec<_>>(),
        [100, 120, 123]
    );
    let p = patched(
        &c,
        r#"{"refSide":"open","strokeCounts":4641,"preClose":true}"#,
    )
    .expect("ok");
    assert_eq!(p.ref_side, RefSide::Open);
    assert_eq!(p.stroke_counts, 4641);
    assert!(p.pre_close);
    assert_eq!(p.points(), c.points());
    let p = patched(&c, r#"{"enabled":false,"points":[]}"#).expect("ok");
    assert_eq!(p.count, 0);
    let o = open_curve();
    let p = patched(&o, r#"{"liftCounts":100,"floorCounts":700}"#).expect("ok");
    assert_eq!((p.lift_counts, p.floor_counts), (100, 700));
    assert_eq!((p.stroke_counts, p.points()), (o.stroke_counts, o.points()));
    let p = patched(&p, r#"{"liftCounts":0}"#).expect("ok");
    assert_eq!((p.lift_counts, p.floor_counts), (0, 700));
    let p = patched(&p, r#"{"liftCounts":1000,"floorCounts":4640}"#).expect("ok");
    assert_eq!((p.lift_counts, p.floor_counts), (1000, 4640));
    // R2 from the closed reference in one patch, and back: the lift goes with the side.
    let body = r#"{"refSide":"open","strokeCounts":4641,"liftCounts":60,"floorCounts":850}"#;
    let p = patched(&c, body).expect("ok");
    assert_eq!(
        (p.ref_side, p.lift_counts, p.floor_counts),
        (RefSide::Open, 60, 850)
    );
    let p = patched(&p, r#"{"refSide":"closed","liftCounts":0}"#).expect("ok");
    assert_eq!(
        (p.ref_side, p.lift_counts, p.floor_counts),
        (RefSide::Closed, 0, 850)
    );
    assert_eq!(patched(&c, "{}"), Ok(c));
    assert_eq!(
        patched(&c, r#"{"offset":-10000}"#).map(|p| p.offset),
        Ok(-10000)
    );
}

#[test]
fn json_patch_refusals_leave_the_config_alone() {
    let c = curve();
    let cases = [
        ("", FlowError::Malformed),
        ("[]", FlowError::Malformed),
        ("{", FlowError::Malformed),
        ("{\"offset\" 1}", FlowError::Malformed),
        ("{\"offset\":1,}", FlowError::Malformed),
        ("{\"offset\":1}x", FlowError::Malformed),
        ("{offset:1}", FlowError::Malformed),
        ("{\"off\nset\":1}", FlowError::Malformed),
        ("{\"offset\":1 \"gain\":1}", FlowError::Malformed),
        ("{\"other\":1}", FlowError::UnknownKey),
        ("{\"off\\u0073et\":1}", FlowError::UnknownKey),
        ("{\"enabled\":1}", FlowError::Enabled),
        ("{\"enabled\":\"true\"}", FlowError::Enabled),
        ("{\"preClose\":0}", FlowError::PreClose),
        ("{\"refSide\":\"up\"}", FlowError::RefSide),
        ("{\"refSide\":1}", FlowError::RefSide),
        ("{\"strokeCounts\":0}", FlowError::StrokeCounts),
        ("{\"strokeCounts\":10001}", FlowError::StrokeCounts),
        ("{\"strokeCounts\":1.5}", FlowError::StrokeCounts),
        ("{\"strokeCounts\":-1}", FlowError::StrokeCounts),
        ("{\"strokeCounts\":70000}", FlowError::StrokeCounts),
        ("{\"liftCounts\":1001}", FlowError::LiftCounts),
        ("{\"liftCounts\":1.5}", FlowError::LiftCounts),
        ("{\"liftCounts\":-1}", FlowError::LiftCounts),
        ("{\"liftCounts\":70000}", FlowError::LiftCounts),
        ("{\"liftCounts\":\"100\"}", FlowError::LiftCounts),
        ("{\"liftCounts\":59}", FlowError::LiftCounts),
        ("{\"liftCounts\":60}", FlowError::LiftRefSide),
        ("{\"floorCounts\":10001}", FlowError::FloorCounts),
        ("{\"floorCounts\":2400}", FlowError::FloorCounts),
        ("{\"floorCounts\":0.5}", FlowError::FloorCounts),
        ("{\"floorCounts\":-1}", FlowError::FloorCounts),
        ("{\"floorCounts\":70000}", FlowError::FloorCounts),
        ("{\"floorCounts\":true}", FlowError::FloorCounts),
        ("{\"qmax\":\"2.7\"}", FlowError::Qmax),
        ("{\"qmax\":2.7001}", FlowError::Qmax),
        ("{\"qmax\":1e3}", FlowError::Qmax),
        ("{\"qmax\":-2.7}", FlowError::Qmax),
        ("{\"qmax\":70}", FlowError::Qmax),
        ("{\"qmax\":2.}", FlowError::Qmax),
        ("{\"qmax\":.5}", FlowError::Qmax),
        ("{\"qmax\":00.5}", FlowError::Qmax),
        ("{\"qmax\":3000000}", FlowError::Qmax),
        ("{\"refmA\":34.5}", FlowError::RefMa),
        ("{\"refmA\":300}", FlowError::RefMa),
        ("{\"refmA\":4}", FlowError::RefMa),
        ("{\"openmA\":61}", FlowError::OpenMa),
        ("{\"openmA\":true}", FlowError::OpenMa),
        ("{\"fallbackPct\":101}", FlowError::FallbackPct),
        ("{\"fallbackPct\":-1}", FlowError::FallbackPct),
        ("{\"minIntervalS\":3601}", FlowError::MinInterval),
        ("{\"minIntervalS\":1.5}", FlowError::MinInterval),
        ("{\"minIntervalS\":70000}", FlowError::MinInterval),
        ("{\"deadbandLpm\":5.001}", FlowError::Deadband),
        ("{\"deadbandLpm\":-1}", FlowError::Deadband),
        ("{\"deadbandLpm\":70}", FlowError::Deadband),
        ("{\"offset\":1.5}", FlowError::Offset),
        ("{\"offset\":10001}", FlowError::Offset),
        ("{\"offset\":40000}", FlowError::Offset),
        ("{\"gain\":0.099}", FlowError::Gain),
        ("{\"gain\":10.001}", FlowError::Gain),
        ("{\"gain\":-1}", FlowError::Gain),
        ("{\"gain\":70}", FlowError::Gain),
        ("{\"points\":1}", FlowError::Points),
        ("{\"points\":[1]}", FlowError::Points),
        ("{\"points\":[[1]]}", FlowError::Points),
        ("{\"points\":[[1,2,3]]}", FlowError::Points),
        ("{\"points\":[[1,2],]}", FlowError::Points),
        ("{\"points\":[[1,2]", FlowError::Points),
        ("{\"points\":[[\"1\",2]]}", FlowError::Points),
        ("{\"points\":[[1,\"2\"]]}", FlowError::Points),
        ("{\"points\":[[1 2]]}", FlowError::Points),
        ("{\"points\":[[1,2]}", FlowError::Points),
        ("{\"points\":[[1.5,2]]}", FlowError::PointCounts),
        ("{\"points\":[[-1,2]]}", FlowError::PointCounts),
        ("{\"points\":[[70000,2]]}", FlowError::PointCounts),
        ("{\"points\":[[10001,2],[10002,3]]}", FlowError::PointCounts),
        ("{\"points\":[[5,2],[5,3]]}", FlowError::PointCounts),
        ("{\"points\":[[1,-2]]}", FlowError::PointFlow),
        ("{\"points\":[[1,70]]}", FlowError::PointFlow),
        ("{\"points\":[[1,2],[2,1]]}", FlowError::PointFlow),
        ("{\"points\":[[1,2]]}", FlowError::PointCount),
        ("{\"points\":[]}", FlowError::PointCount),
    ];
    for (body, err) in cases {
        assert_eq!(patched(&c, body), Err(err), "{body}");
    }
    // 33 points: one more than a curve holds, refused while it is read.
    let points: Vec<String> = (1..=33).map(|i| std::format!("[{i},0]")).collect();
    let body = std::format!("{{\"points\":[{}]}}", points.join(","));
    assert_eq!(patched(&c, &body), Err(FlowError::PointCount));
    // Without a stroke: no layer and no open reference.
    let d = FlowConfig::DEFAULT;
    for body in [r#"{"refSide":"open"}"#, r#"{"enabled":true}"#] {
        assert_eq!(patched(&d, body), Err(FlowError::StrokeCounts), "{body}");
    }
    // A lift needs a floor; the closed reference refuses a lift that is there.
    let o = open_curve();
    assert_eq!(
        patched(&o, r#"{"liftCounts":100}"#),
        Err(FlowError::LiftFloor)
    );
    let r2 = patched(&o, r#"{"liftCounts":100,"floorCounts":850}"#).expect("ok");
    for (body, err) in [
        (r#"{"refSide":"closed"}"#, FlowError::LiftRefSide),
        (r#"{"floorCounts":0}"#, FlowError::LiftFloor),
        (r#"{"strokeCounts":850}"#, FlowError::FloorCounts),
    ] {
        assert_eq!(patched(&r2, body), Err(err), "{body}");
    }
}

// ================================================================ the state machine

/// gvlvy data of an idle, calibrated valve with the STM target `target` and the flags.
fn ex(valve: u8, target: u8, flags: u16) -> ValveEx {
    ValveEx {
        valve,
        status: 1,
        position: 20,
        target,
        mean_current: 20,
        open_count: 4641,
        close_count: 4800,
        v3: true,
        flags,
        fs_pct: 50,
        ..ValveEx::default()
    }
}

/// The STM as the layer sees it: a model with the active valves, fed with gvlvy replies that
/// a scripted move record follows.
struct Stm {
    m: ValveModel,
    now: u32,
    ex: [ValveEx; 12],
    events: Vec<Event>,
}

impl Stm {
    fn new(active: u16) -> Self {
        let mut m = ValveModel::default();
        m.set_active_mask(active);
        m.set_flow_mask(active);
        let mut ex_all = [ValveEx::default(); 12];
        for (v, e) in (0u8..).zip(ex_all.iter_mut()) {
            *e = ex(v, 15, 0);
        }
        let mut stm = Stm {
            m,
            now: 1000,
            ex: ex_all,
            events: Vec::new(),
        };
        for v in 0..12u8 {
            if (active >> v) & 1 != 0 {
                stm.poll(v);
            }
        }
        stm
    }

    /// A gvlvy reply of valve `v` with its current data.
    fn poll(&mut self, v: u8) {
        self.now += 2;
        let e = self.ex[usize::from(v)];
        self.m.apply_valve_ex(&e, self.now);
    }

    fn target(&mut self, v: u8, pct: u8) {
        assert!(self
            .m
            .set_desired_target(v, pct, TargetSource::Web, self.now));
    }

    /// A target in l/min of a valve of `cfg`, as the session sets it.
    fn flow(&mut self, cfg: &FlowConfig, v: u8, ml: u16) {
        let t = flow_target(cfg, u32::from(ml));
        assert!(self
            .m
            .set_desired_flow(v, t.pct, t.ml, TargetSource::Web, self.now));
    }

    /// The STM ends a move of `v`: the record, then the gvlvy reply the ESP polls.
    fn moved(&mut self, v: u8, dir: MoveDir, req: u16, cnt: u16, stop: StopReason) {
        let e = &mut self.ex[usize::from(v)];
        e.last_move = MoveResult {
            dir,
            requested_counts: u32::from(req),
            counted_counts: u32::from(cnt),
            stop,
            peak_current: 123,
            duration_ms: u32::from(cnt) * 7,
        };
        e.flags |= STM_FLAG_SVC_HOLD;
        self.poll(v);
    }

    fn flags(&mut self, v: u8, flags: u16) {
        self.ex[usize::from(v)].flags = flags;
        self.poll(v);
    }

    /// The positioning of valve 0 that was asked for goes through the open end stop and closes
    /// `close` pulses; the keep follows.
    fn moved_through(&mut self, f: &mut FlowLayer, close: u16) {
        assert_eq!(self.next(f), mv(0, MoveDir::Open, 10000, 30));
        self.result(f, 0, FlowOutcome::Ok);
        self.tick(f);
        self.moved(0, MoveDir::Open, 10000, 3000, StopReason::EndStop);
        self.tick(f);
        assert_eq!(self.next(f), mv(0, MoveDir::Close, close, 34));
        self.result(f, 0, FlowOutcome::Ok);
        self.tick(f);
        self.moved(0, MoveDir::Close, close, close, StopReason::Target);
        self.tick(f);
        assert_eq!(self.next(f), keep(0, true));
        self.result(f, 0, FlowOutcome::Ok);
        self.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    }

    fn inputs(&self) -> FlowInputs {
        FlowInputs {
            link_up: true,
            lease_expired: false,
        }
    }

    fn next(&mut self, f: &mut FlowLayer) -> Option<FlowRequest> {
        self.now += 2;
        let inp = self.inputs();
        f.next(&self.m, inp, self.now)
    }

    fn result(&mut self, f: &mut FlowLayer, v: u8, outcome: FlowOutcome) {
        self.now += 2;
        let events = &mut self.events;
        f.on_result(v, outcome, self.now, &mut |e| events.push(e.clone()));
    }

    fn tick(&mut self, f: &mut FlowLayer) {
        self.now += 1000;
        let events = &mut self.events;
        f.tick(&self.m, self.now, &mut |e| events.push(e.clone()));
    }

    /// A new STM session for the layer (the model follows in the test).
    fn rebooted(&mut self, f: &mut FlowLayer) {
        let events = &mut self.events;
        f.on_stm_rebooted(&mut |e| events.push(e.clone()));
    }

    fn codes(&self) -> Vec<EventCode> {
        self.events.iter().map(|e| e.code).collect()
    }
}

/// A layer with `cfg` on the valves of `mask`, supported.
fn layer(cfg: &FlowConfig, valves: &[u8]) -> FlowLayer {
    let mut f = FlowLayer::new();
    f.set_configs(&configs(cfg, valves));
    f.set_supported(true);
    f
}

fn mv(valve: u8, dir: MoveDir, counts: u16, max_ma: u8) -> Option<FlowRequest> {
    Some(FlowRequest::Move {
        valve,
        dir,
        counts,
        max_ma,
    })
}

fn keep(valve: u8, on: bool) -> Option<FlowRequest> {
    Some(FlowRequest::Keep { valve, on })
}

fn set_target(valve: u8, pct: u8) -> Option<FlowRequest> {
    Some(FlowRequest::SetTarget { valve, pct })
}

/// Runs the P1 of a closed reference from the fallback target to `counts` opened pulses:
/// stgtp when `fallback`, the close to the end stop, the opening move, the keep.
fn position(stm: &mut Stm, f: &mut FlowLayer, v: u8, fallback: bool, close: u16, counts: u16) {
    if fallback {
        assert_eq!(stm.next(f), set_target(v, 15));
        stm.result(f, v, FlowOutcome::Ok);
        stm.ex[usize::from(v)].target = 15;
    }
    assert_eq!(stm.next(f), mv(v, MoveDir::Close, close, 34));
    stm.result(f, v, FlowOutcome::Ok);
    assert_eq!(stm.next(f), None); // the record is awaited
    stm.tick(f);
    assert!(f.busy(v));
    stm.moved(v, MoveDir::Close, close, 97, StopReason::EndStop);
    stm.tick(f);
    if counts > 0 {
        assert_eq!(stm.next(f), mv(v, MoveDir::Open, counts, 30));
        stm.result(f, v, FlowOutcome::Ok);
        stm.tick(f);
        stm.moved(v, MoveDir::Open, counts, counts, StopReason::Target);
        stm.tick(f);
    }
    assert_eq!(stm.next(f), keep(v, true));
    stm.result(f, v, FlowOutcome::Ok);
    stm.flags(v, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
}

#[test]
fn new_layer_everything_off_nothing_sent_the_view_is_empty() {
    let mut f = FlowLayer::default();
    assert!(!f.supported());
    assert_eq!(f.active_mask(), 0);
    assert_eq!(f.view(0), FlowView::default());
    assert_eq!(f.view(12), FlowView::default());
    assert_eq!(*f.config(3), FlowConfig::DEFAULT);
    assert_eq!(*f.config(12), FlowConfig::DEFAULT);
    assert!(!f.busy(0));
    assert!(!f.take_dirty());
    let mut stm = Stm::new(0x001);
    stm.target(0, 50);
    assert_eq!(stm.next(&mut f), None);
    assert!(!f.position_counts(0, 700, served(50), 0));
    f.on_result(0, FlowOutcome::Ok, 0, &mut |_| panic!("no event"));
    f.on_dropped(0, 0);
    f.on_result(12, FlowOutcome::Ok, 0, &mut |_| panic!("no event"));
    f.on_dropped(12, 0);
    stm.tick(&mut f);
    assert!(stm.events.is_empty());
    assert_eq!(flow_state_name(FlowState::Off), "off");
    assert_eq!(flow_state_name(FlowState::Unsupported), "unsupported");
    assert_eq!(flow_state_name(FlowState::Unknown), "unknown");
    assert_eq!(flow_state_name(FlowState::Positioning), "positioning");
    assert_eq!(flow_state_name(FlowState::Held), "held");
    assert_eq!(flow_state_name(FlowState::Failed), "failed");
    for (i, s) in [
        FlowState::Off,
        FlowState::Unsupported,
        FlowState::Unknown,
        FlowState::Positioning,
        FlowState::Held,
        FlowState::Failed,
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(*s as usize, i);
    }
    assert_eq!(FlowRequest::SetTarget { valve: 3, pct: 1 }.valve(), 3);
    assert_eq!(FlowRequest::Keep { valve: 4, on: true }.valve(), 4);
    assert_eq!(mv(5, MoveDir::Open, 1, 5).map(|r| r.valve()), Some(5));
}

#[test]
fn the_view_shows_qmax_of_an_enabled_valve_only() {
    let mut f = FlowLayer::new();
    let mut c = curve();
    c.qmax_ml = 2500;
    f.set_configs(&configs(&c, &[0]));
    assert_eq!(f.view(0).qmax_ml, 2500);
    assert_eq!(f.view(1).qmax_ml, 0);
    assert_eq!(f.view(1), FlowView::default());
}

#[test]
fn unsupported_stm_the_enabled_valve_shows_unsupported_and_nothing_is_sent() {
    let mut f = FlowLayer::new();
    f.set_configs(&configs(&curve(), &[0]));
    assert!(f.take_dirty());
    assert_eq!(f.view(0).state, FlowState::Unsupported);
    assert!(f.view(0).enabled);
    assert_eq!(f.active_mask(), 0);
    let mut stm = Stm::new(0x001);
    stm.target(0, 50);
    assert_eq!(stm.next(&mut f), None);
    assert!(!f.position_counts(0, 700, served(50), 0));
    // Supported: the first positioning starts with the fallback target (the STM target is 50).
    stm.ex[0].target = 50;
    stm.poll(0);
    f.set_supported(true);
    assert!(f.supported());
    assert!(f.take_dirty());
    assert_eq!(f.active_mask(), 0x001);
    assert_eq!(f.view(0).state, FlowState::Unknown);
    assert_eq!(stm.next(&mut f), set_target(0, 15));
    assert_eq!(f.view(0).state, FlowState::Positioning);
    // Unsupported again (another STM): the request in flight is forgotten.
    f.set_supported(false);
    f.set_supported(false);
    assert_eq!(f.view(0).state, FlowState::Unsupported);
    assert_eq!(f.active_mask(), 0);
    stm.result(&mut f, 0, FlowOutcome::Ok);
    f.set_supported(true);
    assert_eq!(stm.next(&mut f), set_target(0, 15));
}

#[test]
fn first_positioning_fallback_target_close_to_the_end_stop_open_keep_held() {
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    assert_eq!(stm.next(&mut f), None); // no desired target
    stm.target(0, 37);
    // 37 % of the stroke is 888 pulses; the STM target is 15 already? No: 50.
    stm.ex[0].target = 50;
    stm.poll(0);
    assert_eq!(stm.next(&mut f), set_target(0, 15));
    assert_eq!(stm.next(&mut f), None); // one request at a time
    assert_eq!(f.view(0).state, FlowState::Positioning);
    assert_eq!(f.view(0).target_counts, 888);
    assert!(f.take_dirty());
    stm.result(&mut f, 0, FlowOutcome::Ok);
    // The stroke (4800) closes it: the position is unknown.
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), None);
    // A record of another move does not end it; the valve still closing neither.
    stm.moved(0, MoveDir::Close, 4700, 4700, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), None);
    stm.ex[0].status = 3;
    stm.moved(0, MoveDir::Close, 4800, 4799, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), None);
    stm.ex[0].status = 1;
    stm.poll(0);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 888, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 888, 888, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    assert_eq!(f.view(0).state, FlowState::Positioning);
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!(stm.codes(), [EventCode::FlowPositioned]);
    assert_eq!(
        (stm.events[0].valve, stm.events[0].arg1, stm.events[0].arg2),
        (0, 888, 37)
    );
    let v = f.view(0);
    assert_eq!(v.state, FlowState::Held);
    assert!(v.known);
    assert_eq!(
        (v.counts, v.target_counts, v.flow_ml, v.pct),
        (888, 888, 1000, 37)
    );
    assert!(v.last_valid);
    assert_eq!((v.last_ms, v.last_peak), (888 * 7, 123));
    assert!(!f.busy(0));
    // Held: nothing more while the flag arrives; the same target is served.
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Held);
    assert_eq!(stm.next(&mut f), None);
    stm.target(0, 37);
    assert_eq!(stm.next(&mut f), None);
}

#[test]
fn fallback_target_is_skipped_when_the_stm_target_is_it_already_and_sent_once() {
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 50);
    // The STM target is 15 (the fallback): the close comes first; 1200 pulses.
    position(&mut stm, &mut f, 0, false, 4800, 1200);
    assert_eq!(f.view(0).state, FlowState::Held);
    // A target only the reference reaches, after the interval: no stgtp again, the close counts
    // from the held position + 500.
    stm.now += 300_000;
    stm.target(0, 100);
    position(&mut stm, &mut f, 0, false, 1700, 2400);
    assert_eq!(f.view(0).pct, 100);
}

#[test]
fn closed_target_the_reference_close_alone_is_kept_the_position_is_0() {
    let c = curve();
    let mut f = layer(&c, &[2]);
    let mut stm = Stm::new(0x004);
    stm.target(2, 0);
    position(&mut stm, &mut f, 2, false, 4800, 0);
    let v = f.view(2);
    assert_eq!(v.state, FlowState::Held);
    assert_eq!((v.counts, v.flow_ml, v.pct, v.target_counts), (0, 0, 0, 0));
    assert!(v.known);
    assert_eq!((v.last_ms, v.last_peak), (97 * 7, 123));
    assert_eq!(stm.events.last().map(|e| (e.arg1, e.arg2)), Some((0, 0)));
    // A target of 0 is never delayed nor filtered: held at 37, the interval not over.
    stm.target(2, 37);
    stm.now += 300_000;
    position(&mut stm, &mut f, 2, false, 500, 888);
    stm.target(2, 0);
    assert_eq!(stm.next(&mut f), mv(2, MoveDir::Close, 1388, 34));
}

#[test]
fn dead_band_and_interval_hold_a_new_target_back_the_latest_wins() {
    let mut c = curve();
    c.min_interval_s = 60;
    c.deadband_ml = 100;
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 1000);
    let started = stm.now + 2; // the first request of the positioning
    position(&mut stm, &mut f, 0, false, 4800, 888);
    // 1.05 l/min: inside the dead band of 0.1 -> nothing.
    stm.flow(&c, 0, 1050);
    assert_eq!(stm.next(&mut f), None);
    // 1.35 l/min: outside, but the interval decides.
    stm.now = started + 30_000;
    stm.target(0, 50);
    assert_eq!(stm.next(&mut f), None);
    stm.target(0, 60); // the latest wins
    stm.now = started + 59_997; // the call comes 2 ms later
    assert_eq!(stm.next(&mut f), None);
    stm.now = started + 59_998;
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 552, 30));
    assert_eq!(f.view(0).target_counts, 1440);
    assert_eq!(f.view(0).state, FlowState::Positioning);
    // While it positions, the held value stays reported.
    assert_eq!(f.view(0).pct, 37);
    assert!(f.view(0).known);
}

#[test]
fn a_new_target_exactly_the_dead_band_away_is_served() {
    let mut c = curve();
    c.min_interval_s = 0;
    c.deadband_ml = 100;
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 1000);
    position(&mut stm, &mut f, 0, false, 4800, 888);
    stm.flow(&c, 0, 1099);
    assert_eq!(stm.next(&mut f), None);
    stm.flow(&c, 0, 901);
    assert_eq!(stm.next(&mut f), None);
    stm.flow(&c, 0, 1100); // 902 pulses
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 14, 30));
    assert_eq!(f.view(0).state, FlowState::Positioning);
}

#[test]
fn the_reference_close_is_repeated_up_to_three_times_then_the_positioning_fails() {
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 4800, 4800, StopReason::Target); // no end stop
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 800, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 800, 800, StopReason::Target);
    stm.tick(&mut f);
    // The third equals the last record: one pulse more, so its record differs.
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 801, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 801, 801, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), None);
    assert_eq!(f.view(0).state, FlowState::Failed);
    assert!(!f.view(0).known);
    assert_eq!(stm.codes(), [EventCode::FlowPositionFailed]);
    let e = &stm.events[0];
    assert_eq!((e.arg1, e.arg2), (2, StopReason::Target as i32));
    assert_eq!(&e.text[..], b"no end stop");
    // Failed: tried again after 5 min, or at once with another target.
    stm.now += 299_000;
    assert_eq!(stm.next(&mut f), None);
    stm.now += 1_000;
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    stm.result(&mut f, 0, FlowOutcome::Rejected(1));
    assert_eq!(f.view(0).state, FlowState::Failed);
    assert_eq!(stm.events[1].arg2, 1);
    assert_eq!(&stm.events[1].text[..], b"rejected");
    stm.target(0, 38);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    // A second close that reaches the end stop continues.
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 4800, 4800, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 800, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 800, 60, StopReason::EarlyEndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 912, 30));
    assert_eq!(CLOSE_EXTRA_COUNTS, 500);
    assert_eq!(REF_RETRY_COUNTS, 800);
    assert_eq!(REF_ATTEMPTS, 3);
}

#[test]
fn an_unexpected_stop_of_a_move_fails_the_positioning_with_the_reason() {
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 4800, 12, StopReason::UnderCurrent);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Failed);
    let e = &stm.events[0];
    assert_eq!((e.arg1, e.arg2), (2, 5));
    assert_eq!(&e.text[..], b"undercurrent");
    // ... and of the opening move (the close equals the last record: one pulse more).
    stm.target(0, 38);
    position_until_open(&mut stm, &mut f, 0, 4801);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 912, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 912, 400, StopReason::Aborted);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Failed);
    assert_eq!(stm.events[1].arg1, 5);
    assert_eq!(&stm.events[1].text[..], b"aborted");
    // An opening move that meets the open end stop is a position too.
    stm.target(0, 39);
    position_until_open(&mut stm, &mut f, 0, 4800);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 936, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 936, 700, StopReason::EarlyEndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!(f.view(0).counts, 700);
}

/// The close of a positioning that then awaits the opening move.
fn position_until_open(stm: &mut Stm, f: &mut FlowLayer, v: u8, close: u16) {
    assert_eq!(stm.next(f), mv(v, MoveDir::Close, close, 34));
    stm.result(f, v, FlowOutcome::Ok);
    stm.tick(f);
    stm.moved(v, MoveDir::Close, close, 90, StopReason::EndStop);
    stm.tick(f);
}

#[test]
fn a_move_that_never_ends_times_out_after_120_s() {
    assert_eq!(MOVE_TIMEOUT_MS, 120_000);
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    for _ in 0..119 {
        stm.tick(&mut f);
    }
    assert_eq!(f.view(0).state, FlowState::Positioning);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Failed);
    assert_eq!((stm.events[0].arg1, stm.events[0].arg2), (2, -2));
    assert_eq!(&stm.events[0].text[..], b"timeout");
    assert_eq!(stm.next(&mut f), None);
}

#[test]
fn busy_and_calibration_pending_rejections_retry_later() {
    assert_eq!(BUSY_RETRY_MS, 3_000);
    assert_eq!(BUSY_RETRIES_MAX, 100);
    assert_eq!(CALIB_RETRY_MS, 60_000);
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    stm.result(&mut f, 0, FlowOutcome::Rejected(2));
    assert_eq!(f.view(0).state, FlowState::Positioning);
    let at = stm.now;
    assert_eq!(stm.next(&mut f), None);
    stm.now = at + 2_990;
    assert_eq!(stm.next(&mut f), None);
    stm.now = at + 3_000;
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    // Calibration pending: the end of the calibration or 60 s.
    stm.result(&mut f, 0, FlowOutcome::Rejected(3));
    let at = stm.now;
    stm.now = at + 59_000;
    assert_eq!(stm.next(&mut f), None);
    stm.ex[0].calibrating = true;
    stm.poll(0);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), None); // calibrating: not free
    stm.ex[0].calibrating = false;
    stm.poll(0);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    stm.result(&mut f, 0, FlowOutcome::Rejected(3));
    stm.now += 60_000;
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    // 100 busy answers are retried, the 101st fails the positioning.
    for _ in 0..99 {
        stm.result(&mut f, 0, FlowOutcome::Rejected(2));
        stm.now += 3_000;
        assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    }
    stm.result(&mut f, 0, FlowOutcome::Rejected(2));
    assert_eq!(f.view(0).state, FlowState::Failed);
    assert_eq!(&stm.events[0].text[..], b"busy");
    assert_eq!(stm.events[0].arg2, 2);
}

#[test]
fn no_reply_and_a_rejected_keep_fail_the_positioning() {
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    stm.result(&mut f, 0, FlowOutcome::NoReply);
    assert_eq!(f.view(0).state, FlowState::Failed);
    assert_eq!((stm.events[0].arg1, stm.events[0].arg2), (2, -1));
    assert_eq!(&stm.events[0].text[..], b"no reply");
    stm.target(0, 38);
    position_until_open(&mut stm, &mut f, 0, 4800);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 912, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 912, 912, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Rejected(1));
    assert_eq!(f.view(0).state, FlowState::Failed);
    assert_eq!((stm.events[1].arg1, stm.events[1].arg2), (6, 1));
    // A fallback target without an answer fails at step 1.
    let mut f = layer(&c, &[1]);
    let mut stm = Stm::new(0x002);
    stm.ex[1].target = 50;
    stm.poll(1);
    stm.target(1, 37);
    assert_eq!(stm.next(&mut f), set_target(1, 15));
    stm.result(&mut f, 1, FlowOutcome::NoReply);
    assert_eq!(f.view(1).state, FlowState::Failed);
    assert_eq!((stm.events[0].arg1, stm.events[0].arg2), (1, -1));
}

#[test]
fn a_dropped_request_is_tried_again_after_3_s() {
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    let at = stm.now;
    f.on_dropped(0, at);
    stm.now = at + 2_000;
    assert_eq!(stm.next(&mut f), None);
    stm.now = at + 3_000;
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    assert_eq!(f.view(0).state, FlowState::Positioning);
}

#[test]
fn a_request_whose_result_never_comes_frees_the_layer_after_30_s() {
    assert_eq!(FLOW_REQ_TIMEOUT_MS, 30_000);
    let c = curve();
    let mut f = layer(&c, &[0, 1]);
    let mut stm = Stm::new(0x003);
    stm.now += 100_000;
    stm.ex[0].target = 50;
    stm.poll(0);
    stm.target(0, 37);
    assert_eq!(stm.next(&mut f), set_target(0, 15));
    let at = stm.now;
    // Another stgtp of valve 0 took its place in the queue: no result. Valve 1 waits ...
    stm.target(1, 37);
    let inp = inputs(true, false);
    assert_eq!(f.next(&stm.m, inp, at + FLOW_REQ_TIMEOUT_MS - 1), None);
    // ... until the request counts as not queued: valve 0 tries again after 3 s, valve 1
    // starts.
    stm.now = at + FLOW_REQ_TIMEOUT_MS;
    assert_eq!(
        f.next(&stm.m, inp, stm.now),
        mv(1, MoveDir::Close, 4800, 34)
    );
    assert_eq!(f.view(0).state, FlowState::Positioning);
    // A late result of the old request changes nothing.
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert!(stm.events.is_empty());
    stm.result(&mut f, 1, FlowOutcome::Rejected(1));
    stm.now += BUSY_RETRY_MS;
    assert_eq!(stm.next(&mut f), set_target(0, 15));
    // Disabled while its request waits for a result that never comes: the release goes out
    // 30 s after the request, not before; a release without a result goes out again.
    let at = stm.now;
    stm.now += 20_000;
    f.set_configs(&configs(&c, &[1]));
    assert_eq!(f.next(&stm.m, inp, at + FLOW_REQ_TIMEOUT_MS - 1), None);
    stm.now = at + FLOW_REQ_TIMEOUT_MS;
    assert_eq!(f.next(&stm.m, inp, stm.now), keep(0, false));
    let at = stm.now;
    assert_eq!(f.next(&stm.m, inp, at + FLOW_REQ_TIMEOUT_MS - 1), None);
    assert_eq!(
        f.next(&stm.m, inp, at + FLOW_REQ_TIMEOUT_MS),
        keep(0, false)
    );
    assert_eq!(f.view(0).state, FlowState::Off);
}

#[test]
fn the_result_of_a_release_frees_the_layer_at_once() {
    let c = curve();
    let mut f = layer(&c, &[0, 1]);
    let mut stm = Stm::new(0x003);
    stm.target(0, 37);
    position(&mut stm, &mut f, 0, false, 4800, 888);
    f.set_configs(&configs(&c, &[1]));
    assert_eq!(stm.next(&mut f), keep(0, false));
    stm.target(1, 37);
    assert_eq!(stm.next(&mut f), None); // the release is in flight
    f.take_dirty();
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert!(f.take_dirty());
    assert_eq!(stm.next(&mut f), mv(1, MoveDir::Close, 4800, 34));
}

#[test]
fn a_retry_time_half_the_clock_range_ahead_is_not_reached() {
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    let until = stm.now + BUSY_RETRY_MS;
    f.on_dropped(0, stm.now);
    let inp = inputs(true, false);
    assert_eq!(f.next(&stm.m, inp, until.wrapping_add(0x8000_0000)), None);
    assert_eq!(
        f.next(&stm.m, inp, until.wrapping_add(0x7fff_ffff)),
        mv(0, MoveDir::Close, 4800, 34)
    );
}

/// The request of valve 0 that is out is refused: the positioning has failed.
fn refused(stm: &mut Stm, f: &mut FlowLayer) {
    stm.result(f, 0, FlowOutcome::Rejected(1));
    assert_eq!(f.view(0).state, FlowState::Failed);
}

/// The next retry comes `ms` after the failure to the ms, a close through the end stop.
fn retry_after(stm: &mut Stm, f: &mut FlowLayer, ms: u32) {
    let inp = inputs(true, false);
    assert_eq!(f.next(&stm.m, inp, stm.now + ms - 1), None);
    stm.now += ms;
    assert_eq!(
        f.next(&stm.m, inp, stm.now),
        mv(0, MoveDir::Close, 4800, 34)
    );
}

/// A valve 0 whose first positioning to 37 % failed.
fn failed_once(c: &FlowConfig) -> (Stm, FlowLayer) {
    let mut f = layer(c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    refused(&mut stm, &mut f);
    (stm, f)
}

const MIN: u32 = 60_000;

#[test]
fn a_failed_positioning_is_tried_again_after_5_10_20_and_then_every_60_min() {
    let (mut stm, mut f) = failed_once(&curve());
    for ms in [5 * MIN, 10 * MIN, 20 * MIN, 60 * MIN, 60 * MIN, 60 * MIN] {
        retry_after(&mut stm, &mut f, ms);
        refused(&mut stm, &mut f);
    }
    // every failure is one event 417
    let failed = stm
        .codes()
        .iter()
        .filter(|c| **c == EventCode::FlowPositionFailed)
        .count();
    assert_eq!(failed, 7);
}

#[test]
fn the_failures_stop_counting_at_their_limit_and_the_wait_stays_60_min() {
    let (mut stm, mut f) = failed_once(&curve());
    f.v[0].failures = u8::MAX;
    retry_after(&mut stm, &mut f, 60 * MIN);
    refused(&mut stm, &mut f);
    assert_eq!(f.v[0].failures, u8::MAX);
}

#[test]
fn a_new_target_goes_out_at_once_and_starts_the_waits_again() {
    let (mut stm, mut f) = failed_once(&curve());
    retry_after(&mut stm, &mut f, 5 * MIN);
    refused(&mut stm, &mut f);
    stm.target(0, 40);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    refused(&mut stm, &mut f);
    retry_after(&mut stm, &mut f, 5 * MIN);
}

#[test]
fn a_success_starts_the_waits_again() {
    let (mut stm, mut f) = failed_once(&curve());
    retry_after(&mut stm, &mut f, 5 * MIN);
    refused(&mut stm, &mut f);
    retry_after(&mut stm, &mut f, 10 * MIN);
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 4800, 97, StopReason::EndStop);
    stm.tick(&mut f);
    assert!(matches!(stm.next(&mut f), Some(FlowRequest::Move { .. })));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    let counts = f.view(0).target_counts;
    stm.moved(0, MoveDir::Open, counts, counts, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!(f.view(0).state, FlowState::Held);
    assert_eq!(f.v[0].failures, 0);
}

#[test]
fn a_counts_request_and_a_changed_curve_start_the_waits_again() {
    let c = curve();
    let (stm, mut f) = failed_once(&c);
    assert!(f.position_counts(0, 700, served(37), stm.now));
    assert_eq!(f.v[0].failures, 0);
    let (_, mut f) = failed_once(&c);
    let moved = FlowConfig { offset: 5, ..c };
    f.set_configs(&configs(&moved, &[0]));
    assert_eq!(f.v[0].failures, 0);
}

#[test]
fn a_new_stm_session_tries_at_once_and_starts_the_waits_again() {
    let (mut stm, mut f) = failed_once(&curve());
    retry_after(&mut stm, &mut f, 5 * MIN);
    refused(&mut stm, &mut f);
    stm.rebooted(&mut f);
    stm.m.on_stm_rebooted(stm.now);
    assert_eq!(stm.next(&mut f), set_target(0, 15));
    assert_eq!(f.v[0].failures, 0);
}

#[test]
fn the_record_of_the_final_move_counts_once_the_valve_stopped() {
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 4800, 97, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 888, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    // The record is there while the valve still opens: the keep waits.
    stm.ex[0].status = 2;
    stm.moved(0, MoveDir::Open, 888, 888, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), None);
    assert!(!f.view(0).last_valid);
    stm.ex[0].status = 1;
    stm.poll(0);
    stm.tick(&mut f);
    assert!(f.view(0).last_valid);
    assert_eq!(stm.next(&mut f), keep(0, true));
}

#[test]
fn hold_lost_by_the_flag_or_another_move_repositions_when_the_valve_is_free() {
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    position(&mut stm, &mut f, 0, false, 4800, 888);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Held);
    // The flag is gone (a calibration, the lease): unknown, repositioned once the valve is
    // free; the event names the flags.
    stm.ex[0].calibrating = true;
    stm.flags(0, 0);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Unknown);
    assert!(!f.view(0).known);
    assert_eq!(f.view(0).pct, 0);
    assert_eq!(
        stm.codes(),
        [EventCode::FlowPositioned, EventCode::FlowHoldLost]
    );
    assert_eq!(stm.events[1].arg1, 0);
    assert_eq!(stm.next(&mut f), None);
    stm.ex[0].calibrating = false;
    stm.poll(0);
    stm.tick(&mut f);
    // The position is unknown again: the stroke closes it, no interval.
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 4800, 100, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 888, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 888, 888, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Held);
    // Another move of the valve (a user service move, a failsafe move): lost as well.
    stm.moved(0, MoveDir::Open, 50, 50, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Unknown);
    assert_eq!(stm.codes().len(), 4);
}

#[test]
fn hold_the_flag_counts_only_from_a_poll_after_the_keep() {
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    position_until_open(&mut stm, &mut f, 0, 4800);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 888, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 888, 888, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    // The last poll predates the keep: its flags (without svcKeep) do not count.
    stm.tick(&mut f);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Held);
    // A poll after it without the flag: lost.
    stm.flags(0, STM_FLAG_SVC_HOLD);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Unknown);
}

#[test]
fn stm_reboot_forgets_the_hold_and_the_request_in_flight_the_fallback_goes_first() {
    let c = curve();
    let mut f = layer(&c, &[0, 1]);
    let mut stm = Stm::new(0x003);
    stm.target(0, 37);
    position(&mut stm, &mut f, 0, false, 4800, 888);
    stm.target(1, 50);
    assert_eq!(stm.next(&mut f), mv(1, MoveDir::Close, 4800, 34));
    stm.rebooted(&mut f);
    assert!(f.take_dirty());
    assert_eq!(f.view(0).state, FlowState::Unknown);
    assert_eq!(f.view(1).state, FlowState::Unknown);
    assert!(!f.view(0).known);
    // the last move stays on record
    assert!(f.view(0).last_valid);
    assert_eq!((f.view(0).last_ms, f.view(0).last_peak), (6216, 123));
    assert!(!f.busy(1));
    // The held valve lost its hold (event), the positioning valve silently.
    assert_eq!(
        stm.codes(),
        [EventCode::FlowPositioned, EventCode::FlowHoldLost]
    );
    assert_eq!((stm.events[1].valve, stm.events[1].arg1), (0, 0));
    // The STM's targets are unknown after the reboot: the fallback goes out before the first
    // read-back (valve 0 first: the cursor moved past valve 1), then the close from the stroke.
    stm.m.on_stm_rebooted(stm.now);
    stm.result(&mut f, 1, FlowOutcome::NoReply); // the late answer of the old request
    assert_eq!(f.view(1).state, FlowState::Unknown);
    assert_eq!(stm.events.len(), 2); // no failure logged for it
    assert_eq!(stm.next(&mut f), set_target(0, 15));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    // Round robin: valve 1 starts (its fallback too) before valve 0 continues.
    assert_eq!(stm.next(&mut f), set_target(1, 15));
    stm.result(&mut f, 1, FlowOutcome::Ok);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34)); // the position is unknown
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 4800, 100, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(1, MoveDir::Close, 4800, 34));
    stm.result(&mut f, 1, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(1, MoveDir::Close, 4800, 100, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 888, 30));
}

#[test]
fn valves_wait_while_moving_calibrating_in_assembly_at_the_lease_failsafe_or_without_a_link() {
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    let busy = [
        |e: &mut ValveEx| e.status = 2,
        |e: &mut ValveEx| e.status = 3,
        |e: &mut ValveEx| e.calibrating = true,
        |e: &mut ValveEx| e.flags = STM_FLAG_FS_LEASE,
    ];
    for (i, change) in busy.iter().enumerate() {
        let mut e = ex(0, 15, 0);
        change(&mut e);
        stm.ex[0] = e;
        stm.poll(0);
        assert_eq!(stm.next(&mut f), None, "case {i}");
    }
    stm.ex[0] = ex(0, 15, 0);
    stm.poll(0);
    let inp = FlowInputs {
        link_up: true,
        lease_expired: true,
    };
    assert_eq!(f.next(&stm.m, inp, stm.now), None);
    let inp = FlowInputs {
        link_up: false,
        lease_expired: false,
    };
    assert_eq!(f.next(&stm.m, inp, stm.now), None);
    // An assembly: the layer yields until the next target, which ends the assembly hold with
    // the fallback target first (the STM target is 100 then, not read back yet).
    stm.m.set_assembly(0, stm.now);
    assert_eq!(stm.next(&mut f), None);
    stm.target(0, 37);
    assert_eq!(stm.next(&mut f), set_target(0, 15));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    // No v3 data (gvlvx): nothing either.
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.ex[0].v3 = false;
    stm.poll(0);
    stm.target(0, 37);
    assert_eq!(stm.next(&mut f), None);
    // An inactive valve of the model is never known: nothing.
    let mut f = layer(&c, &[5]);
    let mut stm = Stm::new(0x001);
    assert_eq!(stm.next(&mut f), None);
}

#[test]
fn moves_of_different_valves_run_one_after_the_other_round_robin() {
    let c = curve();
    let mut f = layer(&c, &[0, 1, 2]);
    let mut stm = Stm::new(0x007);
    stm.target(0, 37);
    stm.target(1, 37);
    stm.target(2, 37);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    assert_eq!(stm.next(&mut f), None);
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!(stm.next(&mut f), None); // the move of valve 0 runs
    assert!(f.busy(0));
    assert!(!f.busy(1)); // not started yet: polled at the idle cadence
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 4800, 100, StopReason::EndStop);
    stm.tick(&mut f);
    // Round robin: valve 1 starts before valve 0 continues.
    assert_eq!(stm.next(&mut f), mv(1, MoveDir::Close, 4800, 34));
    stm.result(&mut f, 1, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(1, MoveDir::Close, 4800, 100, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(2, MoveDir::Close, 4800, 34));
    stm.result(&mut f, 2, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(2, MoveDir::Close, 4800, 100, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 888, 30));
}

#[test]
fn counts_request_positions_at_once_and_counts_the_desired_target_as_served() {
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    position(&mut stm, &mut f, 0, false, 4800, 888);
    // 984 corrected pulses at once (the interval is not over), the view with its own %.
    assert!(f.position_counts(0, 984, served(37), stm.now));
    assert!(f.take_dirty());
    assert_eq!(f.view(0).state, FlowState::Positioning);
    assert_eq!(f.view(0).target_counts, 984);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1388, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 1388, 100, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 984, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 984, 984, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    let v = f.view(0);
    assert_eq!((v.counts, v.flow_ml, v.pct), (984, 1686, 41));
    assert_eq!(stm.events.last().map(|e| e.arg2), Some(41));
    // The desired 37 is served: no positioning back, until it changes.
    stm.now += 400_000;
    assert_eq!(stm.next(&mut f), None);
    stm.target(0, 100);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1484, 34));
    // A counts request during a positioning follows it; the limit is 10000; 0 closes.
    assert!(f.position_counts(0, 12000, served(100), stm.now));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 1484, 100, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 2400, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 2400, 2400, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 2900, 34));
    assert_eq!(f.view(0).target_counts, 10000);
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 2900, 100, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 10000, 4700, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    assert_eq!(f.view(0).pct, 100);
    assert!(f.position_counts(0, 0, None, stm.now));
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 5200, 34));
    assert_eq!(f.view(0).target_counts, 0);
}

#[test]
fn disabling_releases_the_keep_the_curve_change_repositions() {
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 999);
    position(&mut stm, &mut f, 0, false, 4800, 888);
    // The same config again: nothing.
    assert!(f.take_dirty());
    f.set_configs(&configs(&c, &[0]));
    assert!(!f.take_dirty());
    assert_eq!(f.view(0).state, FlowState::Held);
    // The dead band only: nothing; the offset: positioned again, no interval.
    let mut c2 = c;
    c2.deadband_ml = 10;
    f.set_configs(&configs(&c2, &[0]));
    assert_eq!(f.view(0).state, FlowState::Held);
    c2.offset = 5;
    f.set_configs(&configs(&c2, &[0]));
    assert!(f.take_dirty());
    assert_eq!(f.view(0).state, FlowState::Unknown);
    assert!(f.view(0).known); // the old position until the moves start
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1388, 34));
    assert_eq!(f.view(0).target_counts, 893);
    // A change during the positioning: another one follows the keep.
    c2.gain_milli = 1001;
    f.set_configs(&configs(&c2, &[0]));
    assert_eq!(f.view(0).state, FlowState::Positioning);
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 1388, 100, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 893, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 893, 893, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!(f.view(0).state, FlowState::Unknown);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1393, 34));
    assert_eq!(f.view(0).target_counts, 894);
    // Disabled while positioning: the keep is released once, the state is off.
    f.set_configs(&configs(&FlowConfig::DEFAULT, &[0]));
    assert_eq!(f.view(0).state, FlowState::Off);
    assert_eq!(f.active_mask(), 0);
    stm.result(&mut f, 0, FlowOutcome::Ok); // the close in flight: ignored
    assert_eq!(stm.next(&mut f), keep(0, false));
    stm.result(&mut f, 0, FlowOutcome::NoReply);
    assert_eq!(stm.next(&mut f), None);
    assert_eq!(f.view(0).state, FlowState::Off);
    assert!(!f.view(0).known);
    // A dropped release goes out again.
    f.set_configs(&configs(&c, &[0]));
    stm.ex[0].target = 15;
    stm.poll(0);
    position(&mut stm, &mut f, 0, false, 4800, 888);
    f.set_configs(&configs(&FlowConfig::DEFAULT, &[0]));
    assert_eq!(stm.next(&mut f), keep(0, false));
    f.on_dropped(0, stm.now);
    assert_eq!(stm.next(&mut f), keep(0, false));
    // Disabled while unknown and idle: nothing to release.
    f.set_configs(&configs(&c, &[0]));
    assert_eq!(f.view(0).state, FlowState::Unknown);
    f.set_configs(&configs(&FlowConfig::DEFAULT, &[0]));
    assert_eq!(stm.next(&mut f), None);
}

#[test]
fn pre_close_warms_the_end_stop_before_the_opening_move() {
    assert_eq!(WARM_UP_COUNTS, 300);
    let mut c = curve();
    c.pre_close = true;
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    position_until_open(&mut stm, &mut f, 0, 4800);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 300, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 300, 300, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 800, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 800, 350, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 888, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 888, 888, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!(f.view(0).counts, 888);
    // The warm-up close that misses the end stop is repeated; a warm-up opening that meets
    // the open end stop goes on; a target of 0 skips the warm-up.
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    stm.now += 300_000;
    stm.target(0, 100);
    position_until_open(&mut stm, &mut f, 0, 1388);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 300, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 300, 299, StopReason::EarlyEndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 800, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 800, 800, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 801, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 801, 400, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 2400, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 2400, 200, StopReason::UnderCurrent);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Failed);
    stm.target(0, 0);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 4800, 300, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    // A warm-up opening that fails.
    let mut f = layer(&c, &[1]);
    let mut stm = Stm::new(0x002);
    stm.target(1, 37);
    position_until_open(&mut stm, &mut f, 1, 4800);
    assert_eq!(stm.next(&mut f), mv(1, MoveDir::Open, 300, 30));
    stm.result(&mut f, 1, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(1, MoveDir::Open, 300, 10, StopReason::Timeout);
    stm.tick(&mut f);
    assert_eq!(f.view(1).state, FlowState::Failed);
    assert_eq!(stm.events.last().map(|e| e.arg1), Some(3));
    // With an open reference the warm-up does not apply.
    let mut open = open_curve();
    open.pre_close = true;
    let mut f = layer(&open, &[2]);
    let mut stm = Stm::new(0x004);
    stm.flow(&open, 2, 999);
    assert_eq!(stm.next(&mut f), mv(2, MoveDir::Open, 10000, 30));
    stm.result(&mut f, 2, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(2, MoveDir::Open, 10000, 4650, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(2, MoveDir::Close, 3753, 34));
}

#[test]
fn open_reference_opens_to_the_end_stop_then_closes_the_curve_pulses() {
    let c = open_curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 999);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    // Not at the end stop: again with 800, then 801 (the record), then it fails.
    stm.moved(0, MoveDir::Open, 10000, 10000, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 800, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 800, 800, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 801, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 801, 30, StopReason::EarlyEndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 3753, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 3753, 3753, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    let v = f.view(0);
    assert_eq!(v.state, FlowState::Held);
    assert_eq!((v.counts, v.flow_ml, v.pct), (888, 1000, 19)); // 4641 - 3753
    assert_eq!((v.last_ms, v.last_peak), (3753 * 7, 123));
    // No held position before the first positioning.
    assert_eq!(v.ref_delta, None);
    // A positioning that goes through the end stop again (the whole range, one more than the
    // record when equal); the final close may meet the closed end stop.
    assert!(f.position_counts(0, 795, Served::of(stm.m.valve(0)), stm.now));
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 10000, 3753, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 3846, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 3846, 3861, StopReason::EarlyEndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    assert_eq!(f.view(0).counts, 780); // 4641 - 3861
    assert_eq!(f.view(0).pct, 17); // of 4641
    assert_eq!(f.view(0).ref_delta, Some(0));
    // A target of 0 closes to the closed reference as always.
    stm.target(0, 0);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1280, 34)); // 780 + 500
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 1280, 700, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!((f.view(0).counts, f.view(0).pct), (0, 0));
    // The close to the reference measures nothing: the last measure stays, the event has none.
    assert_eq!(f.view(0).ref_delta, Some(0));
    assert_eq!(stm.events.last().unwrap().text.as_slice(), b"");
}

#[test]
fn ref_delta_is_what_the_opening_counted_above_the_way_to_the_end_stop() {
    let c = open_curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 999);
    let go = |stm: &mut Stm, f: &mut FlowLayer, counts, cnt, stop, close: u16| {
        stm.now += 300_000;
        if counts != 888 || f.view(0).known {
            assert!(f.position_counts(0, counts, Served::of(stm.m.valve(0)), stm.now));
        }
        assert_eq!(stm.next(f), mv(0, MoveDir::Open, 10000, 30));
        stm.result(f, 0, FlowOutcome::Ok);
        stm.tick(f);
        stm.moved(0, MoveDir::Open, 10000, cnt, stop);
        stm.tick(f);
        assert_eq!(stm.next(f), mv(0, MoveDir::Close, close, 34));
        stm.result(f, 0, FlowOutcome::Ok);
        stm.tick(f);
        stm.moved(0, MoveDir::Close, close, close + 1, StopReason::Target);
        stm.tick(f);
        assert_eq!(stm.next(f), keep(0, true));
        stm.result(f, 0, FlowOutcome::Ok);
        stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
        f.view(0)
    };
    let v = go(&mut stm, &mut f, 888, 4000, StopReason::EndStop, 3753);
    assert_eq!((v.counts, v.ref_delta), (887, None));
    assert_eq!(stm.events.last().unwrap().text.as_slice(), b"");
    let v = go(&mut stm, &mut f, 795, 3780, StopReason::EndStop, 3846);
    assert_eq!((v.counts, v.ref_delta), (794, Some(26))); // 3780 - (4641 - 887)
    assert_eq!(stm.events.last().unwrap().text.as_slice(), b"ref +26");
    let v = go(&mut stm, &mut f, 888, 3831, StopReason::EarlyEndStop, 3753);
    assert_eq!((v.counts, v.ref_delta), (887, Some(-16))); // 3831 - (4641 - 794)
    assert_eq!(stm.events.last().unwrap().text.as_slice(), b"ref -16");
}

#[test]
fn ref_delta_needs_the_first_reference_move_from_a_held_position() {
    let c = open_curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 999);
    position_open(&mut stm, &mut f, 0, 3753);
    assert_eq!(f.view(0).counts, 888);
    // The first move stops short of the end stop: the count of the second one is only its end.
    assert!(f.position_counts(0, 795, Served::of(stm.m.valve(0)), stm.now));
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 10000, 3000, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 800, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 800, 700, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 3846, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 3846, 3846, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!(f.view(0).ref_delta, None);
    // A disabled layer shows none.
    let mut measured = f.view(0);
    measured.ref_delta = Some(5);
    assert_ne!(measured, f.view(0));
    f.set_configs(&configs(&c, &[]));
    assert_eq!(f.view(0).ref_delta, None);
}

#[test]
fn reference_counts_when_the_stroke_is_unknown_the_whole_range_at_the_limit_one_less() {
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.ex[0].open_count = 0;
    stm.ex[0].close_count = 0;
    stm.ex[0].last_move.dir = MoveDir::Close;
    stm.ex[0].last_move.requested_counts = 10000;
    stm.poll(0);
    stm.target(0, 37);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 9999, 34));
    // A held position near the limit.
    let mut f = layer(&c, &[1]);
    let mut stm = Stm::new(0x002);
    stm.ex[1].open_count = 12000;
    stm.poll(1);
    stm.target(1, 37);
    assert_eq!(stm.next(&mut f), mv(1, MoveDir::Close, 10000, 34));
    stm.result(&mut f, 1, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(1, MoveDir::Close, 10000, 4700, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(1, MoveDir::Open, 888, 30));
    stm.result(&mut f, 1, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(1, MoveDir::Open, 888, 9800, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(1, true));
    stm.result(&mut f, 1, FlowOutcome::Ok);
    stm.flags(1, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    assert_eq!(f.view(1).counts, 9800);
    stm.now += 300_000;
    stm.target(1, 100);
    assert_eq!(stm.next(&mut f), mv(1, MoveDir::Close, 10000, 34));
    // The reference close that would equal the last record gets one pulse more.
    let mut f = layer(&c, &[2]);
    let mut stm = Stm::new(0x004);
    stm.ex[2].last_move.dir = MoveDir::Close;
    stm.ex[2].last_move.requested_counts = 4800;
    stm.poll(2);
    stm.target(2, 37);
    assert_eq!(stm.next(&mut f), mv(2, MoveDir::Close, 4801, 34));
    // An assembly flag of the STM alone does not stop the layer (the source does).
    let mut f = layer(&c, &[3]);
    let mut stm = Stm::new(0x008);
    stm.flags(3, STM_FLAG_ASSEMBLY);
    stm.target(3, 37);
    assert_eq!(stm.next(&mut f), mv(3, MoveDir::Close, 4800, 34));
}

// ================================================================ one move from a held position

/// Valve 0 of `c`, an open reference, held at 888 pulses for 1 l/min, the interval over.
fn held_open(c: &FlowConfig) -> (Stm, FlowLayer) {
    let mut f = layer(c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(c, 0, 1000);
    position_open(&mut stm, &mut f, 0, c.stroke_counts - 888);
    stm.now += 300_000;
    (stm, f)
}

/// A target of 1.35 l/min on valve 0 of the open curve: 937 pulses.
fn to_937(stm: &mut Stm) {
    stm.flow(&open_curve(), 0, 1350);
}

/// A target of 1 l/min on valve 0 of the open curve: 888 pulses.
fn to_888(stm: &mut Stm) {
    stm.flow(&open_curve(), 0, 1000);
}

/// The record of the direct move of valve 0 and its keep.
fn direct_done(stm: &mut Stm, f: &mut FlowLayer, dir: MoveDir, req: u16, cnt: u16) {
    stm.result(f, 0, FlowOutcome::Ok);
    stm.tick(f);
    stm.moved(0, dir, req, cnt, StopReason::Target);
    stm.tick(f);
    assert_eq!(stm.next(f), keep(0, true));
    stm.result(f, 0, FlowOutcome::Ok);
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
}

#[test]
fn a_new_target_of_a_held_valve_is_one_move_and_the_keep() {
    let (mut stm, mut f) = held_open(&open_curve());
    to_937(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 49, 30));
    assert!(f.busy(0));
    let v = f.view(0);
    assert_eq!(
        (v.state, v.counts, v.target_counts),
        (FlowState::Positioning, 888, 937)
    );
    assert!(v.known);
    // The counted pulses hold the brake's: 52, not the 49 asked for.
    direct_done(&mut stm, &mut f, MoveDir::Open, 49, 52);
    let v = f.view(0);
    assert_eq!(
        (v.state, v.counts, v.target_counts),
        (FlowState::Held, 940, 937)
    );
    assert_eq!((v.last_ms, v.last_peak), (52 * 7, 123));
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Held);
    assert_eq!(stm.next(&mut f), None);
    // Back: closing with the threshold of a closing move.
    stm.now += 300_000;
    to_888(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 52, 34));
    direct_done(&mut stm, &mut f, MoveDir::Close, 52, 54);
    assert_eq!(f.view(0).counts, 886);
    // No event of these moves in the ring of 32.
    assert_eq!(stm.codes(), [EventCode::FlowPositioned]);
}

#[test]
fn a_direct_move_of_the_same_pulses_as_the_last_one_gets_a_record_of_its_own() {
    let (mut stm, mut f) = held_open(&open_curve());
    to_937(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 49, 30));
    direct_done(&mut stm, &mut f, MoveDir::Open, 49, 49);
    stm.now += 300_000;
    to_888(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 50, 34));
    direct_done(&mut stm, &mut f, MoveDir::Close, 50, 50);
    assert_eq!(f.view(0).counts, 887);
}

#[test]
fn the_held_position_of_a_direct_move_stays_inside_the_range() {
    let (mut stm, mut f) = held_open(&open_curve());
    to_937(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 49, 30));
    direct_done(&mut stm, &mut f, MoveDir::Open, 49, 9900);
    assert_eq!(f.view(0).counts, FLOW_COUNTS_MAX);
    let (mut stm, mut f) = held_open(&open_curve());
    stm.flow(&open_curve(), 0, 540); // 804 pulses
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 84, 34));
    direct_done(&mut stm, &mut f, MoveDir::Close, 84, 984);
    assert_eq!(f.view(0).counts, 0);
    let v = f.view(0);
    assert_eq!((v.state, v.known), (FlowState::Held, true));
}

#[test]
fn a_direct_move_needs_the_hold_of_the_stm_and_the_record_of_the_last_move() {
    // The hold counts down only without the keep, so a kept valve shows both flags. The keep
    // alone says the STM is free to drive the valve: the moves start from the end stop.
    let (mut stm, mut f) = held_open(&open_curve());
    stm.flags(0, STM_FLAG_SVC_KEEP);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Held);
    to_937(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    assert!(!f.view(0).known);
    // The hold alone will be noticed by the next tick: the position is still known.
    let (mut stm, mut f) = held_open(&open_curve());
    stm.flags(0, STM_FLAG_SVC_HOLD);
    to_937(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 49, 30));
    // Another record before the tick: not where the last move left it.
    let (mut stm, mut f) = held_open(&open_curve());
    stm.moved(0, MoveDir::Open, 50, 50, StopReason::Target);
    to_937(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    assert!(!f.view(0).known);
}

#[test]
fn a_direct_move_waits_with_a_busy_stm_and_starts_again_when_the_valve_left_meanwhile() {
    let (mut stm, mut f) = held_open(&open_curve());
    to_937(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 49, 30));
    stm.result(&mut f, 0, FlowOutcome::Rejected(2));
    stm.now += BUSY_RETRY_MS;
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 49, 30));
    stm.result(&mut f, 0, FlowOutcome::Rejected(2));
    foreign_move(&mut stm, 0, 0);
    stm.now += BUSY_RETRY_MS;
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    // The moves of the reference follow, the event with them.
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 10000, 4000, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 3704, 34));
    direct_done(&mut stm, &mut f, MoveDir::Close, 3704, 3704);
    assert_eq!(f.view(0).counts, 937);
    assert_eq!(
        stm.codes(),
        [EventCode::FlowPositioned, EventCode::FlowPositioned]
    );
    // A link lost before the move starts from the end stop too, with nothing of the held
    // position left in the sequence.
    let (mut stm, mut f) = held_open(&open_curve());
    to_937(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 49, 30));
    stm.result(&mut f, 0, FlowOutcome::Rejected(2));
    assert_eq!(f.next(&stm.m, inputs(false, false), stm.now), None);
    stm.now += BUSY_RETRY_MS;
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 10000, 4000, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 3704, 34));
}

#[test]
fn a_direct_move_that_meets_an_end_stop_goes_on_from_the_reference_end_stop() {
    for stop in [StopReason::EndStop, StopReason::EarlyEndStop] {
        let (mut stm, mut f) = held_open(&open_curve());
        to_937(&mut stm);
        assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 49, 30));
        stm.result(&mut f, 0, FlowOutcome::Ok);
        stm.tick(&mut f);
        stm.moved(0, MoveDir::Open, 49, 20, stop);
        stm.tick(&mut f);
        // Not a failure: the same sequence, the position forgotten.
        assert_eq!(f.view(0).state, FlowState::Positioning);
        assert!(!f.view(0).known);
        assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
        stm.result(&mut f, 0, FlowOutcome::Ok);
        stm.tick(&mut f);
        stm.moved(0, MoveDir::Open, 10000, 20, StopReason::EndStop);
        stm.tick(&mut f);
        assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 3704, 34));
        direct_done(&mut stm, &mut f, MoveDir::Close, 3704, 3704);
        assert_eq!(f.view(0).counts, 937);
        assert_eq!(f.view(0).ref_delta, None);
        // The positioning through the end stop reports itself.
        assert_eq!(
            stm.codes(),
            [EventCode::FlowPositioned, EventCode::FlowPositioned]
        );
        assert_eq!((stm.events[1].arg1, stm.events[1].arg2), (937, 20));
    }
}

#[test]
fn any_other_stop_of_a_direct_move_fails_at_the_final_step() {
    let (mut stm, mut f) = held_open(&open_curve());
    to_937(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 49, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 49, 12, StopReason::UnderCurrent);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Failed);
    assert!(!f.view(0).known);
    let e = stm.events.last().expect("event");
    assert_eq!(e.code, EventCode::FlowPositionFailed);
    assert_eq!((e.arg1, e.arg2), (5, StopReason::UnderCurrent as i32));
    assert_eq!(&e.text[..], b"undercurrent");
    // The next try is a whole positioning.
    stm.flow(&open_curve(), 0, 1620);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
}

#[test]
fn a_stgtp_that_is_due_ends_the_hold_so_the_moves_go_through_the_reference_end_stop() {
    let (mut stm, mut f) = held_open(&open_curve());
    stm.ex[0].target = 50;
    stm.poll(0);
    to_937(&mut stm);
    assert_eq!(stm.next(&mut f), set_target(0, 15));
    stm.ex[0].target = 15;
    stm.poll(0);
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    // ... and a target the valve stands at already is no reason to skip it.
    let mut c = open_curve();
    c.deadband_ml = 0;
    let (mut stm, mut f) = held_open(&c);
    assert!(f.position_counts(0, 937, Served::of(stm.m.valve(0)), stm.now));
    stm.moved_through(&mut f, c.stroke_counts - 937);
    stm.now += 300_000;
    stm.ex[0].target = 50;
    stm.poll(0);
    stm.flow(&c, 0, 1348);
    assert_eq!(stm.next(&mut f), set_target(0, 15));
}

#[test]
fn a_closed_target_the_first_opening_and_the_open_end_stop_go_through_an_end_stop() {
    // Closed: the close to the closed end stop.
    let (mut stm, mut f) = held_open(&open_curve());
    stm.target(0, 0);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1388, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 1388, 800, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    assert_eq!(f.view(0).counts, 0);
    // From there the first opening goes through the open end stop.
    stm.now += 300_000;
    stm.target(0, 50);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    // A target on the open end stop: the opening move alone, no closing move after it.
    let mut c = open_curve();
    c.stroke_counts = 1180;
    let (mut stm, mut f) = held_open(&c);
    stm.target(0, 100);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 10000, 292, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!(f.view(0).counts, 1180);
    // The top of the curve at the end stop as well, however the target came.
    let (mut stm, mut f) = held_open(&c);
    stm.flow(&c, 0, 2600);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    // One pulse below it: one move, and 100 % is the end stop all the same.
    let mut c = open_curve();
    c.stroke_counts = 1181;
    let (mut stm, mut f) = held_open(&c);
    stm.flow(&c, 0, 2600);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 292, 30));
    let (mut stm, mut f) = held_open(&c);
    stm.target(0, 100);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
}

#[test]
fn a_counts_request_and_a_changed_curve_go_through_the_reference_end_stop() {
    let (mut stm, mut f) = held_open(&open_curve());
    assert!(f.position_counts(0, 937, Served::of(stm.m.valve(0)), stm.now));
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    let c = open_curve();
    let (mut stm, mut f) = held_open(&c);
    let mut changed = c;
    changed.offset = 5;
    f.set_configs(&configs(&changed, &[0]));
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    // A curve that changes during a direct move: its keep is followed by a whole positioning.
    let (mut stm, mut f) = held_open(&c);
    to_937(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 49, 30));
    f.set_configs(&configs(&changed, &[0]));
    assert_eq!(f.view(0).state, FlowState::Positioning);
    direct_done(&mut stm, &mut f, MoveDir::Open, 49, 49);
    assert_eq!(f.view(0).state, FlowState::Unknown);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
}

#[test]
fn a_target_at_the_held_pulses_is_served_without_a_move() {
    let mut c = open_curve();
    c.deadband_ml = 0;
    let (mut stm, mut f) = held_open(&c);
    assert!(f.position_counts(0, 937, Served::of(stm.m.valve(0)), stm.now));
    stm.moved_through(&mut f, c.stroke_counts - 937);
    let start = f.v[0].last_start_ms;
    stm.now += 300_000;
    stm.flow(&c, 0, 1350);
    assert_eq!(stm.next(&mut f), None);
    assert_eq!(stm.next(&mut f), None);
    assert_eq!(
        f.v[0].served,
        Some(Served {
            pct: 20,
            flow_ml: Some(1350)
        })
    );
    assert_eq!(f.v[0].last_start_ms, start);
    assert_eq!(f.view(0).state, FlowState::Held);
    // A target in % there as well.
    stm.target(0, 20); // 928 pulses
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 9, 34));
    direct_done(&mut stm, &mut f, MoveDir::Close, 9, 9);
    assert_eq!(f.view(0).counts, 928);
    stm.now += 300_000;
    stm.flow(&c, 0, 1286); // 928 pulses
    assert_eq!(stm.next(&mut f), None);
    stm.target(0, 20);
    assert_eq!(stm.next(&mut f), None);
    assert_eq!(f.v[0].served, served(20));
}

#[test]
fn a_target_at_the_held_pulses_is_the_target_shown() {
    let (mut stm, mut f) = held_open(&open_curve());
    to_937(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 49, 30));
    // 9 pulses short of the target
    direct_done(&mut stm, &mut f, MoveDir::Open, 49, 40);
    assert_eq!((f.view(0).counts, f.view(0).target_counts), (928, 937));
    stm.now += 300_000;
    stm.target(0, 20); // 928 pulses
    assert_eq!(stm.next(&mut f), None);
    assert_eq!((f.view(0).counts, f.view(0).target_counts), (928, 928));
    assert_eq!(f.v[0].target_pct, 20);
}

#[test]
fn a_target_in_percent_moves_the_valve_above_the_top_of_the_curve_too() {
    let mut c = curve();
    c.min_interval_s = 0;
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 60); // 1440 pulses, above the curve
    position(&mut stm, &mut f, 0, false, 4800, 1440);
    assert_eq!(f.view(0).flow_ml, 2600);
    stm.target(0, 70); // 1680 pulses, the same flow
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 240, 30));
    direct_done(&mut stm, &mut f, MoveDir::Open, 240, 240);
    assert_eq!(f.view(0).counts, 1680);
    assert_eq!(f.view(0).flow_ml, 2600);
}

#[test]
fn two_flows_in_the_same_whole_percent_are_two_positions() {
    let mut c = curve();
    c.min_interval_s = 0;
    c.deadband_ml = 0;
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 1000);
    position(&mut stm, &mut f, 0, false, 4800, 888);
    stm.flow(&c, 0, 1015); // 890 pulses
    assert_eq!(stm.m.valve(0).desired, 37);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 2, 30));
    direct_done(&mut stm, &mut f, MoveDir::Open, 2, 2);
    assert_eq!(f.view(0).counts, 890);
}

#[test]
fn a_target_in_percent_ends_the_flow_target_of_the_same_percent() {
    let mut c = curve();
    c.min_interval_s = 0;
    c.deadband_ml = 0;
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 1015); // 37 %, 890 pulses
    position(&mut stm, &mut f, 0, false, 4800, 890);
    stm.target(0, 37); // 888 pulses
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 2, 34));
}

#[test]
fn the_dead_band_counts_from_the_flow_of_the_held_position() {
    let mut c = curve();
    c.min_interval_s = 0;
    c.deadband_ml = 100;
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 1000);
    // The brake leaves it 20 pulses above the 888 asked for: 1.14 l/min on the curve.
    position_until_open(&mut stm, &mut f, 0, 4800);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 888, 30));
    direct_done(&mut stm, &mut f, MoveDir::Open, 888, 908);
    assert_eq!(f.view(0).flow_ml, 1143);
    stm.flow(&c, 0, 1200);
    assert_eq!(stm.next(&mut f), None);
    stm.flow(&c, 0, 999); // 0.144 l/min away, 888 pulses
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 20, 34));
}

#[test]
fn no_flow_closes_at_once_past_the_dead_band_and_the_interval() {
    let mut c = curve();
    c.deadband_ml = 2000;
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 1000);
    position(&mut stm, &mut f, 0, false, 4800, 888);
    stm.flow(&c, 0, 0);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1388, 34));
    assert_eq!(f.view(0).target_counts, 0);
}

// ================================================================ a position lost between moves

/// The requests of a positioning to 37 % with the warm-up (closed reference, the STM target
/// is the fallback already): the reference close, the warm-up opening and close, the final
/// opening, the keep ...
fn warm_requests() -> [Option<FlowRequest>; 5] {
    [
        mv(0, MoveDir::Close, 4800, 34),
        mv(0, MoveDir::Open, 300, 30),
        mv(0, MoveDir::Close, 800, 34),
        mv(0, MoveDir::Open, 888, 30),
        keep(0, true),
    ]
}

/// ... and the records of the four moves.
const WARM_RECORDS: [(MoveDir, u16, u16, StopReason); 4] = [
    (MoveDir::Close, 4800, 97, StopReason::EndStop),
    (MoveDir::Open, 300, 300, StopReason::Target),
    (MoveDir::Close, 800, 350, StopReason::EndStop),
    (MoveDir::Open, 888, 888, StopReason::Target),
];

/// Valve 0 of a layer with the warm-up after `moves` moves of its positioning to 37 %: the
/// next request is that of step `moves` + 2 (3 warm-up opening, 4 warm-up close, 5 final
/// move, 6 keep).
fn warm_after(moves: usize) -> (Stm, FlowLayer) {
    let mut c = curve();
    c.pre_close = true;
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    for (req, rec) in warm_requests().iter().zip(WARM_RECORDS).take(moves) {
        assert_eq!(stm.next(&mut f), *req);
        stm.result(&mut f, 0, FlowOutcome::Ok);
        stm.tick(&mut f);
        stm.moved(0, rec.0, rec.1, rec.2, rec.3);
        stm.tick(&mut f);
    }
    (stm, f)
}

/// A move the layer did not ask for ended (a calibration, a failsafe or target drive): its
/// record with `flags`.
fn foreign_move(stm: &mut Stm, v: u8, flags: u16) {
    let e = &mut stm.ex[usize::from(v)];
    e.last_move = MoveResult {
        dir: MoveDir::Close,
        requested_counts: 3000,
        counted_counts: 2900,
        stop: StopReason::Target,
        peak_current: 99,
        duration_ms: 9000,
    };
    e.flags = flags;
    stm.poll(v);
}

/// The reference close of an unknown position: the stroke, one pulse more when the last
/// record asked for it.
fn stroke_close(stm: &Stm) -> Option<FlowRequest> {
    let n = if stm.ex[0].last_move.requested_counts == 4800 {
        4801
    } else {
        4800
    };
    mv(0, MoveDir::Close, n, 34)
}

fn inputs(link_up: bool, lease_expired: bool) -> FlowInputs {
    FlowInputs {
        link_up,
        lease_expired,
    }
}

#[test]
fn a_rejected_move_then_a_calibration_restarts_the_sequence_from_the_end_stop() {
    for moves in 1..4 {
        let (mut stm, mut f) = warm_after(moves);
        assert_eq!(
            stm.next(&mut f),
            warm_requests()[moves],
            "step {}",
            moves + 2
        );
        stm.result(&mut f, 0, FlowOutcome::Rejected(3));
        // The calibration ends the hold and drives the valve to the STM's target.
        stm.ex[0].calibrating = true;
        stm.flags(0, 0);
        stm.tick(&mut f);
        assert_eq!(stm.next(&mut f), None);
        stm.ex[0].calibrating = false;
        foreign_move(&mut stm, 0, 0);
        stm.tick(&mut f);
        assert_eq!(stm.next(&mut f), stroke_close(&stm), "step {}", moves + 2);
        assert_eq!(f.view(0).state, FlowState::Positioning);
        // The sequence runs again from there: the warm-up opening follows the end stop.
        stm.result(&mut f, 0, FlowOutcome::Ok);
        stm.tick(&mut f);
        stm.moved(0, MoveDir::Close, 4800, 2000, StopReason::EndStop);
        stm.tick(&mut f);
        assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 300, 30));
    }
}

#[test]
fn a_lease_failsafe_between_moves_restarts_the_sequence_from_the_end_stop() {
    for moves in 1..5 {
        let (mut stm, mut f) = warm_after(moves);
        stm.now += 2;
        if moves < 4 {
            // (the keep is no move: it would go out)
            assert_eq!(f.next(&stm.m, inputs(true, true), stm.now), None);
        }
        // The STM drives the failsafe position, which ends the hold.
        foreign_move(&mut stm, 0, STM_FLAG_FS_LEASE);
        assert_eq!(f.next(&stm.m, inputs(true, true), stm.now), None);
        assert_eq!(stm.next(&mut f), None); // still at the failsafe
        stm.flags(0, 0);
        assert_eq!(stm.next(&mut f), stroke_close(&stm), "step {}", moves + 2);
    }
}

#[test]
fn a_link_loss_between_moves_restarts_the_sequence_from_the_end_stop() {
    for moves in 1..5 {
        let (mut stm, mut f) = warm_after(moves);
        stm.now += 2;
        assert_eq!(f.next(&stm.m, inputs(false, false), stm.now), None);
        // Back: the model still shows the hold and the last record, which may predate the
        // loss (the hold ran out meanwhile).
        assert_eq!(stm.next(&mut f), stroke_close(&stm), "step {}", moves + 2);
        assert!(!f.view(0).known);
    }
    // A move in flight or running when the link goes is followed by its record as usual.
    for running in [false, true] {
        let (mut stm, mut f) = warm_after(3);
        assert_eq!(stm.next(&mut f), warm_requests()[3]);
        if running {
            stm.result(&mut f, 0, FlowOutcome::Ok);
        }
        stm.now += 2;
        assert_eq!(f.next(&stm.m, inputs(false, false), stm.now), None);
        if !running {
            stm.result(&mut f, 0, FlowOutcome::Ok);
        }
        stm.tick(&mut f);
        stm.moved(0, MoveDir::Open, 888, 888, StopReason::Target);
        stm.tick(&mut f);
        assert_eq!(stm.next(&mut f), keep(0, true), "running {running}");
    }
}

#[test]
fn a_held_valve_and_a_reference_retry_do_not_depend_on_the_last_move() {
    // A link loss leaves a held valve alone: its position stays known, the next positioning
    // closes from it.
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    position(&mut stm, &mut f, 0, false, 4800, 888);
    stm.now += 2;
    assert_eq!(f.next(&stm.m, inputs(false, false), stm.now), None);
    assert!(f.view(0).known);
    stm.now += 300_000;
    stm.target(0, 100);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1388, 34));
    // The retry of a reference close that missed the end stop is the retry, hold or not.
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 1388, 1388, StopReason::Target);
    stm.tick(&mut f);
    stm.flags(0, 0);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 800, 34));
}

#[test]
fn busy_answers_longer_than_the_hold_restart_the_sequence_from_the_end_stop() {
    for moves in 1..4 {
        let (mut stm, mut f) = warm_after(moves);
        assert_eq!(stm.next(&mut f), warm_requests()[moves]);
        stm.result(&mut f, 0, FlowOutcome::Rejected(2));
        // The hold ran out: the STM drove the valve to its target.
        foreign_move(&mut stm, 0, 0);
        stm.now += BUSY_RETRY_MS;
        assert_eq!(stm.next(&mut f), stroke_close(&stm), "step {}", moves + 2);
    }
}

#[test]
fn another_record_or_a_lost_hold_alone_restarts_the_sequence() {
    // Another service move during the wait: the hold is there, the record is not ours.
    let (mut stm, mut f) = warm_after(3);
    assert_eq!(stm.next(&mut f), warm_requests()[3]);
    stm.result(&mut f, 0, FlowOutcome::Rejected(2));
    stm.moved(0, MoveDir::Open, 50, 50, StopReason::Target);
    stm.now += BUSY_RETRY_MS;
    assert_eq!(stm.next(&mut f), stroke_close(&stm));
    // The hold alone is gone (the same record): the same.
    let (mut stm, mut f) = warm_after(3);
    assert_eq!(stm.next(&mut f), warm_requests()[3]);
    stm.result(&mut f, 0, FlowOutcome::Rejected(2));
    stm.flags(0, 0);
    stm.now += BUSY_RETRY_MS;
    assert_eq!(stm.next(&mut f), stroke_close(&stm));
    // Both there: the step goes on.
    let (mut stm, mut f) = warm_after(3);
    assert_eq!(stm.next(&mut f), warm_requests()[3]);
    stm.result(&mut f, 0, FlowOutcome::Rejected(2));
    stm.flags(0, STM_FLAG_SVC_HOLD);
    stm.now += BUSY_RETRY_MS;
    assert_eq!(stm.next(&mut f), warm_requests()[3]);
}

#[test]
fn the_restart_closes_the_whole_stroke_not_a_retry_nor_the_held_position() {
    let mut c = curve();
    c.pre_close = true;
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    position_until_open(&mut stm, &mut f, 0, 4800);
    for (req, rec) in warm_requests().iter().zip(WARM_RECORDS).skip(1) {
        assert_eq!(stm.next(&mut f), *req);
        stm.result(&mut f, 0, FlowOutcome::Ok);
        stm.tick(&mut f);
        stm.moved(0, rec.0, rec.1, rec.2, rec.3);
        stm.tick(&mut f);
    }
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    assert!(f.view(0).known);
    // From the held 888: the close of 1388 misses the end stop, the retry of 800 meets it.
    stm.now += 300_000;
    stm.target(0, 100);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1388, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 1388, 1388, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 800, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 800, 120, StopReason::EndStop);
    stm.tick(&mut f);
    assert!(f.view(0).known); // the held value until the position is lost
                              // The hold is gone before the warm-up: the stroke, neither 800 nor 888 + 500.
    stm.flags(0, 0);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    assert!(!f.view(0).known);
}

// ================================================================ l/min (2.2.10)

#[test]
fn flow_target_is_the_flow_the_position_on_the_curve_and_the_clip_at_qmax_and_the_top_of_the_curve()
{
    let c = curve();
    let t = |ml| flow_target(&c, ml);
    // 1.26 l/min: 924 pulses, 38.5 % of the stroke, the nearest pulse gives 1.257.
    assert_eq!(
        t(1260),
        FlowTarget {
            ml: 1260,
            pct: 39,
            flow_ml: 1257,
            clipped: false
        }
    );
    assert_eq!(
        t(0),
        FlowTarget {
            ml: 0,
            pct: 0,
            flow_ml: 0,
            clipped: false
        }
    );
    // Any flow above nothing below the first flow of the curve (Rust 2.2.15): the point of that
    // flow, clipped.
    let least = FlowTarget {
        ml: 1,
        pct: 32,
        flow_ml: 300,
        clipped: true,
    };
    assert_eq!(t(1), least);
    assert_eq!(t(299), FlowTarget { ml: 299, ..least });
    assert_eq!(
        t(300),
        FlowTarget {
            ml: 300,
            clipped: false,
            ..least
        }
    );
    // qmax itself, at the top of the curve.
    let top = FlowTarget {
        ml: 2600,
        pct: 49,
        flow_ml: 2600,
        clipped: false,
    };
    assert_eq!(t(2600), top);
    // Above qmax it is qmax, and clipped.
    let clipped = FlowTarget {
        clipped: true,
        ..top
    };
    assert_eq!(t(2601), clipped);
    assert_eq!(t(u32::MAX), clipped);
    // A curve that ends below qmax: the last point answers, one ml/min above it is clipped.
    let mut low = curve();
    low.count = 4;
    let t = |ml| flow_target(&low, ml);
    assert_eq!(
        t(2300),
        FlowTarget {
            ml: 2300,
            pct: 45,
            flow_ml: 2300,
            clipped: false
        }
    );
    assert_eq!(
        t(2301),
        FlowTarget {
            ml: 2301,
            pct: 45,
            flow_ml: 2300,
            clipped: true
        }
    );
    // A qmax below the top of the curve: never above it through l/min.
    let mut capped = curve();
    capped.qmax_ml = 1000;
    assert_eq!(
        flow_target(&capped, 2500),
        FlowTarget {
            ml: 1000,
            pct: 37,
            flow_ml: 1000,
            clipped: true
        }
    );
    // The pulses are read back through the correction.
    let mut c2 = curve();
    c2.offset = 40;
    assert_eq!(flow_target(&c2, 1000).flow_ml, 1000);
    // No curve: everything lies above it, nothing is reachable.
    assert_eq!(
        flow_target(&FlowConfig::DEFAULT, 1),
        FlowTarget {
            ml: 1,
            pct: 0,
            flow_ml: 0,
            clipped: true
        }
    );
}

/// A curve with a point every 0.1 l/min from 0.5 on: [0, 0], then 1070 pulses at 0.5 l/min and
/// 10 pulses more for each 0.1 l/min, `points` points in all.
fn step_curve(points: usize) -> FlowConfig {
    let mut c = FlowConfig {
        enabled: true,
        count: points as u8,
        stroke_counts: 2400,
        ..FlowConfig::DEFAULT
    };
    for (i, p) in c.points[1..points].iter_mut().enumerate() {
        *p = FlowPoint {
            counts: 1070 + 10 * i as u16,
            flow_ml: 500 + 100 * i as u16,
        };
    }
    c
}

#[test]
fn a_flow_below_the_first_flow_of_the_curve_is_that_point_clipped_0_still_closes() {
    let c = step_curve(16);
    assert_eq!(validate_flow_config(&c), Ok(()));
    let at_first = FlowTarget {
        ml: 300,
        pct: 45, // 1070 of 2400
        flow_ml: 500,
        clipped: true,
    };
    assert_eq!(flow_target(&c, 300), at_first);
    for ml in [1, 300, 499] {
        assert_eq!(flow_counts(&c, ml), 1070, "{ml}");
        assert_eq!(
            flow_target(&c, ml),
            FlowTarget {
                ml: ml as u16,
                ..at_first
            }
        );
    }
    assert_eq!(
        flow_target(&c, 500),
        FlowTarget {
            ml: 500,
            clipped: false,
            ..at_first
        }
    );
    // Every point's flow is exactly its point.
    for p in &c.points()[1..] {
        let t = flow_target(&c, u32::from(p.flow_ml));
        assert_eq!(flow_counts(&c, u32::from(p.flow_ml)), p.counts);
        assert_eq!((t.flow_ml, t.clipped), (u32::from(p.flow_ml), false));
    }
    // Between two points: linear, not clipped.
    assert_eq!(flow_counts(&c, 550), 1075);
    assert!(!flow_target(&c, 550).clipped);
    // 0 closes.
    assert_eq!(flow_counts(&c, 0), 0);
    assert_eq!(
        flow_target(&c, 0),
        FlowTarget {
            ml: 0,
            pct: 0,
            flow_ml: 0,
            clipped: false
        }
    );
    // The correction applies to that point as to any other.
    let mut corrected = c;
    corrected.offset = -9;
    corrected.gain_milli = 1010;
    // 1070 x 1.01 = 1080.7 -> 1081 - 9
    assert_eq!(flow_counts(&corrected, 300), 1072);
    // A curve whose first point has a flow: its counts, now clipped.
    let mut first = curve();
    first.points[0].flow_ml = 200;
    assert_eq!(flow_counts(&first, 100), 630);
    assert!(flow_target(&first, 100).clipped);
    assert!(!flow_target(&first, 200).clipped);
    // Several points without flow: the first with one.
    let mut flat = curve();
    flat.points[1].flow_ml = 0;
    assert_eq!(flow_counts(&flat, 1), 888);
    assert!(flow_target(&flat, 999).clipped);
    assert!(!flow_target(&flat, 1000).clipped);
    // No flow anywhere: above the top, as before.
    let mut none = curve();
    none.points.iter_mut().for_each(|p| p.flow_ml = 0);
    assert_eq!(flow_counts(&none, 100), 1180);
    assert!(flow_target(&none, 100).clipped);
}

#[test]
fn below_the_first_flow_the_floor_clamp_applies_after_that_point() {
    // R2 with the step curve from the open end stop: the floor plus 60 above the first point
    // raises it, below it the point stays.
    let mut c = step_curve(16);
    c.ref_side = RefSide::Open;
    c.stroke_counts = 4641;
    c.lift_counts = 100;
    c.floor_counts = 1050;
    assert_eq!(validate_flow_config(&c), Ok(()));
    assert_eq!(flow_counts(&c, 300), 1110);
    assert_eq!(flow_counts(&c, 500), 1110);
    assert_eq!(
        (flow_target(&c, 500).clipped, flow_target(&c, 300).clipped),
        (true, true)
    );
    c.floor_counts = 900;
    assert_eq!(flow_counts(&c, 300), 1070);
    assert_eq!(
        (flow_target(&c, 500).clipped, flow_target(&c, 300).clipped),
        (false, true)
    );
    assert_eq!(flow_counts(&c, 0), 0);
}

#[test]
fn flows_below_the_first_flow_of_the_curve_are_one_position_and_need_no_move() {
    let c = open_curve();
    let (mut stm, mut f) = held_open(&c);
    stm.flow(&c, 0, 100); // 760 pulses
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 128, 34));
    direct_done(&mut stm, &mut f, MoveDir::Close, 128, 128);
    assert_eq!(f.view(0).counts, 760);
    // (beyond the dead band of 0.05 l/min from the 0.3 l/min of 760 pulses)
    for ml in [200, 1, 250] {
        stm.now += 300_000;
        stm.flow(&c, 0, ml);
        assert_eq!(stm.next(&mut f), None, "{ml}");
        assert_eq!(
            f.v[0].served,
            Some(Served {
                pct: 16,
                flow_ml: Some(ml)
            })
        );
        assert_eq!(f.view(0).counts, 760);
    }
}

#[test]
fn flow_target_json_one_member_from_0_above_qmax_it_is_clipped() {
    let c = curve();
    let ok = |s: &str| parse_flow_target_json(&c, s.as_bytes());
    assert_eq!(ok(r#"{"lpm":1.25}"#).unwrap().ml, 1250);
    assert_eq!(ok(" {\t\"lpm\" :\n2.6 } \r\n").unwrap().ml, 2600);
    assert_eq!(ok(r#"{"lpm":0}"#).unwrap().ml, 0);
    let above = ok(r#"{"lpm":2.601}"#).unwrap();
    assert_eq!((above.ml, above.clipped), (2600, true));
    assert!(!ok(r#"{"lpm":2.6}"#).unwrap().clipped);
    assert_eq!(ok(r#"{"lpm":70.5}"#).unwrap().ml, 2600); // past u16 in ml/min
    for (body, err) in [
        ("", FlowError::Malformed),
        ("[]", FlowError::Malformed),
        ("{", FlowError::Malformed),
        ("{}", FlowError::Malformed),
        (r#"{"lpm" 1}"#, FlowError::Malformed),
        (r#"{"lpm":1"#, FlowError::Malformed),
        (r#"{"lpm":1,"x":2}"#, FlowError::Malformed),
        (r#"{"lpm":1}x"#, FlowError::Malformed),
        (r#"{"flow":1}"#, FlowError::UnknownKey),
        (r#"{"lpm":"1"}"#, FlowError::Lpm),
        (r#"{"lpm":null}"#, FlowError::Lpm),
        (r#"{"lpm":1e0}"#, FlowError::Lpm),
        (r#"{"lpm":1.2345}"#, FlowError::Lpm),
        (r#"{"lpm":-0.001}"#, FlowError::Lpm),
    ] {
        assert_eq!(ok(body), Err(err), "{body}");
    }
}

// ================================================================ R2: the lift (2.2.14)

/// The open curve with a lift and a floor.
fn lift_curve(lift: u16, floor: u16) -> FlowConfig {
    let mut c = open_curve();
    c.lift_counts = lift;
    c.floor_counts = floor;
    c
}

/// The moves of an R2 positioning of valve 0 that was asked for: the open end stop (its opening
/// counts `opened`), the close of `close` pulses that counts as many, the lift of `lift` pulses
/// that counts `counted`, the keep.
fn r2(stm: &mut Stm, f: &mut FlowLayer, opened: u16, close: u16, lift: u16, counted: u16) {
    assert_eq!(stm.next(f), mv(0, MoveDir::Open, 10000, 30));
    stm.result(f, 0, FlowOutcome::Ok);
    stm.tick(f);
    stm.moved(0, MoveDir::Open, 10000, opened, StopReason::EndStop);
    stm.tick(f);
    assert_eq!(stm.next(f), mv(0, MoveDir::Close, close, 34));
    stm.result(f, 0, FlowOutcome::Ok);
    stm.tick(f);
    stm.moved(0, MoveDir::Close, close, close, StopReason::Target);
    stm.tick(f);
    assert_eq!(stm.next(f), mv(0, MoveDir::Open, lift, 30));
    stm.result(f, 0, FlowOutcome::Ok);
    stm.tick(f);
    stm.moved(0, MoveDir::Open, lift, counted, StopReason::Target);
    stm.tick(f);
    assert_eq!(stm.next(f), keep(0, true));
    stm.result(f, 0, FlowOutcome::Ok);
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
}

/// The open end stop of an R2 positioning of valve 0 to 888 pulses (lift 100, floor at most
/// 788) and its close of 3853 pulses that ends with `stop` after `counted`.
fn r2_close(stm: &mut Stm, f: &mut FlowLayer, counted: u16, stop: StopReason) {
    assert_eq!(stm.next(f), mv(0, MoveDir::Open, 10000, 30));
    stm.result(f, 0, FlowOutcome::Ok);
    stm.tick(f);
    stm.moved(0, MoveDir::Open, 10000, 4000, StopReason::EndStop);
    stm.tick(f);
    assert_eq!(stm.next(f), mv(0, MoveDir::Close, 3853, 34));
    stm.result(f, 0, FlowOutcome::Ok);
    stm.tick(f);
    stm.moved(0, MoveDir::Close, 3853, counted, stop);
    stm.tick(f);
}

/// [`r2_close`] to the target, then the lift of 100 pulses that ends with `stop`.
fn r2_lift(stm: &mut Stm, f: &mut FlowLayer, stop: StopReason) {
    r2_close(stm, f, 3853, StopReason::Target);
    assert_eq!(stm.next(f), mv(0, MoveDir::Open, 100, 30));
    stm.result(f, 0, FlowOutcome::Ok);
    stm.tick(f);
    stm.moved(0, MoveDir::Open, 100, 30, stop);
    stm.tick(f);
}

#[test]
fn r2_goes_to_the_open_end_stop_closes_to_the_target_minus_the_lift_and_opens_the_lift() {
    let c = lift_curve(100, 100);
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 1000); // 888 pulses
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 10000, 4000, StopReason::EndStop);
    stm.tick(&mut f);
    // 4641 - (888 - 100)
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 3853, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 3853, 3855, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Positioning);
    assert!(f.busy(0));
    // The lift: 100 pulses open at the threshold of an opening move.
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 100, 30));
    assert_eq!(stm.next(&mut f), None);
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), None); // the record is awaited
    stm.moved(0, MoveDir::Open, 100, 104, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    let v = f.view(0);
    // 4641 - 3855 = 786 after the close, + 104 counted by the lift
    assert_eq!(
        (v.state, v.counts, v.target_counts, v.pct),
        (FlowState::Held, 890, 888, 19)
    );
    assert_eq!(v.flow_ml, 1014);
    // The record kept is that of the closing move, whose peak shows a sticky valve; the lift is
    // the STM's last move.
    assert!(v.last_valid);
    assert_eq!((v.last_ms, v.last_peak), (3855 * 7, 123));
    assert_eq!(stm.codes(), [EventCode::FlowPositioned]);
    assert_eq!((stm.events[0].arg1, stm.events[0].arg2), (890, 19));
    assert_eq!(stm.events[0].text.as_slice(), b"");
    // The next target goes through the open end stop again, which measures the drift from the
    // held position; the lift ends it.
    stm.now += 300_000;
    stm.flow(&c, 0, 1350); // 937 pulses
    r2(&mut stm, &mut f, 3760, 3804, 100, 100); // 4641 - 837
    assert_eq!(f.view(0).counts, 937);
    assert_eq!(f.view(0).ref_delta, Some(9)); // 3760 - (4641 - 890)
    assert_eq!(f.view(0).last_ms, 3804 * 7);
    assert_eq!(stm.events.last().unwrap().text.as_slice(), b"ref +9");
    assert_eq!(
        stm.codes(),
        [EventCode::FlowPositioned, EventCode::FlowPositioned]
    );
}

#[test]
fn with_a_lift_every_new_target_goes_through_the_open_end_stop_without_one_move() {
    let mut c = lift_curve(100, 100);
    c.deadband_ml = 0;
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 1000);
    r2(&mut stm, &mut f, 4000, 3853, 100, 100);
    assert_eq!(f.view(0).counts, 888);
    stm.now += 300_000;
    // Another flow, or one in %: the open end stop first.
    stm.flow(&c, 0, 1350);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    let (mut stm, mut f) = (Stm::new(0x001), layer(&c, &[0]));
    stm.flow(&c, 0, 1000);
    r2(&mut stm, &mut f, 4000, 3853, 100, 100);
    stm.now += 300_000;
    stm.target(0, 20); // 928 pulses
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
}

#[test]
fn the_dead_band_and_the_interval_hold_a_lift_back_a_close_goes_at_once() {
    let c = lift_curve(100, 100);
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 1000);
    r2(&mut stm, &mut f, 4000, 3853, 100, 104); // 892 pulses, 1.029 l/min
                                                // within the interval
    stm.flow(&c, 0, 1350);
    assert_eq!(stm.next(&mut f), None);
    // past it, within the dead band
    stm.now += 300_000;
    stm.flow(&c, 0, 1070); // 0.041 l/min away
    assert_eq!(stm.next(&mut f), None);
    stm.flow(&c, 0, 1350);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    // A close goes out within the interval.
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 1000);
    r2(&mut stm, &mut f, 4000, 3853, 100, 104);
    stm.flow(&c, 0, 0);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1392, 34)); // 892 + 500
}

#[test]
fn the_lift_stops_at_the_floor_and_a_flow_below_the_floor_plus_60_is_clamped_to_it() {
    // 888 is 68 above the floor: the lift is 68.
    let c = lift_curve(100, 820);
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 1000);
    r2(&mut stm, &mut f, 4000, 3821, 68, 68); // 4641 - 820
    assert_eq!(f.view(0).counts, 888);
    // 888 lies below 850 + 60: the target is 910, the lift 60.
    let c = lift_curve(100, 850);
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 1000);
    r2(&mut stm, &mut f, 4000, 3791, 60, 60); // 4641 - 850
    assert_eq!((f.view(0).counts, f.view(0).target_counts), (910, 910));
    // A target in %: 10 % is 464 pulses, below 700 + 60.
    let c = lift_curve(100, 700);
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 10);
    r2(&mut stm, &mut f, 4000, 3941, 60, 60); // 4641 - 700
    assert_eq!(f.view(0).target_counts, 760);
    // The lowest floor: 1 % is 46 pulses, raised to 61, the lift 60 from 1 pulse above closed.
    let c = lift_curve(100, 1);
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 1);
    r2(&mut stm, &mut f, 4000, 4640, 60, 60);
    assert_eq!(f.view(0).target_counts, 61);
}

#[test]
fn a_flow_below_the_floor_plus_60_is_clipped_to_it_without_r2_nothing_changes() {
    let c = lift_curve(100, 800);
    // 0.6 l/min lies at 815 pulses: 860, the flow there, clipped.
    assert_eq!(
        flow_target(&c, 600),
        FlowTarget {
            ml: 600,
            pct: 19,
            flow_ml: 847,
            clipped: true
        }
    );
    assert_eq!(
        flow_target(&c, 847),
        FlowTarget {
            ml: 847,
            pct: 19,
            flow_ml: 847,
            clipped: false
        }
    );
    assert_eq!(flow_counts(&c, 840), 860); // 859 on the curve
    assert!(flow_target(&c, 840).clipped);
    // Below the first flow of the curve: its point, 760, then the clamp.
    assert_eq!(flow_counts(&c, 1), 860);
    assert_eq!(flow_counts(&c, 1000), 888);
    assert!(!flow_target(&c, 1000).clipped);
    assert!(flow_target(&c, 2601).clipped);
    assert_eq!(
        flow_target(&c, 0),
        FlowTarget {
            ml: 0,
            pct: 0,
            flow_ml: 0,
            clipped: false
        }
    );
    // No lift: the curve alone, on either reference side.
    let mut closed = curve();
    closed.floor_counts = 800;
    for c in [lift_curve(0, 800), closed] {
        assert_eq!(validate_flow_config(&c), Ok(()));
        assert_eq!(flow_counts(&c, 600), 815);
        assert!(!flow_target(&c, 600).clipped);
    }
    // The clamp ends at the range.
    assert_eq!(flow_counts(&lift_curve(100, 9939), 1000), 9999);
    assert_eq!(flow_counts(&lift_curve(100, 9940), 1000), 10000);
    assert_eq!(flow_counts(&lift_curve(100, 9990), 1000), 10000);
    assert_eq!(flow_counts(&lift_curve(100, 10000), 1000), 10000);
}

#[test]
fn a_counts_request_is_not_clamped_and_lifts_down_to_the_floor_at_most() {
    let c = lift_curve(100, 850);
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    // 888: 38 above the floor, no clamp (the fallback target first: the layer knows no STM
    // target yet).
    assert!(f.position_counts(0, 888, None, stm.now));
    assert_eq!(stm.next(&mut f), set_target(0, 15));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    r2(&mut stm, &mut f, 4000, 3791, 38, 38);
    assert_eq!((f.view(0).counts, f.view(0).target_counts), (888, 888));
    // At or below the floor: no lift, R1.
    for counts in [850, 840] {
        assert!(f.position_counts(0, counts, None, stm.now));
        stm.moved_through(&mut f, 4641 - counts);
        assert_eq!(f.view(0).counts, counts);
        assert_eq!(f.view(0).state, FlowState::Held);
    }
    // The lowest floor: a lift ends 1 pulse above closed at most.
    let c = lift_curve(100, 1);
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    assert!(f.position_counts(0, 50, None, stm.now));
    assert_eq!(stm.next(&mut f), set_target(0, 15));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    r2(&mut stm, &mut f, 4000, 4640, 49, 49);
    assert_eq!(f.view(0).counts, 50);
}

#[test]
fn the_open_end_stop_and_closed_need_no_lift() {
    let c = lift_curve(100, 100);
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 100);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 10000, 4000, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    assert_eq!(f.view(0).counts, 4641);
    stm.target(0, 0);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 5141, 34)); // 4641 + 500
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 5141, 4700, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!(f.view(0).counts, 0);
    // One pulse below the open end stop: the close and the lift.
    let mut c = lift_curve(100, 100);
    c.stroke_counts = 1181;
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 2600); // 1180 pulses
    r2(&mut stm, &mut f, 4000, 101, 100, 100); // 1181 - 1080
    assert_eq!(f.view(0).counts, 1180);
}

#[test]
fn a_new_target_at_the_position_of_a_lifted_valve_is_served_where_it_stands() {
    // Below the floor + 60 every flow is one position: 0.3, 0.2 and 0.1 l/min are all 910 pulses.
    let c = lift_curve(100, 850);
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 300);
    r2(&mut stm, &mut f, 4000, 3791, 60, 60);
    assert_eq!((f.view(0).counts, f.view(0).target_counts), (910, 910));
    let start = f.v[0].last_start_ms;
    for ml in [200, 100] {
        stm.now += 300_000;
        stm.flow(&c, 0, ml);
        assert!(flow_target(&c, u32::from(ml)).clipped);
        assert_eq!(stm.next(&mut f), None, "{ml}");
        assert_eq!(stm.next(&mut f), None, "{ml}");
        assert_eq!(f.v[0].served, Served::of(stm.m.valve(0)), "{ml}");
        let v = f.view(0);
        assert_eq!(
            (v.state, v.counts, v.target_counts),
            (FlowState::Held, 910, 910)
        );
    }
    assert_eq!(f.v[0].last_start_ms, start);
    assert_eq!(stm.codes(), [EventCode::FlowPositioned]);
    // Another position goes through the open end stop.
    stm.now += 300_000;
    stm.flow(&c, 0, 1350); // 937 pulses
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
}

#[test]
fn a_target_at_most_2_pulses_from_the_target_of_a_lifted_valve_needs_no_move() {
    let mut c = lift_curve(100, 100);
    c.deadband_ml = 0;
    let ml_at = |counts: u16| {
        (0..2600u16)
            .find(|&ml| flow_counts(&c, u32::from(ml)) == counts)
            .expect("a flow")
    };
    // Positioned to 888, the lift counted 4 pulses more: it stands at 892.
    let held = || {
        let mut f = layer(&c, &[0]);
        let mut stm = Stm::new(0x001);
        stm.flow(&c, 0, 1000);
        r2(&mut stm, &mut f, 4000, 3853, 100, 104);
        stm.now += 300_000;
        (stm, f)
    };
    // Within 2 pulses of the target (not of the held position): no move, the target stays.
    let (mut stm, mut f) = held();
    for counts in [890, 886, 888] {
        stm.flow(&c, 0, ml_at(counts));
        assert_eq!(stm.next(&mut f), None, "{counts}");
        assert_eq!(f.v[0].served, Served::of(stm.m.valve(0)), "{counts}");
        let v = f.view(0);
        assert_eq!(
            (v.state, v.counts, v.target_counts),
            (FlowState::Held, 892, 888)
        );
    }
    // 3 pulses away: the whole positioning, 1 pulse from where it stands too.
    for counts in [891, 885] {
        let (mut stm, mut f) = held();
        stm.flow(&c, 0, ml_at(counts));
        assert_eq!(
            stm.next(&mut f),
            mv(0, MoveDir::Open, 10000, 30),
            "{counts}"
        );
    }
    // A stgtp that is due ends the hold: the moves go through the open end stop.
    let (mut stm, mut f) = held();
    stm.ex[0].target = 50;
    stm.poll(0);
    stm.flow(&c, 0, ml_at(888));
    assert_eq!(stm.next(&mut f), set_target(0, 15));
    // A valve that is not held: the whole positioning.
    let (mut stm, mut f) = held();
    stm.flags(0, STM_FLAG_SVC_HOLD);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Unknown);
    stm.flow(&c, 0, ml_at(888));
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
}

#[test]
fn a_closing_move_that_meets_the_closed_end_stop_goes_on_from_the_open_end_stop_once() {
    for stop in [StopReason::EndStop, StopReason::EarlyEndStop] {
        let c = lift_curve(100, 100);
        let mut f = layer(&c, &[0]);
        let mut stm = Stm::new(0x001);
        stm.flow(&c, 0, 1000);
        r2_close(&mut stm, &mut f, 3700, stop);
        // No lift from the seat: the same sequence, the position forgotten.
        assert_eq!(f.view(0).state, FlowState::Positioning);
        assert!(!f.view(0).known);
        r2_close(&mut stm, &mut f, 3700, stop);
        // The second time it fails at the closing move, step 5.
        assert_eq!(f.view(0).state, FlowState::Failed);
        let e = stm.events.last().expect("event");
        assert_eq!(
            (e.code, e.arg1, e.arg2),
            (EventCode::FlowPositionFailed, 5, R2_END_STOP_AGAIN)
        );
        assert_eq!(&e.text[..], b"end stop again");
        assert_eq!(stm.codes(), [EventCode::FlowPositionFailed]);
        // One such end per positioning: a close at the end stop, then a lift at one.
        let mut f = layer(&c, &[0]);
        let mut stm = Stm::new(0x001);
        stm.flow(&c, 0, 1000);
        r2_close(&mut stm, &mut f, 3700, stop);
        r2_lift(&mut stm, &mut f, stop);
        let e = stm.events.last().expect("event");
        assert_eq!((e.arg1, e.arg2), (7, R2_END_STOP_AGAIN));
    }
    // Without a lift the close that meets the end stop is where the valve stands, as in 2.2.13.
    let c = open_curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 1000);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 10000, 4000, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 3753, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 3753, 3700, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!(f.view(0).counts, 941); // 4641 - 3700
}

#[test]
fn a_lift_that_meets_an_end_stop_goes_on_from_the_reference_end_stop_once() {
    for stop in [StopReason::EndStop, StopReason::EarlyEndStop] {
        let c = lift_curve(100, 100);
        let mut f = layer(&c, &[0]);
        let mut stm = Stm::new(0x001);
        stm.flow(&c, 0, 1000);
        r2_lift(&mut stm, &mut f, stop);
        // Not a failure: the same sequence, the position forgotten.
        assert_eq!(f.view(0).state, FlowState::Positioning);
        assert!(!f.view(0).known);
        r2_lift(&mut stm, &mut f, stop);
        // The second time it fails at the lift, step 7.
        assert_eq!(f.view(0).state, FlowState::Failed);
        let e = stm.events.last().expect("event");
        assert_eq!(e.code, EventCode::FlowPositionFailed);
        assert_eq!((e.arg1, e.arg2), (7, R2_END_STOP_AGAIN));
        assert_eq!(&e.text[..], b"end stop again");
        assert_eq!(stm.codes(), [EventCode::FlowPositionFailed]);
        // The retry is a whole positioning, which may meet an end stop once again.
        stm.now += 300_000;
        r2_lift(&mut stm, &mut f, stop);
        assert_eq!(f.view(0).state, FlowState::Positioning);
    }
}

#[test]
fn any_other_stop_or_a_rejection_of_the_lift_fails_at_step_7() {
    let c = lift_curve(100, 100);
    let until_lift = |stm: &mut Stm, f: &mut FlowLayer| {
        stm.flow(&c, 0, 1000);
        r2_close(stm, f, 3853, StopReason::Target);
        assert_eq!(stm.next(f), mv(0, MoveDir::Open, 100, 30));
    };
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    until_lift(&mut stm, &mut f);
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 100, 12, StopReason::UnderCurrent);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Failed);
    let e = stm.events.last().expect("event");
    assert_eq!((e.arg1, e.arg2), (7, StopReason::UnderCurrent as i32));
    assert_eq!(&e.text[..], b"undercurrent");
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    until_lift(&mut stm, &mut f);
    stm.result(&mut f, 0, FlowOutcome::Rejected(5));
    let e = stm.events.last().expect("event");
    assert_eq!(
        (e.code, e.arg1, e.arg2),
        (EventCode::FlowPositionFailed, 7, 5)
    );
    // A busy STM: the lift again after 3 s.
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    until_lift(&mut stm, &mut f);
    stm.result(&mut f, 0, FlowOutcome::Rejected(2));
    assert_eq!(stm.next(&mut f), None);
    stm.now += BUSY_RETRY_MS;
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 100, 30));
}

#[test]
fn a_valve_that_left_where_the_close_put_it_starts_again_from_the_end_stop() {
    let c = lift_curve(100, 100);
    let until_lift = |stm: &mut Stm, f: &mut FlowLayer| {
        stm.flow(&c, 0, 1000);
        r2_close(stm, f, 3853, StopReason::Target);
    };
    // The hold ran out before the lift.
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    until_lift(&mut stm, &mut f);
    stm.flags(0, 0);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    assert!(!f.view(0).known);
    // Another move of the valve.
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    until_lift(&mut stm, &mut f);
    foreign_move(&mut stm, 0, STM_FLAG_SVC_HOLD);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    // The link went.
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    until_lift(&mut stm, &mut f);
    stm.now += 2;
    assert_eq!(f.next(&stm.m, inputs(false, false), stm.now), None);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
}

#[test]
fn a_new_lift_or_floor_positions_a_held_valve_again() {
    let (mut stm, mut f) = held_open(&open_curve());
    assert_eq!(f.view(0).state, FlowState::Held);
    let c = lift_curve(100, 100);
    f.set_configs(&configs(&c, &[0]));
    assert_eq!(f.view(0).state, FlowState::Unknown);
    r2(&mut stm, &mut f, 3753, 3853, 100, 100);
    assert_eq!(f.view(0).counts, 888);
    let floor = lift_curve(100, 820);
    f.set_configs(&configs(&floor, &[0]));
    assert_eq!(f.view(0).state, FlowState::Unknown);
    r2(&mut stm, &mut f, 3753, 3821, 68, 68);
    // The same configuration again: nothing.
    f.set_configs(&configs(&floor, &[0]));
    assert_eq!(f.view(0).state, FlowState::Held);
}

#[test]
fn a_floor_without_a_lift_changes_nothing() {
    let c = lift_curve(0, 850);
    let (mut stm, mut f) = held_open(&c);
    assert_eq!(f.view(0).counts, 888);
    to_937(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 49, 30));
    assert!(!flow_target(&c, 1000).clipped);
}

#[test]
fn after_an_r2_restart_ref_delta_is_that_of_the_first_move_to_the_end_stop_or_none() {
    let c = lift_curve(100, 100);
    let step = |stm: &mut Stm, f: &mut FlowLayer, dir, req, cnt, stop| {
        let ma = if dir == MoveDir::Open { 30 } else { 34 };
        assert_eq!(stm.next(f), mv(0, dir, req, ma));
        stm.result(f, 0, FlowOutcome::Ok);
        stm.tick(f);
        stm.moved(0, dir, req, cnt, stop);
        stm.tick(f);
    };
    // To 937 pulses: the open end stop, a close that meets the closed end stop, the open end
    // stop again from the seat, the close, the lift, the keep.
    let restarted = |stm: &mut Stm, f: &mut FlowLayer, opened| {
        step(stm, f, MoveDir::Open, 10000, opened, StopReason::EndStop);
        step(stm, f, MoveDir::Close, 3804, 3700, StopReason::EndStop);
        step(stm, f, MoveDir::Open, 10000, 4641, StopReason::EndStop);
        step(stm, f, MoveDir::Close, 3804, 3804, StopReason::Target);
        step(stm, f, MoveDir::Open, 100, 100, StopReason::Target);
        assert_eq!(stm.next(f), keep(0, true));
        stm.result(f, 0, FlowOutcome::Ok);
        stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    };
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 1000);
    r2(&mut stm, &mut f, 4000, 3853, 100, 104); // held at 892
    assert_eq!(f.view(0).ref_delta, None);
    // From the held position the first opening counts 3758: 3758 - (4641 - 892) = +9; the
    // opening from the seat measures nothing.
    stm.now += 300_000;
    stm.flow(&c, 0, 1350);
    restarted(&mut stm, &mut f, 3758);
    assert_eq!((f.view(0).state, f.view(0).counts), (FlowState::Held, 937));
    assert_eq!(f.view(0).ref_delta, Some(9));
    assert_eq!(stm.events.last().expect("event").text.as_slice(), b"ref +9");
    // The hold is lost: the next positioning starts from no held position and, started again,
    // shows none, not the +9 of the last one.
    stm.flags(0, STM_FLAG_SVC_HOLD);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Unknown);
    restarted(&mut stm, &mut f, 4000);
    assert_eq!(f.view(0).state, FlowState::Held);
    assert_eq!(f.view(0).ref_delta, None);
    assert_eq!(stm.events.last().expect("event").text.as_slice(), b"");
}
