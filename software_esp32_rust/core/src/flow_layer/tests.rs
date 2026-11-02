//! The flow layer (Rust 2.2.8): configuration rules, blob and JSON forms, the curve mapping
//! both ways and the positioning state machine against a valve model fed with gvlvy data.

use std::string::String;
use std::vec;
use std::vec::Vec;

use super::*;
use crate::stm_codec::{MoveResult, ValveEx, STM_FLAG_ASSEMBLY, STM_FLAG_SVC_HOLD};
use crate::valve_model::TargetSource;

/// west 1 of the plan: 593 pulses without flow, 2.5 l/min at 1016, qmax 2.7; 37 % of the stroke is
/// 809 pulses.
fn curve() -> FlowConfig {
    let mut c = FlowConfig {
        enabled: true,
        count: 5,
        stroke_counts: 2187,
        ..FlowConfig::DEFAULT
    };
    c.points[0] = FlowPoint {
        counts: 593,
        flow_ml: 0,
    };
    c.points[1] = FlowPoint {
        counts: 713,
        flow_ml: 250,
    };
    c.points[2] = FlowPoint {
        counts: 809,
        flow_ml: 1000,
    };
    c.points[3] = FlowPoint {
        counts: 1016,
        flow_ml: 2500,
    };
    c.points[4] = FlowPoint {
        counts: 1100,
        flow_ml: 2700,
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

/// The same loop measured from the open end stop: 4701 pulses of stroke.
fn open_curve() -> FlowConfig {
    let mut c = curve();
    c.ref_side = RefSide::Open;
    c.stroke_counts = 4701;
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
    assert_eq!(c.count, 0);
    assert!(c.points().is_empty());
    assert_eq!(validate_flow_config(&c), Ok(()));
    // The limits.
    assert_eq!(FLOW_POINTS_MIN, 2);
    assert_eq!(FLOW_POINTS_MAX, 16);
    assert_eq!(FLOW_COUNTS_MAX, 10000);
    assert_eq!((FLOW_QMAX_MIN_ML, FLOW_QMAX_MAX_ML), (10, 20000));
    assert_eq!(FLOW_DEADBAND_MAX_ML, 5000);
    assert_eq!(FLOW_INTERVAL_MAX_S, 3600);
    assert_eq!(FLOW_OFFSET_MAX, 10000);
    assert_eq!((FLOW_GAIN_MIN_MILLI, FLOW_GAIN_MAX_MILLI), (100, 10000));
    assert_eq!((FLOW_MA_MIN, FLOW_MA_MAX), (5, 60));
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
        (|c| c.count = 17, FlowError::PointCount),
        (|c| c.count = 0, FlowError::PointCount),
        (|c| c.points[2].counts = 713, FlowError::PointCounts),
        (|c| c.points[2].counts = 700, FlowError::PointCounts),
        (|c| c.points[4].counts = 10001, FlowError::PointCounts),
        (|c| c.points[2].flow_ml = 249, FlowError::PointFlow),
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
    ok.points[2].flow_ml = 250; // equal flows are allowed
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
    // 16 points.
    let mut full = curve();
    full.count = 16;
    for (i, p) in full.points.iter_mut().enumerate() {
        *p = FlowPoint {
            counts: 100 * (i as u16 + 1),
            flow_ml: 100 * i as u16,
        };
    }
    assert_eq!(validate_flow_config(&full), Ok(()));
    assert_eq!(full.points().len(), 16);
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
        (FlowError::PointCount, "points 2..16 (enabled needs them)"),
        (FlowError::PointCounts, "points counts 0..10000, increasing"),
        (FlowError::PointFlow, "points lpm 0..65.535, non-decreasing"),
        (FlowError::RefSide, "refSide closed|open"),
        (FlowError::PreClose, "preClose true|false"),
        (
            FlowError::StrokeCounts,
            "strokeCounts 0..10000 (enabled or refSide open needs 1..)",
        ),
        (FlowError::Lpm, "lpm a plain number from 0"),
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
    assert_eq!(curve_counts(&c, 0), 593);
    assert_eq!(curve_counts(&c, 250), 713);
    assert_eq!(curve_counts(&c, 125), 653);
    assert_eq!(curve_counts(&c, 1000), 809);
    assert_eq!(curve_counts(&c, 1750), 913); // 809 + 750/1500 x 207 = 912.5 -> 913
    assert_eq!(curve_counts(&c, 2700), 1100);
    assert_eq!(curve_counts(&c, 5000), 1100);
    // A flat segment: its first position.
    let mut flat = c;
    flat.points[2].flow_ml = 250;
    assert_eq!(curve_counts(&flat, 250), 713);
    assert_eq!(curve_counts(&flat, 251), 809); // 809 + 1/2250 x 207 -> 809.09 -> 809
    assert_eq!(curve_counts(&FlowConfig::DEFAULT, 100), 0);
    // The flow targets: 0 is closed, otherwise the corrected counts.
    assert_eq!(flow_counts(&c, 0), 0);
    assert_eq!(flow_counts(&c, 27), 606); // 593 + 27/250 x 120 = 605.96
    assert_eq!(flow_counts(&c, 999), 809); // 808.87
    assert_eq!(flow_counts(&c, 2700), 1100);
    let mut corrected = c;
    corrected.offset = -9;
    corrected.gain_milli = 1010;
    assert_eq!(flow_counts(&corrected, 999), 808); // 809 x 1.01 = 817.09 -> 817 - 9
    corrected.offset = -10000;
    assert_eq!(flow_counts(&corrected, 999), 1); // never below 1
    corrected.offset = 10000;
    assert_eq!(flow_counts(&corrected, 999), 10000); // never above 10000
                                                     // qmax is the limit, whatever the curve goes on to.
    let mut low = c;
    low.qmax_ml = 1000;
    assert_eq!(flow_counts(&low, 2500), 809);
    assert_eq!(flow_counts(&low, 1000), 809);
    assert_eq!(flow_counts(&low, 999), 809);
}

#[test]
fn a_target_is_a_percentage_of_the_stroke_and_the_curve_has_no_part_in_it() {
    let mut c = curve();
    c.stroke_counts = 4701;
    assert_eq!(target_counts(&c, 0), 0);
    assert_eq!(target_counts(&c, 1), 47);
    assert_eq!(target_counts(&c, 37), 1739); // 1739.37
    assert_eq!(target_counts(&c, 38), 1786); // 1786.38
    assert_eq!(target_counts(&c, 50), 2351); // 2350.5
    assert_eq!(target_counts(&c, 100), 4701);
    c.stroke_counts = 10000;
    assert_eq!(target_counts(&c, 100), 10000);
    assert_eq!(target_counts(&c, 7), 700);
    let mut corrected = curve();
    corrected.offset = -9;
    corrected.gain_milli = 1010;
    assert_eq!(target_counts(&corrected, 37), 809);
    corrected.count = 0;
    assert_eq!(target_counts(&corrected, 37), 809);
    assert_eq!(target_counts(&FlowConfig::DEFAULT, 37), 0);
}

#[test]
fn a_position_in_percent_of_the_stroke_is_rounded_and_ends_at_100() {
    let mut c = curve();
    c.stroke_counts = 4701;
    assert_eq!(stroke_pct(&c, 0), 0);
    assert_eq!(stroke_pct(&c, 23), 0); // 0.49
    assert_eq!(stroke_pct(&c, 24), 1); // 0.51
    assert_eq!(stroke_pct(&c, 1739), 37);
    assert_eq!(stroke_pct(&c, 4701), 100);
    assert_eq!(stroke_pct(&c, 10000), 100); // past the stroke
    assert_eq!(stroke_pct(&FlowConfig::DEFAULT, 500), 0);
    for stroke in [100u16, 2187, 4162, 4961, 10000] {
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
    assert_eq!(curve_flow(&c, 593), 0);
    assert_eq!(curve_flow(&c, 653), 125);
    assert_eq!(curve_flow(&c, 713), 250);
    assert_eq!(curve_flow(&c, 913), 1754); // 1000 + 104/207 x 1500 = 1753.6
    assert_eq!(curve_flow(&c, 1100), 2700);
    assert_eq!(curve_flow(&c, 5000), 2700);
    assert_eq!(curve_flow(&FlowConfig::DEFAULT, 500), 0);
    assert_eq!(counts_flow(&c, 0), 0);
    assert_eq!(counts_flow(&c, 809), 1000);
    assert_eq!(counts_flow(&c, 1), 0);
    let mut corrected = c;
    corrected.offset = -9;
    corrected.gain_milli = 1010;
    assert_eq!(counts_flow(&corrected, 808), 1000); // (808 + 9) / 1.01 = 808.9 -> 809
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
    for counts in [0u16, 1, 593, 700, 809, 1100, 4701, 10000] {
        assert_eq!(counts_flow(&o, counts), counts_flow(&c, counts), "{counts}");
    }
    assert_eq!(target_counts(&o, 37), 809);
    assert_eq!(flow_counts(&o, 999), 809);
}

// ================================================================ blob

#[test]
fn blob_round_trip_and_rejections() {
    assert_eq!(FLOW_RECORD_SIZE, 83);
    assert_eq!(FLOW_BLOB_SIZE, 1006);
    let mut all = configs(&curve(), &[0, 11]);
    all[5] = open_curve();
    all[5].pre_close = true;
    all[5].offset = -17;
    let mut bytes = [0xEE; FLOW_BLOB_SIZE];
    assert_eq!(encode_flow_configs(&all, &mut bytes), FLOW_BLOB_SIZE);
    assert_eq!(&bytes[..6], b"VDMF\x01\x0C");
    // valve 0: enabled, 2700 = 0x0A8C, 34, 30, 15, 60 = 0x003C, 50, 0, 1000 = 0x03E8,
    // closed, no warm-up, 2187 = 0x088B, 5 points, the first 593 = 0x0251 at 0.
    assert_eq!(
        &bytes[6..29],
        &[
            1, 0x8C, 0x0A, 34, 30, 15, 0x3C, 0x00, 50, 0, 0, 0, 0xE8, 0x03, 0, 0, 0x8B, 0x08, 5,
            0x51, 0x02, 0, 0
        ]
    );
    let valve5 = 6 + 5 * FLOW_RECORD_SIZE;
    assert_eq!(bytes[valve5 + 14], 1); // open
    assert_eq!(bytes[valve5 + 15], 1); // preClose
    assert_eq!(&bytes[valve5 + 16..valve5 + 18], &4701u16.to_le_bytes()); // strokeCounts
    assert_eq!(&bytes[valve5 + 10..valve5 + 12], &(-17i16).to_le_bytes());
    let mut back = [FlowConfig::DEFAULT; 12];
    assert!(decode_flow_configs(&bytes, &mut back));
    assert_eq!(back, all);
    // Rejections: the length, the magic, the version, the count, the CRC, a bad record.
    let mut out = configs(&curve(), &[3]);
    assert!(!decode_flow_configs(&bytes[..FLOW_BLOB_SIZE - 1], &mut out));
    assert_eq!(out, [FlowConfig::DEFAULT; 12]);
    let mut bad = bytes;
    bad[0] = b'X';
    assert!(!decode_flow_configs(&bad, &mut out));
    let mut bad = bytes;
    bad[4] = 2;
    assert!(!decode_flow_configs(&bad, &mut out));
    let mut bad = bytes;
    bad[5] = 11;
    assert!(!decode_flow_configs(&bad, &mut out));
    let mut bad = bytes;
    bad[FLOW_BLOB_SIZE - 1] ^= 1;
    assert!(!decode_flow_configs(&bad, &mut out));
    let fix = |b: &mut [u8; FLOW_BLOB_SIZE]| {
        let crc = crc32(&b[..FLOW_CRC_AT], 0);
        b[FLOW_CRC_AT..].copy_from_slice(&crc.to_le_bytes());
    };
    for (at, value) in [(6, 2u8), (6 + 14, 2), (6 + 15, 2), (6 + 3, 4), (6 + 18, 1)] {
        let mut bad = bytes;
        bad[at] = value;
        fix(&mut bad);
        let mut out = configs(&curve(), &[3]);
        assert!(!decode_flow_configs(&bad, &mut out), "byte {at}");
        assert_eq!(out, [FlowConfig::DEFAULT; 12]);
    }
    // Every valve at the defaults decodes to the defaults.
    let all = [FlowConfig::DEFAULT; 12];
    assert_eq!(encode_flow_configs(&all, &mut bytes), FLOW_BLOB_SIZE);
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

// ================================================================ JSON

#[test]
fn json_document_and_patch_round_trip() {
    let c = curve();
    let text = json(&c);
    assert_eq!(
        text,
        concat!(
            r#"{"enabled":true,"qmax":2.700,"refmA":34,"openmA":30,"fallbackPct":15,"#,
            r#""minIntervalS":60,"deadbandLpm":0.050,"offset":0,"gain":1.000,"#,
            r#""refSide":"closed","strokeCounts":2187,"preClose":false,"#,
            r#""points":[[593,0.000],[713,0.250],[809,1.000],[1016,2.500],[1100,2.700]]}"#
        )
    );
    assert_eq!(patched(&FlowConfig::DEFAULT, &text), Ok(c));
    let mut open = open_curve();
    open.pre_close = true;
    open.offset = -17;
    open.gain_milli = 995;
    open.deadband_ml = 5;
    let text = json(&open);
    assert!(text.contains(r#""refSide":"open","strokeCounts":4701,"preClose":true,"#));
    assert!(text.contains(r#""offset":-17,"gain":0.995,"#));
    assert!(text.contains(r#""deadbandLpm":0.005,"#));
    assert_eq!(patched(&FlowConfig::DEFAULT, &text), Ok(open));
    assert_eq!(json(&FlowConfig::DEFAULT).len(), 197);
    assert!(json(&FlowConfig::DEFAULT).ends_with(r#""points":[]}"#));
}

#[test]
fn json_patch_keeps_absent_members_and_takes_every_number_form() {
    let c = curve();
    let p = patched(&c, r#"{"offset":12,"gain":1.5}"#).expect("ok");
    assert_eq!((p.offset, p.gain_milli), (12, 1500));
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
        r#"{"refSide":"open","strokeCounts":4701,"preClose":true}"#,
    )
    .expect("ok");
    assert_eq!(p.ref_side, RefSide::Open);
    assert_eq!(p.stroke_counts, 4701);
    assert!(p.pre_close);
    assert_eq!(p.points(), c.points());
    let p = patched(&c, r#"{"enabled":false,"points":[]}"#).expect("ok");
    assert_eq!(p.count, 0);
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
        (
            "{\"points\":[[1,0],[2,0],[3,0],[4,0],[5,0],[6,0],[7,0],[8,0],[9,0],[10,0],[11,0],[12,0],[13,0],[14,0],[15,0],[16,0],[17,0]]}",
            FlowError::PointCount,
        ),
    ];
    for (body, err) in cases {
        assert_eq!(patched(&c, body), Err(err), "{body}");
    }
    // Without a stroke: no layer and no open reference.
    let d = FlowConfig::DEFAULT;
    for body in [r#"{"refSide":"open"}"#, r#"{"enabled":true}"#] {
        assert_eq!(patched(&d, body), Err(FlowError::StrokeCounts), "{body}");
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
        open_count: 4701,
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
    // 37 % of the stroke is 809 pulses; the STM target is 15 already? No: 50.
    stm.ex[0].target = 50;
    stm.poll(0);
    assert_eq!(stm.next(&mut f), set_target(0, 15));
    assert_eq!(stm.next(&mut f), None); // one request at a time
    assert_eq!(f.view(0).state, FlowState::Positioning);
    assert_eq!(f.view(0).target_counts, 809);
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
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 809, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 809, 809, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    assert_eq!(f.view(0).state, FlowState::Positioning);
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!(stm.codes(), [EventCode::FlowPositioned]);
    assert_eq!(
        (stm.events[0].valve, stm.events[0].arg1, stm.events[0].arg2),
        (0, 809, 37)
    );
    let v = f.view(0);
    assert_eq!(v.state, FlowState::Held);
    assert!(v.known);
    assert_eq!(
        (v.counts, v.target_counts, v.flow_ml, v.pct),
        (809, 809, 1000, 37)
    );
    assert!(v.last_valid);
    assert_eq!((v.last_ms, v.last_peak), (809 * 7, 123));
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
    // The STM target is 15 (the fallback): the close comes first; 1094 pulses.
    position(&mut stm, &mut f, 0, false, 4800, 1094);
    assert_eq!(f.view(0).state, FlowState::Held);
    // A target only the reference reaches, after the interval: no stgtp again, the close counts
    // from the held position + 500.
    stm.now += 300_000;
    stm.target(0, 100);
    position(&mut stm, &mut f, 0, false, 1594, 2187);
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
    position(&mut stm, &mut f, 2, false, 500, 809);
    stm.target(2, 0);
    assert_eq!(stm.next(&mut f), mv(2, MoveDir::Close, 1309, 34));
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
    position(&mut stm, &mut f, 0, false, 4800, 809);
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
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 503, 30));
    assert_eq!(f.view(0).target_counts, 1312);
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
    position(&mut stm, &mut f, 0, false, 4800, 809);
    stm.flow(&c, 0, 1099);
    assert_eq!(stm.next(&mut f), None);
    stm.flow(&c, 0, 901);
    assert_eq!(stm.next(&mut f), None);
    stm.flow(&c, 0, 1100); // 823 pulses
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
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 831, 30));
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
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 831, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 831, 400, StopReason::Aborted);
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Failed);
    assert_eq!(stm.events[1].arg1, 5);
    assert_eq!(&stm.events[1].text[..], b"aborted");
    // An opening move that meets the open end stop is a position too.
    stm.target(0, 39);
    position_until_open(&mut stm, &mut f, 0, 4800);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 853, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 853, 700, StopReason::EarlyEndStop);
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
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 831, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 831, 831, StopReason::Target);
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
    position(&mut stm, &mut f, 0, false, 4800, 809);
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

#[test]
fn a_failed_positioning_is_tried_again_5_min_after_the_failure_to_the_ms() {
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 4800, 34));
    stm.result(&mut f, 0, FlowOutcome::Rejected(1));
    assert_eq!(f.view(0).state, FlowState::Failed);
    let at = stm.now;
    let inp = inputs(true, false);
    assert_eq!(f.next(&stm.m, inp, at + FAILED_RETRY_MS - 1), None);
    assert_eq!(
        f.next(&stm.m, inp, at + FAILED_RETRY_MS),
        mv(0, MoveDir::Close, 4800, 34)
    );
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
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 809, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    // The record is there while the valve still opens: the keep waits.
    stm.ex[0].status = 2;
    stm.moved(0, MoveDir::Open, 809, 809, StopReason::Target);
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
    position(&mut stm, &mut f, 0, false, 4800, 809);
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
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 809, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 809, 809, StopReason::Target);
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
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 809, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 809, 809, StopReason::Target);
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
    position(&mut stm, &mut f, 0, false, 4800, 809);
    stm.target(1, 50);
    assert_eq!(stm.next(&mut f), mv(1, MoveDir::Close, 4800, 34));
    stm.rebooted(&mut f);
    assert!(f.take_dirty());
    assert_eq!(f.view(0).state, FlowState::Unknown);
    assert_eq!(f.view(1).state, FlowState::Unknown);
    assert!(!f.view(0).known);
    assert!(f.view(0).last_valid); // the last move stays on record
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
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 809, 30));
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
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 809, 30));
}

