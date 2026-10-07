// New cases (design §5.5, no C++ counterpart): the words of the fault record, its check word,
// the count over consecutive faults and the names the terminal prints.

use super::*;

fn sample() -> FaultRecord {
    FaultRecord {
        kind: KIND_HARD_FAULT,
        pc: 0x0800_1235,
        lr: 0xFFFF_FFF9,
        xpsr: 0x2100_0000,
        cfsr: 0x0001_0000,
        hfsr: 0x4000_0000,
        bfar: 0xE000_ED38,
        count: 3,
    }
}

#[test]
fn the_words_are_magic_fields_count_and_the_inverted_xor() {
    let w = sample().to_words();
    assert_eq!(w[0], 0x5644_4652);
    assert_eq!(
        &w[1..9],
        &[
            1,
            0x0800_1235,
            0xFFFF_FFF9,
            0x2100_0000,
            0x0001_0000,
            0x4000_0000,
            0xE000_ED38,
            3
        ]
    );
    let x = w[..9].iter().fold(0u32, |a, b| a ^ b);
    assert_eq!(w[9], !x);
    assert_eq!(check(&w), w[9]);
    assert_eq!(FaultRecord::from_words(&w), Some(sample()));
}

#[test]
fn a_wrong_magic_or_check_word_or_any_changed_word_is_no_record() {
    let w = sample().to_words();
    for i in 0..RECORD_WORDS {
        let mut bad = w;
        if let Some(word) = bad.get_mut(i) {
            *word ^= 0x10;
        }
        assert_eq!(FaultRecord::from_words(&bad), None, "word {i}");
    }
    assert_eq!(FaultRecord::from_words(&[0; RECORD_WORDS]), None);
    // random RAM after a power-on that happens to hold the magic but no check word
    let mut random = [0xA5A5_A5A5; RECORD_WORDS];
    random[0] = RECORD_MAGIC;
    assert_eq!(FaultRecord::from_words(&random), None);
}

#[test]
fn the_count_continues_a_valid_record_and_starts_at_1_otherwise() {
    let w = sample().to_words();
    assert_eq!(FaultRecord::next_count(&w), 4);
    assert_eq!(FaultRecord::next_count(&[0; RECORD_WORDS]), 1);
    let mut wrap = sample();
    wrap.count = u32::MAX;
    assert_eq!(FaultRecord::next_count(&wrap.to_words()), 0);
}

#[test]
fn every_kind_has_its_name() {
    let names: [(u32, &[u8]); 8] = [
        (KIND_HARD_FAULT, b"HardFault"),
        (KIND_NMI, b"NMI"),
        (KIND_MEM_MANAGE, b"MemManage"),
        (KIND_BUS_FAULT, b"BusFault"),
        (KIND_USAGE_FAULT, b"UsageFault"),
        (KIND_UNEXPECTED, b"unexpected interrupt"),
        (KIND_PANIC, b"panic"),
        (0, b"unknown"),
    ];
    for (kind, name) in names {
        let r = FaultRecord { kind, ..sample() };
        assert_eq!(r.kind_name(), name, "kind {kind}");
    }
    assert_eq!(
        [
            KIND_HARD_FAULT,
            KIND_NMI,
            KIND_MEM_MANAGE,
            KIND_BUS_FAULT,
            KIND_USAGE_FAULT,
            KIND_UNEXPECTED,
            KIND_PANIC
        ],
        [1, 2, 3, 4, 5, 6, 7]
    );
}
