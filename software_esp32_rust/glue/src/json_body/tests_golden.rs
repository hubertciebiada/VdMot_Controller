//! Differential proof against ArduinoJson 6.21.6 (GLUE-DESIGN-ESP.md 4.4): every body of
//! `tests/json_body/corpus.txt` goes through json_body and its outcome, formatted as
//! `tests/json_body/reference.cpp` prints it, must equal the line of `tests/json_body/golden.txt`,
//! the output of that reference program (format: the head comment of reference.cpp).
//!
//! Live run of the C++ reference (g++ with the 32-bit multilib of the tools/rust image), which
//! compares its output with golden.txt and with json_body:
//!
//! ```text
//! bash tools/rust/docker.sh run "cd software_esp32_rust && CARGO_TARGET_DIR=/target/software_esp32_rust cargo test -p vdm-esp-glue json_body::tests_golden -- --ignored"
//! ```
//!
//! With `VDM_JSON_BODY_BLESS=1` in that command, golden.txt is rewritten from the reference's
//! output first (after a change of the corpus).
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use super::*;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

const CORPUS: &str = include_str!("../../tests/json_body/corpus.txt");
const GOLDEN: &str = include_str!("../../tests/json_body/golden.txt");

/// The keys the glue looks up, the empty key and two of the corpus (`kProbes` of the reference).
const PROBES: [&[u8]; 27] = [
    b"target",
    b"dir",
    b"counts",
    b"maxmA",
    b"slot1",
    b"slot2",
    b"motor",
    b"learnMovements",
    b"breakaway",
    b"lowC",
    b"highC",
    b"startOnPower",
    b"noOfMinCount",
    b"maxCalReps",
    b"enable",
    b"stepPct",
    b"confirm",
    b"image",
    b"mode",
    b"force",
    b"board",
    b"action",
    b"valve",
    b"value",
    b"",
    b"a",
    b"k1",
];

/// The lines of a text file without CR (the checkout may have CRLF).
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(|l| l.strip_suffix('\r').unwrap_or(l))
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// One unit at `i`: a literal byte or %HH.
fn unit(b: &[u8], i: &mut usize, out: &mut Vec<u8>) -> Option<()> {
    if b[*i] != b'%' {
        out.push(b[*i]);
        *i += 1;
        return Some(());
    }
    let value = hex(*b.get(*i + 1)?)? * 16 + hex(*b.get(*i + 2)?)?;
    out.push(value);
    *i += 3;
    Some(())
}

/// A body of the corpus (`decodeBody` of the reference).
fn decode_body(text: &str) -> Option<Vec<u8>> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !b[i..].starts_with(b"%{") {
            unit(b, &mut i, &mut out)?;
            continue;
        }
        let close = i + b[i..].iter().position(|&c| c == b'}')?;
        let digits = &b[i + 2..close];
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let count: usize = std::str::from_utf8(digits).ok()?.parse().ok()?;
        i = close + 1;
        let mut group = Vec::new();
        if *b.get(i)? == b'(' {
            let end = i + b[i..].iter().position(|&c| c == b')')?;
            let mut k = i + 1;
            while k < end {
                unit(b, &mut k, &mut group)?;
                if k > end {
                    return None;
                }
            }
            i = end + 1;
        } else {
            unit(b, &mut i, &mut group)?;
        }
        for _ in 0..count {
            out.extend_from_slice(&group);
        }
    }
    Some(out)
}

/// The bodies of the corpus with their names.
fn corpus() -> Vec<(&'static str, Vec<u8>)> {
    lines(CORPUS)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (name, body) = l.split_once(' ').unwrap_or((l, ""));
            let body = decode_body(body).unwrap_or_else(|| panic!("malformed corpus line {name}"));
            (name, body)
        })
        .collect()
}

fn put_escaped(out: &mut String, bytes: &[u8]) {
    for &c in bytes {
        if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'-' {
            out.push(char::from(c));
        } else {
            let _ = write!(out, "\\x{c:02x}");
        }
    }
}

fn put_bits(out: &mut String, d: f64) {
    let _ = write!(out, "{:016x}", d.to_bits());
}

fn bit(out: &mut String, b: bool) {
    out.push(if b { '1' } else { '0' });
}

/// Index of the member whose value `found` is.
fn index_of(o: JsonObjectConst<'_>, found: JsonVariantConst<'_>) -> Option<usize> {
    let found = found.data?;
    o.iter()
        .position(|(_, v)| v.data.is_some_and(|d| core::ptr::eq(d, found)))
}

fn put_index(out: &mut String, index: Option<usize>) {
    match index {
        Some(i) => {
            let _ = write!(out, "{i}");
        }
        None => out.push('-'),
    }
}

fn put_queries(out: &mut String, v: JsonVariantConst<'_>) {
    out.push_str("<n");
    bit(out, v.is_null());
    out.push_str(" i");
    bit(out, v.is_i64());
    let _ = write!(out, ":{}", v.as_i64());
    out.push_str(" d");
    bit(out, v.is_f64());
    out.push(':');
    put_bits(out, v.as_f64());
    out.push_str(" b");
    bit(out, v.is_bool());
    out.push(':');
    bit(out, v.as_bool());
    out.push_str(" s");
    bit(out, v.is_str());
    out.push(':');
    match v.as_str() {
        Some(s) => {
            out.push('"');
            put_escaped(out, s);
            out.push('"');
        }
        None => out.push('-'),
    }
    out.push_str(" o");
    bit(out, v.is_object());
    out.push_str(" a");
    bit(out, v.is_array());
    let _ = write!(out, " z{}>", v.size());
}