#[test]
fn counts_request_positions_at_once_and_counts_the_desired_target_as_served() {
    let c = curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 37);
    position(&mut stm, &mut f, 0, false, 4800, 809);
    // 900 corrected pulses at once (the interval is not over), the view with its own %.
    assert!(f.position_counts(0, 900, served(37), stm.now));
    assert!(f.take_dirty());
    assert_eq!(f.view(0).state, FlowState::Positioning);
    assert_eq!(f.view(0).target_counts, 900);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1309, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 1309, 100, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 900, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 900, 900, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    let v = f.view(0);
    assert_eq!((v.counts, v.flow_ml, v.pct), (900, 1659, 41));
    assert_eq!(stm.events.last().map(|e| e.arg2), Some(41));
    // The desired 37 is served: no positioning back, until it changes.
    stm.now += 400_000;
    assert_eq!(stm.next(&mut f), None);
    stm.target(0, 100);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1400, 34));
    // A counts request during a positioning follows it; the limit is 10000; 0 closes.
    assert!(f.position_counts(0, 12000, served(100), stm.now));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 1400, 100, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 2187, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 2187, 2187, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 2687, 34));
    assert_eq!(f.view(0).target_counts, 10000);
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 2687, 100, StopReason::EndStop);
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
    position(&mut stm, &mut f, 0, false, 4800, 809);
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
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1309, 34));
    assert_eq!(f.view(0).target_counts, 814);
    // A change during the positioning: another one follows the keep.
    c2.gain_milli = 1001;
    f.set_configs(&configs(&c2, &[0]));
    assert_eq!(f.view(0).state, FlowState::Positioning);
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 1309, 100, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 814, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 814, 814, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!(f.view(0).state, FlowState::Unknown);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1314, 34));
    assert_eq!(f.view(0).target_counts, 815);
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
    position(&mut stm, &mut f, 0, false, 4800, 809);
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
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 809, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 809, 809, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!(f.view(0).counts, 809);
    // The warm-up close that misses the end stop is repeated; a warm-up opening that meets
    // the open end stop goes on; a target of 0 skips the warm-up.
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    stm.now += 300_000;
    stm.target(0, 100);
    position_until_open(&mut stm, &mut f, 0, 1309);
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
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 2187, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 2187, 200, StopReason::UnderCurrent);
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
    assert_eq!(stm.next(&mut f), mv(2, MoveDir::Close, 3892, 34));
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
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 3892, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 3892, 3892, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    let v = f.view(0);
    assert_eq!(v.state, FlowState::Held);
    assert_eq!((v.counts, v.flow_ml, v.pct), (809, 1000, 17)); // 4701 - 3892
    assert_eq!((v.last_ms, v.last_peak), (3892 * 7, 123));
    // No held position before the first positioning.
    assert_eq!(v.ref_delta, None);
    // A positioning that goes through the end stop again (the whole range, one more than the
    // record when equal); the final close may meet the closed end stop.
    assert!(f.position_counts(0, 716, Served::of(stm.m.valve(0)), stm.now));
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 10000, 3892, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 3985, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 3985, 4000, StopReason::EarlyEndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.flags(0, STM_FLAG_SVC_HOLD + STM_FLAG_SVC_KEEP);
    assert_eq!(f.view(0).counts, 701); // 4701 - 4000
    assert_eq!(f.view(0).pct, 15); // of 4701
    assert_eq!(f.view(0).ref_delta, Some(0));
    // A target of 0 closes to the closed reference as always.
    stm.target(0, 0);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1201, 34)); // 701 + 500
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 1201, 700, StopReason::EndStop);
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
        if counts != 809 || f.view(0).known {
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
    let v = go(&mut stm, &mut f, 809, 4000, StopReason::EndStop, 3892);
    assert_eq!((v.counts, v.ref_delta), (808, None));
    assert_eq!(stm.events.last().unwrap().text.as_slice(), b"");
    let v = go(&mut stm, &mut f, 716, 3919, StopReason::EndStop, 3985);
    assert_eq!((v.counts, v.ref_delta), (715, Some(26))); // 3919 - (4701 - 808)
    assert_eq!(stm.events.last().unwrap().text.as_slice(), b"ref +26");
    let v = go(&mut stm, &mut f, 809, 3970, StopReason::EarlyEndStop, 3892);
    assert_eq!((v.counts, v.ref_delta), (808, Some(-16))); // 3970 - (4701 - 715)
    assert_eq!(stm.events.last().unwrap().text.as_slice(), b"ref -16");
}

#[test]
fn ref_delta_needs_the_first_reference_move_from_a_held_position() {
    let c = open_curve();
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 999);
    position_open(&mut stm, &mut f, 0, 3892);
    assert_eq!(f.view(0).counts, 809);
    // The first move stops short of the end stop: the count of the second one is only its end.
    assert!(f.position_counts(0, 716, Served::of(stm.m.valve(0)), stm.now));
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
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 3985, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 3985, 3985, StopReason::Target);
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
    assert_eq!(stm.next(&mut f), mv(1, MoveDir::Open, 809, 30));
    stm.result(&mut f, 1, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(1, MoveDir::Open, 809, 9800, StopReason::Target);
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

/// Valve 0 of `c`, an open reference, held at 809 pulses for 1 l/min, the interval over.
fn held_open(c: &FlowConfig) -> (Stm, FlowLayer) {
    let mut f = layer(c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(c, 0, 1000);
    position_open(&mut stm, &mut f, 0, c.stroke_counts - 809);
    stm.now += 300_000;
    (stm, f)
}

/// A target of 1.35 l/min on valve 0 of the open curve: 857 pulses.
fn to_857(stm: &mut Stm) {
    stm.flow(&open_curve(), 0, 1350);
}

/// A target of 1 l/min on valve 0 of the open curve: 809 pulses.
fn to_809(stm: &mut Stm) {
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
    to_857(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 48, 30));
    assert!(f.busy(0));
    let v = f.view(0);
    assert_eq!(
        (v.state, v.counts, v.target_counts),
        (FlowState::Positioning, 809, 857)
    );
    assert!(v.known);
    // The counted pulses hold the brake's: 51, not the 48 asked for.
    direct_done(&mut stm, &mut f, MoveDir::Open, 48, 51);
    let v = f.view(0);
    assert_eq!(
        (v.state, v.counts, v.target_counts),
        (FlowState::Held, 860, 857)
    );
    assert_eq!((v.last_ms, v.last_peak), (51 * 7, 123));
    stm.tick(&mut f);
    assert_eq!(f.view(0).state, FlowState::Held);
    assert_eq!(stm.next(&mut f), None);
    // Back: closing with the threshold of a closing move.
    stm.now += 300_000;
    to_809(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 51, 34));
    direct_done(&mut stm, &mut f, MoveDir::Close, 51, 53);
    assert_eq!(f.view(0).counts, 807);
    // No event of these moves in the ring of 32.
    assert_eq!(stm.codes(), [EventCode::FlowPositioned]);
}