fn put_value(out: &mut String, v: JsonVariantConst<'_>) {
    match v.value() {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if b { "true" } else { "false" }),
        Value::Number(Number::Unsigned(u)) => {
            let _ = write!(out, "u{u}");
        }
        Value::Number(Number::Signed(s)) => {
            let _ = write!(out, "i{s}");
        }
        Value::Number(Number::Float(f)) => {
            out.push('f');
            put_bits(out, f);
        }
        Value::Str(s) => {
            let _ = write!(out, "s{}\"", s.len());
            put_escaped(out, s);
            out.push('"');
        }
        Value::Object(_) => {
            let o = v.as_object().expect("an object");
            out.push('{');
            for (k, (key, member)) in o.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                out.push('"');
                put_escaped(out, key);
                out.push_str("\"@");
                put_index(out, index_of(o, o.get(key)));
                out.push('=');
                put_value(out, member);
            }
            out.push('}');
        }
        Value::Array(_) => {
            out.push('[');
            for (k, element) in v.as_array().expect("an array").iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                put_value(out, element);
            }
            out.push(']');
        }
    }
    put_queries(out, v);
}

/// The outcome of one body (`outcome()` of the reference): a heap document, the body in a
/// buffer of its own.
fn outcome(body: &[u8]) -> String {
    let mut input = body.to_vec();
    let mut doc = Box::new(JsonDocument::EMPTY);
    let parsed = doc.deserialize(&mut input).map(|root| {
        let mut out = String::new();
        put_value(&mut out, root);
        if let Some(o) = root.as_object() {
            out.push_str(" P=");
            for (k, key) in PROBES.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                put_index(&mut out, index_of(o, o.get(key)));
            }
        }
        out
    });
    match parsed {
        Ok(value) => format!("Ok mem={} {value}", doc.memory_usage()),
        Err(e) => format!("{} mem={}", e.c_str(), doc.memory_usage()),
    }
}

/// "name outcome" of every body of the corpus, in corpus order.
fn rust_outcomes() -> Vec<String> {
    corpus()
        .iter()
        .map(|(name, body)| format!("{name} {}", outcome(body)))
        .collect()
}

/// Panics with the first differing lines of `expected` and `actual`.
fn assert_same(what: &str, expected: &[&str], actual: &[String]) {
    let diffs: Vec<String> = expected
        .iter()
        .zip(actual)
        .filter(|(e, a)| *e != a)
        .take(5)
        .map(|(e, a)| format!("\n  expected {e}\n  actual   {a}"))
        .collect();
    assert!(diffs.is_empty(), "{what}: lines differ:{}", diffs.concat());
    assert_eq!(expected.len(), actual.len(), "{what}: line count");
}

#[test]
fn corpus_names_are_unique_and_bodies_decode() {
    let corpus = corpus();
    let mut names: Vec<&str> = corpus.iter().map(|(name, _)| *name).collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(names.len(), before, "duplicate corpus names");
    assert!(before >= 400, "corpus of {before} bodies");
    assert_eq!(
        decode_body("a%{3}(b%2c)%{2}%41%{2}c"),
        Some(b"ab,b,b,AAcc".to_vec())
    );
    assert_eq!(decode_body("%4"), None);
    assert_eq!(decode_body("%{}x"), None);
    assert_eq!(decode_body("%{2}(x"), None);
}

#[test]
fn every_body_has_the_outcome_of_arduinojson() {
    let golden: Vec<&str> = lines(GOLDEN).collect();
    assert_same("json_body vs golden.txt", &golden, &rust_outcomes());
}

/// Builds and runs `tests/json_body/reference.cpp` (needs g++ with `-m32`; see the module
/// documentation for the command). `VDM_JSON_BODY_BLESS=1` rewrites golden.txt first.
#[test]
#[ignore = "needs g++ with the 32-bit multilib: run in the tools/rust image"]
fn live_reference_matches_golden() {
    let glue = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = glue.join("tests/json_body");
    let header_dir = glue.join("../../software_esp32_revamped/test/native/third_party");
    let exe = std::env::temp_dir().join(format!("json_body_reference_{}", std::process::id()));
    let status = Command::new("g++")
        .args(["-m32", "-msse2", "-mfpmath=sse", "-std=gnu++17", "-O2"])
        .args(["-Wall", "-Wextra", "-Werror", "-I"])
        .arg(&header_dir)
        .arg("-I")
        .arg(dir.join("shim"))
        .arg(dir.join("reference.cpp"))
        .arg("-o")
        .arg(&exe)
        .status()
        .expect("g++ runs");
    assert!(status.success(), "g++ failed");
    let run = Command::new(&exe)
        .arg(dir.join("corpus.txt"))
        .output()
        .expect("reference runs");
    let _ = std::fs::remove_file(&exe);
    assert!(
        run.status.success(),
        "reference: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let reference = String::from_utf8(run.stdout).expect("ASCII output");
    if std::env::var("VDM_JSON_BODY_BLESS").as_deref() == Ok("1") {
        std::fs::write(dir.join("golden.txt"), &reference).expect("golden.txt written");
    } else {
        let golden: Vec<String> = lines(GOLDEN).map(String::from).collect();
        let reference_lines: Vec<&str> = lines(&reference).collect();
        assert_same("reference vs golden.txt", &reference_lines, &golden);
    }
    let reference_lines: Vec<&str> = lines(&reference).collect();
    assert_same("reference vs json_body", &reference_lines, &rust_outcomes());
}