#[test]
fn a_direct_move_of_the_same_pulses_as_the_last_one_gets_a_record_of_its_own() {
    let (mut stm, mut f) = held_open(&open_curve());
    to_857(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 48, 30));
    direct_done(&mut stm, &mut f, MoveDir::Open, 48, 48);
    stm.now += 300_000;
    to_809(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 49, 34));
    direct_done(&mut stm, &mut f, MoveDir::Close, 49, 49);
    assert_eq!(f.view(0).counts, 808);
}

#[test]
fn the_held_position_of_a_direct_move_stays_inside_the_range() {
    let (mut stm, mut f) = held_open(&open_curve());
    to_857(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 48, 30));
    direct_done(&mut stm, &mut f, MoveDir::Open, 48, 9900);
    assert_eq!(f.view(0).counts, FLOW_COUNTS_MAX);
    let (mut stm, mut f) = held_open(&open_curve());
    stm.flow(&open_curve(), 0, 540); // 750 pulses
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 59, 34));
    direct_done(&mut stm, &mut f, MoveDir::Close, 59, 900);
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
    to_857(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    assert!(!f.view(0).known);
    // The hold alone will be noticed by the next tick: the position is still known.
    let (mut stm, mut f) = held_open(&open_curve());
    stm.flags(0, STM_FLAG_SVC_HOLD);
    to_857(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 48, 30));
    // Another record before the tick: not where the last move left it.
    let (mut stm, mut f) = held_open(&open_curve());
    stm.moved(0, MoveDir::Open, 50, 50, StopReason::Target);
    to_857(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    assert!(!f.view(0).known);
}

#[test]
fn a_direct_move_waits_with_a_busy_stm_and_starts_again_when_the_valve_left_meanwhile() {
    let (mut stm, mut f) = held_open(&open_curve());
    to_857(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 48, 30));
    stm.result(&mut f, 0, FlowOutcome::Rejected(2));
    stm.now += BUSY_RETRY_MS;
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 48, 30));
    stm.result(&mut f, 0, FlowOutcome::Rejected(2));
    foreign_move(&mut stm, 0, 0);
    stm.now += BUSY_RETRY_MS;
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    // The moves of the reference follow, the event with them.
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 10000, 4000, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 3844, 34));
    direct_done(&mut stm, &mut f, MoveDir::Close, 3844, 3844);
    assert_eq!(f.view(0).counts, 857);
    assert_eq!(
        stm.codes(),
        [EventCode::FlowPositioned, EventCode::FlowPositioned]
    );
    // A link lost before the move starts from the end stop too, with nothing of the held
    // position left in the sequence.
    let (mut stm, mut f) = held_open(&open_curve());
    to_857(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 48, 30));
    stm.result(&mut f, 0, FlowOutcome::Rejected(2));
    assert_eq!(f.next(&stm.m, inputs(false, false), stm.now), None);
    stm.now += BUSY_RETRY_MS;
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 10000, 4000, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 3844, 34));
}

#[test]
fn a_direct_move_that_meets_an_end_stop_goes_on_from_the_reference_end_stop() {
    for stop in [StopReason::EndStop, StopReason::EarlyEndStop] {
        let (mut stm, mut f) = held_open(&open_curve());
        to_857(&mut stm);
        assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 48, 30));
        stm.result(&mut f, 0, FlowOutcome::Ok);
        stm.tick(&mut f);
        stm.moved(0, MoveDir::Open, 48, 20, stop);
        stm.tick(&mut f);
        // Not a failure: the same sequence, the position forgotten.
        assert_eq!(f.view(0).state, FlowState::Positioning);
        assert!(!f.view(0).known);
        assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
        stm.result(&mut f, 0, FlowOutcome::Ok);
        stm.tick(&mut f);
        stm.moved(0, MoveDir::Open, 10000, 20, StopReason::EndStop);
        stm.tick(&mut f);
        assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 3844, 34));
        direct_done(&mut stm, &mut f, MoveDir::Close, 3844, 3844);
        assert_eq!(f.view(0).counts, 857);
        assert_eq!(f.view(0).ref_delta, None);
        // The positioning through the end stop reports itself.
        assert_eq!(
            stm.codes(),
            [EventCode::FlowPositioned, EventCode::FlowPositioned]
        );
        assert_eq!((stm.events[1].arg1, stm.events[1].arg2), (857, 18));
    }
}

#[test]
fn any_other_stop_of_a_direct_move_fails_at_the_final_step() {
    let (mut stm, mut f) = held_open(&open_curve());
    to_857(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 48, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 48, 12, StopReason::UnderCurrent);
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
    to_857(&mut stm);
    assert_eq!(stm.next(&mut f), set_target(0, 15));
    stm.ex[0].target = 15;
    stm.poll(0);
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    // ... and a target the valve stands at already is no reason to skip it.
    let mut c = open_curve();
    c.deadband_ml = 0;
    let (mut stm, mut f) = held_open(&c);
    assert!(f.position_counts(0, 857, Served::of(stm.m.valve(0)), stm.now));
    stm.moved_through(&mut f, c.stroke_counts - 857);
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
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1309, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 1309, 800, StopReason::EndStop);
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
    c.stroke_counts = 1100;
    let (mut stm, mut f) = held_open(&c);
    stm.target(0, 100);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Open, 10000, 291, StopReason::EndStop);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), keep(0, true));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    assert_eq!(f.view(0).counts, 1100);
    // The top of the curve at the end stop as well, however the target came.
    let (mut stm, mut f) = held_open(&c);
    stm.flow(&c, 0, 2700);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    // One pulse below it: one move, and 100 % is the end stop all the same.
    let mut c = open_curve();
    c.stroke_counts = 1101;
    let (mut stm, mut f) = held_open(&c);
    stm.flow(&c, 0, 2700);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 291, 30));
    let (mut stm, mut f) = held_open(&c);
    stm.target(0, 100);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
}

#[test]
fn a_counts_request_and_a_changed_curve_go_through_the_reference_end_stop() {
    let (mut stm, mut f) = held_open(&open_curve());
    assert!(f.position_counts(0, 857, Served::of(stm.m.valve(0)), stm.now));
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    let c = open_curve();
    let (mut stm, mut f) = held_open(&c);
    let mut changed = c;
    changed.offset = 5;
    f.set_configs(&configs(&changed, &[0]));
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
    // A curve that changes during a direct move: its keep is followed by a whole positioning.
    let (mut stm, mut f) = held_open(&c);
    to_857(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 48, 30));
    f.set_configs(&configs(&changed, &[0]));
    assert_eq!(f.view(0).state, FlowState::Positioning);
    direct_done(&mut stm, &mut f, MoveDir::Open, 48, 48);
    assert_eq!(f.view(0).state, FlowState::Unknown);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 10000, 30));
}

#[test]
fn a_target_at_the_held_pulses_is_served_without_a_move() {
    let mut c = open_curve();
    c.deadband_ml = 0;
    let (mut stm, mut f) = held_open(&c);
    assert!(f.position_counts(0, 857, Served::of(stm.m.valve(0)), stm.now));
    stm.moved_through(&mut f, c.stroke_counts - 857);
    let start = f.v[0].last_start_ms;
    stm.now += 300_000;
    stm.flow(&c, 0, 1350);
    assert_eq!(stm.next(&mut f), None);
    assert_eq!(stm.next(&mut f), None);
    assert_eq!(
        f.v[0].served,
        Some(Served {
            pct: 18,
            flow_ml: Some(1350)
        })
    );
    assert_eq!(f.v[0].last_start_ms, start);
    assert_eq!(f.view(0).state, FlowState::Held);
    // A target in % there as well.
    stm.target(0, 18); // 846 pulses
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 11, 34));
    direct_done(&mut stm, &mut f, MoveDir::Close, 11, 11);
    assert_eq!(f.view(0).counts, 846);
    stm.now += 300_000;
    stm.flow(&c, 0, 1270); // 846 pulses
    assert_eq!(stm.next(&mut f), None);
    stm.target(0, 18);
    assert_eq!(stm.next(&mut f), None);
    assert_eq!(f.v[0].served, served(18));
}

#[test]
fn a_target_at_the_held_pulses_is_the_target_shown() {
    let (mut stm, mut f) = held_open(&open_curve());
    to_857(&mut stm);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 48, 30));
    // 11 pulses short of the target
    direct_done(&mut stm, &mut f, MoveDir::Open, 48, 37);
    assert_eq!((f.view(0).counts, f.view(0).target_counts), (846, 857));
    stm.now += 300_000;
    stm.target(0, 18); // 846 pulses
    assert_eq!(stm.next(&mut f), None);
    assert_eq!((f.view(0).counts, f.view(0).target_counts), (846, 846));
    assert_eq!(f.v[0].target_pct, 18);
}

#[test]
fn a_target_in_percent_moves_the_valve_above_the_top_of_the_curve_too() {
    let mut c = curve();
    c.min_interval_s = 0;
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.target(0, 60); // 1312 pulses, above the curve
    position(&mut stm, &mut f, 0, false, 4800, 1312);
    assert_eq!(f.view(0).flow_ml, 2700);
    stm.target(0, 70); // 1531 pulses, the same flow
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 219, 30));
    direct_done(&mut stm, &mut f, MoveDir::Open, 219, 219);
    assert_eq!(f.view(0).counts, 1531);
    assert_eq!(f.view(0).flow_ml, 2700);
}

#[test]
fn two_flows_in_the_same_whole_percent_are_two_positions() {
    let mut c = curve();
    c.min_interval_s = 0;
    c.deadband_ml = 0;
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 1000);
    position(&mut stm, &mut f, 0, false, 4800, 809);
    stm.flow(&c, 0, 1015); // 811 pulses
    assert_eq!(stm.m.valve(0).desired, 37);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 2, 30));
    direct_done(&mut stm, &mut f, MoveDir::Open, 2, 2);
    assert_eq!(f.view(0).counts, 811);
}

#[test]
fn a_target_in_percent_ends_the_flow_target_of_the_same_percent() {
    let mut c = curve();
    c.min_interval_s = 0;
    c.deadband_ml = 0;
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 1015); // 37 %, 811 pulses
    position(&mut stm, &mut f, 0, false, 4800, 811);
    stm.target(0, 37); // 809 pulses
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
    // The brake leaves it 20 pulses above the 809 asked for: 1.15 l/min on the curve.
    position_until_open(&mut stm, &mut f, 0, 4800);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Open, 809, 30));
    direct_done(&mut stm, &mut f, MoveDir::Open, 809, 829);
    assert_eq!(f.view(0).flow_ml, 1145);
    stm.flow(&c, 0, 1200);
    assert_eq!(stm.next(&mut f), None);
    stm.flow(&c, 0, 999); // 0.146 l/min away, 809 pulses
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 20, 34));
}

#[test]
fn no_flow_closes_at_once_past_the_dead_band_and_the_interval() {
    let mut c = curve();
    c.deadband_ml = 2000;
    let mut f = layer(&c, &[0]);
    let mut stm = Stm::new(0x001);
    stm.flow(&c, 0, 1000);
    position(&mut stm, &mut f, 0, false, 4800, 809);
    stm.flow(&c, 0, 0);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1309, 34));
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
        mv(0, MoveDir::Open, 809, 30),
        keep(0, true),
    ]
}

/// ... and the records of the four moves.
const WARM_RECORDS: [(MoveDir, u16, u16, StopReason); 4] = [
    (MoveDir::Close, 4800, 97, StopReason::EndStop),
    (MoveDir::Open, 300, 300, StopReason::Target),
    (MoveDir::Close, 800, 350, StopReason::EndStop),
    (MoveDir::Open, 809, 809, StopReason::Target),
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
        stm.moved(0, MoveDir::Open, 809, 809, StopReason::Target);
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
    position(&mut stm, &mut f, 0, false, 4800, 809);
    stm.now += 2;
    assert_eq!(f.next(&stm.m, inputs(false, false), stm.now), None);
    assert!(f.view(0).known);
    stm.now += 300_000;
    stm.target(0, 100);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1309, 34));
    // The retry of a reference close that missed the end stop is the retry, hold or not.
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 1309, 1309, StopReason::Target);
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
    // From the held 809: the close of 1309 misses the end stop, the retry of 800 meets it.
    stm.now += 300_000;
    stm.target(0, 100);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 1309, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 1309, 1309, StopReason::Target);
    stm.tick(&mut f);
    assert_eq!(stm.next(&mut f), mv(0, MoveDir::Close, 800, 34));
    stm.result(&mut f, 0, FlowOutcome::Ok);
    stm.tick(&mut f);
    stm.moved(0, MoveDir::Close, 800, 120, StopReason::EndStop);
    stm.tick(&mut f);
    assert!(f.view(0).known); // the held value until the position is lost
                              // The hold is gone before the warm-up: the stroke, neither 800 nor 809 + 500.
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
    // 1.25 l/min: 844 pulses, 38.6 % of the stroke, the nearest pulse gives 1.254.
    assert_eq!(
        t(1250),
        FlowTarget {
            ml: 1250,
            pct: 39,
            flow_ml: 1254,
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
    // Any flow above nothing has a position at the start of the curve.
    assert_eq!(t(1).pct, 27);
    // qmax itself, at the top of the curve.
    let top = FlowTarget {
        ml: 2700,
        pct: 50,
        flow_ml: 2700,
        clipped: false,
    };
    assert_eq!(t(2700), top);
    // Above qmax it is qmax, and clipped.
    let clipped = FlowTarget {
        clipped: true,
        ..top
    };
    assert_eq!(t(2701), clipped);
    assert_eq!(t(u32::MAX), clipped);
    // A curve that ends below qmax: the last point answers, one ml/min above it is clipped.
    let mut low = curve();
    low.count = 4;
    let t = |ml| flow_target(&low, ml);
    assert_eq!(
        t(2500),
        FlowTarget {
            ml: 2500,
            pct: 46,
            flow_ml: 2500,
            clipped: false
        }
    );
    assert_eq!(
        t(2501),
        FlowTarget {
            ml: 2501,
            pct: 46,
            flow_ml: 2500,
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

#[test]
fn flow_target_json_one_member_from_0_above_qmax_it_is_clipped() {
    let c = curve();
    let ok = |s: &str| parse_flow_target_json(&c, s.as_bytes());
    assert_eq!(ok(r#"{"lpm":1.25}"#).unwrap().ml, 1250);
    assert_eq!(ok(" {\t\"lpm\" :\n2.7 } \r\n").unwrap().ml, 2700);
    assert_eq!(ok(r#"{"lpm":0}"#).unwrap().ml, 0);
    let above = ok(r#"{"lpm":2.701}"#).unwrap();
    assert_eq!((above.ml, above.clipped), (2700, true));
    assert!(!ok(r#"{"lpm":2.7}"#).unwrap().clipped);
    assert_eq!(ok(r#"{"lpm":70.5}"#).unwrap().ml, 2700); // past u16 in ml/min
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
