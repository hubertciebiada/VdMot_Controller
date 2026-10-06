//! Tests of `http_parse`: the URL and multipart cases that replace the retired library-quirk cases
//! of `test_web_server__eq.cpp` (docs/rust/GLUE-DESIGN-ESP.md 5.2), the bodies of the real
//! clients, the library's quirks that the port keeps and the changes it makes.

use super::*;

// ---------------------------------------------------------------- helpers

/// An event with owned bytes; consecutive file data is merged, since its cuts follow the chunks.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Ev {
    /// name, full name length, value, full value length
    Field(Vec<u8>, usize, Vec<u8>, usize),
    /// name, full name length, filename, full filename length
    FileStart(Vec<u8>, usize, Vec<u8>, usize),
    Data(Vec<u8>),
    FileEnd,
    Done,
}

fn record(events: &mut Vec<Ev>, event: Event<'_>) {
    let ev = match event {
        Event::Field { name, value } => {
            Ev::Field(name.kept.to_vec(), name.len, value.kept.to_vec(), value.len)
        }
        Event::FileStart { name, filename } => Ev::FileStart(
            name.kept.to_vec(),
            name.len,
            filename.kept.to_vec(),
            filename.len,
        ),
        Event::FileData(bytes) => {
            assert!(!bytes.is_empty(), "empty file data");
            if let Some(Ev::Data(last)) = events.last_mut() {
                last.extend_from_slice(bytes);
                return;
            }
            Ev::Data(bytes.to_vec())
        }
        Event::FileEnd => Ev::FileEnd,
        Event::Done => Ev::Done,
    };
    events.push(ev);
}

/// A parser started for `content_type`, boxed as the web working set holds it.
fn started(content_type: &[u8]) -> Box<Multipart> {
    let mut parser = Box::new(Multipart::EMPTY);
    parser.start(content_type);
    parser
}

/// The events of `chunks` fed one after another into a parser for `content_type`.
fn parse(content_type: &[u8], chunks: &[&[u8]]) -> (Vec<Ev>, Box<Multipart>) {
    let mut parser = started(content_type);
    let mut events = Vec::new();
    for chunk in chunks {
        parser.feed(chunk, |e| record(&mut events, e));
    }
    (events, parser)
}

/// The events of `body` fed at once.
fn parse_all(content_type: &[u8], body: &[u8]) -> Vec<Ev> {
    parse(content_type, &[body]).0
}

fn field(name: &[u8], value: &[u8]) -> Ev {
    Ev::Field(name.to_vec(), name.len(), value.to_vec(), value.len())
}

fn file(name: &[u8], filename: &[u8]) -> Ev {
    Ev::FileStart(name.to_vec(), name.len(), filename.to_vec(), filename.len())
}

fn data(bytes: &[u8]) -> Ev {
    Ev::Data(bytes.to_vec())
}

/// The decoded value of query parameter `name`, read into a buffer as long as the query.
fn param(query: &[u8], name: &[u8]) -> Option<Vec<u8>> {
    let mut out = vec![0u8; query.len()];
    let len = query_param(query, name, &mut out)?;
    assert!(len <= out.len());
    out.truncate(len);
    assert!(has_query_param(query, name));
    Some(out)
}

fn decode(text: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; text.len()];
    let len = url_decode(text, &mut out);
    out.truncate(len);
    out
}

const CT_B: &[u8] = b"multipart/form-data; boundary=b";

/// Headers of a file part named `file` with `filename`.
const FILE_HEADERS: &[u8] =
    b"Content-Disposition: form-data; name=\"file\"; filename=\"x.bin\"\r\n\
Content-Type: application/octet-stream\r\n";

/// A body with boundary "b": one file part (FILE_HEADERS) with `data`, then `tail` (what follows
/// the data, e.g. "\r\n--b--\r\n").
fn file_body(data: &[u8], tail: &[u8]) -> Vec<u8> {
    [&b"--b\r\n"[..], FILE_HEADERS, b"\r\n", data, tail].concat()
}

// ---------------------------------------------------------------- target, URL decoding

#[test]
fn target_splits_at_the_first_question_mark_after_the_first_byte() {
    let cases: [(&[u8], &[u8], &[u8]); 8] = [
        (
            b"/api/events?since=5&limit=2",
            b"/api/events",
            b"since=5&limit=2",
        ),
        (b"/api/status", b"/api/status", b""),
        (b"/a?b?c", b"/a", b"b?c"),
        (b"/?", b"/", b""),
        (b"/?=", b"/", b"="),
        // the library split only at an index > 0
        (b"?a=1", b"?a=1", b""),
        (b"??a", b"??a", b""),
        (b"", b"", b""),
    ];
    for (target, path, query) in cases {
        assert_eq!(split_target(target), (path, query), "{target:?}");
    }
}

#[test]
fn url_decode_reads_a_percent_and_two_hex_digits_as_one_byte() {
    let digits = b"0123456789abcdefABCDEF";
    let value = |d: u8| match d {
        b'0'..=b'9' => d - b'0',
        b'a'..=b'f' => d - b'a' + 10,
        _ => d - b'A' + 10,
    };
    for &hi in digits {
        for &lo in digits {
            assert_eq!(
                decode(&[b'%', hi, lo]),
                [value(hi) * 16 + value(lo)],
                "%{}{}",
                char::from(hi),
                char::from(lo)
            );
        }
    }
    assert_eq!(decode(b"/api/files%2Fstm%2Fx.bin"), b"/api/files/stm/x.bin");
    assert_eq!(decode(b"%41%62c"), b"Abc");
}

#[test]
fn url_decode_reads_other_bytes_after_a_percent_like_strtol() {
    // strtol("0x" + c1 + c2, NULL, 16): hex digits up to the first other byte, the low byte
    let cases: [(&[u8], &[u8]); 14] = [
        (b"%4G", &[0x04]),
        (b"%f ", &[0x0F]),
        (b"%a\xFF", &[0x0A]),
        (b"%G4", &[0x00]),
        (b"% 1", &[0x00]), // no blank skipped after the "0x"
        (b"%-1", &[0x00]), // no sign after the "0x"
        (b"%+1", &[0x00]),
        (b"%x1", &[0x00]), // "0xx1"
        (b"%0x", &[0x00]), // "0x0x"
        (b"%zz", &[0x00]),
        (b"%\xC3\xA4", &[0x00]),
        (b"%00", &[0x00]),
        (b"%%41", &[0x00, b'1']), // "0x%4", then "1"
        (b"%4+", &[0x04]),        // the '+' belongs to the escape: no space
    ];
    for (text, decoded) in cases {
        assert_eq!(decode(text), decoded, "{text:?}");
    }
}

#[test]
fn url_decode_reads_plus_as_space_and_keeps_a_percent_without_two_bytes_after_it() {
    let cases: [(&[u8], &[u8]); 10] = [
        (b"/api/a+b", b"/api/a b"),
        (b"+", b" "),
        (b"+%2B+", b" + "),
        (b"%", b"%"),
        (b"%4", b"%4"),
        (b"a%", b"a%"),
        (b"%41%", b"A%"),
        (b"%41%4", b"A%4"),
        (b"%+", b"% "),
        (b"", b""),
    ];
    for (text, decoded) in cases {
        assert_eq!(decode(text), decoded, "{text:?}");
    }
}

#[test]
fn url_decode_writes_what_fits_and_returns_the_full_length() {
    let mut out = [b'.'; 2];
    assert_eq!(url_decode(b"%41bc", &mut out), 3);
    assert_eq!(&out, b"Ab");
    assert_eq!(url_decode(b"a+b", &mut []), 3);
    let mut out = [b'.'; 6];
    assert_eq!(url_decode(b"%41b", &mut out), 2);
    assert_eq!(&out, b"Ab....");
    assert_eq!(url_decode(b"abc", &mut out), 3);
    assert_eq!(&out, b"abc...");
}

// ---------------------------------------------------------------- query

#[test]
fn query_parameter_is_the_first_one_of_its_name() {
    let q = b"since=5&limit=20&since=7";
    assert_eq!(param(q, b"since").as_deref(), Some(&b"5"[..]));
    assert_eq!(param(q, b"limit").as_deref(), Some(&b"20"[..]));
    assert_eq!(param(q, b"valve"), None);
    assert!(!has_query_param(q, b"valve"));
    assert!(!has_query_param(b"", b"since"));
    // names compare byte for byte: no case folding, no prefixes
    assert_eq!(param(b"MD5=a", b"md5"), None);
    assert_eq!(param(b"md5x=a&md=b&md5", b"md5").as_deref(), Some(&b""[..]));
    assert_eq!(param(b"md5=a", b"md5x"), None);
    assert_eq!(param(b"md5=a", b"md"), None);
    assert_eq!(param(b"md5=a", b"MD5"), None);
}

#[test]
fn query_name_runs_up_to_the_first_equals_and_the_value_after_it() {
    assert_eq!(param(b"a=b=c", b"a").as_deref(), Some(&b"b=c"[..]));
    assert_eq!(param(b"a=", b"a").as_deref(), Some(&b""[..]));
    assert_eq!(param(b"a", b"a").as_deref(), Some(&b""[..]));
    assert_eq!(param(b"x=1&a&b=2", b"a").as_deref(), Some(&b""[..]));
    assert_eq!(param(b"a&b=2", b"b").as_deref(), Some(&b"2"[..]));
    assert_eq!(
        param(b"x=1&dryRun=1", b"dryRun").as_deref(),
        Some(&b"1"[..])
    );
    // empty names are parameters; a '&' at the end does not start one
    assert_eq!(param(b"=1&x", b"").as_deref(), Some(&b"1"[..]));
    assert_eq!(param(b"a&&b=2", b"").as_deref(), Some(&b""[..]));
    assert_eq!(param(b"&", b"").as_deref(), Some(&b""[..]));
    assert!(!has_query_param(b"", b""));
    assert!(!has_query_param(b"a&", b""));
    assert!(!has_query_param(b"a=1&", b""));
}

#[test]
fn query_names_and_values_are_url_decoded() {
    assert_eq!(param(b"%6Dd5=%41+b", b"md5").as_deref(), Some(&b"A b"[..]));
    assert_eq!(param(b"a+b=1", b"a b").as_deref(), Some(&b"1"[..]));
    // an escaped '&' or '=' is no separator
    assert_eq!(param(b"a%26b=1&b=2", b"a&b").as_deref(), Some(&b"1"[..]));
    assert_eq!(param(b"a%3Db=1", b"a=b").as_deref(), Some(&b"1"[..]));
    assert_eq!(
        param(b"path=%2Fstm%2Fx.bin", b"path").as_deref(),
        Some(&b"/stm/x.bin"[..])
    );
    assert_eq!(param(b"x=1%", b"x").as_deref(), Some(&b"1%"[..]));
    // NUL bytes stay in the value (a C string would end there)
    assert_eq!(param(b"md5=%00x", b"md5").as_deref(), Some(&b"\0x"[..]));
}

#[test]
fn query_value_longer_than_the_buffer_gives_its_full_length() {
    let mut out = [b'.'; 4];
    assert_eq!(
        query_param(b"a=1&md5=0123456789", b"md5", &mut out),
        Some(10)
    );
    assert_eq!(&out, b"0123");
    assert_eq!(query_param(b"md5=%41%42", b"md5", &mut []), Some(2));
    assert_eq!(query_param(b"md5=1", b"x", &mut out), None);
}

// ---------------------------------------------------------------- Content-Type

#[test]
fn media_type_is_the_value_up_to_the_first_semicolon() {
    let cases: [(&[u8], &[u8]); 6] = [
        (b"multipart/form-data; boundary=x", b"multipart/form-data"),
        (b"application/json", b"application/json"),
        (b"application/json;charset=utf-8", b"application/json"),
        (b"text/plain ; a;b", b"text/plain "), // not trimmed
        (b";x", b""),
        (b"", b""),
    ];
    for (value, media) in cases {
        assert_eq!(media_type(value), media, "{value:?}");
    }
}

#[test]
fn multipart_is_a_case_sensitive_prefix_of_the_value() {
    for value in [
        &b"multipart/form-data; boundary=x"[..],
        b"multipart/",
        b"multipart/mixed",
    ] {
        assert!(is_multipart(value), "{value:?}");
    }
    for value in [
        &b"Multipart/form-data; boundary=x"[..],
        b"multipart",
        b" multipart/form-data",
        b"application/json",
        b"",
    ] {
        assert!(!is_multipart(value), "{value:?}");
    }
}

#[test]
fn boundary_is_the_text_after_the_first_equals_without_quotes() {
    let cases: [(&[u8], &[u8]); 7] = [
        (b"multipart/form-data; boundary=abc", b"abc"),
        (b"multipart/form-data; boundary=\"abc\"", b"abc"),
        (b"multipart/form-data; boundary=a\"b\"c\"", b"abc"),
        (b"multipart/form-data; boundary==x", b"=x"),
        // the first '=' of the value, whatever its parameter
        (
            b"multipart/form-data; charset=utf-8; boundary=x",
            b"utf-8; boundary=x",
        ),
        // no '=': the whole value
        (b"multipart/form-data", b"multipart/form-data"),
        (b"=x", b"x"),
    ];
    for (value, expected) in cases {
        assert_eq!(boundary(value).as_deref(), Some(expected), "{value:?}");
    }
}

#[test]
fn boundary_has_one_to_seventy_bytes() {
    let b70 = "7".repeat(70);
    let ok = format!("multipart/form-data; boundary={b70}");
    assert_eq!(boundary(ok.as_bytes()).as_deref(), Some(b70.as_bytes()));
    let quoted = format!("multipart/form-data; boundary=\"{b70}\"");
    assert_eq!(boundary(quoted.as_bytes()).as_deref(), Some(b70.as_bytes()));
    let long = format!("multipart/form-data; boundary={b70}7");
    assert_eq!(boundary(long.as_bytes()), None);
    assert_eq!(boundary(b"multipart/form-data; boundary="), None);
    assert_eq!(boundary(b"multipart/form-data; boundary=\"\""), None);
    assert_eq!(boundary(b""), None);
}

// ---------------------------------------------------------------- multipart: real clients

#[test]
fn dashboard_upload_is_one_file_with_the_md5_in_the_query() {
    // web/app.js upload(): XMLHttpRequest with FormData { firmware: file }, MD5 in the query
    let (path, query) = split_target(b"/api/ota/esp?md5=9e107d9d372bb6826bd81d3542a419d6");
    assert_eq!(path, b"/api/ota/esp");
    assert_eq!(
        param(query, b"md5").as_deref(),
        Some(&b"9e107d9d372bb6826bd81d3542a419d6"[..])
    );
    let ct = b"multipart/form-data; boundary=----WebKitFormBoundary7MA4YWxkTrZu0gW";
    let image: Vec<u8> = (0..3000u32).map(|i| (i * 7 + 0xE9) as u8).collect();
    let body = [
        &b"------WebKitFormBoundary7MA4YWxkTrZu0gW\r\n\
Content-Disposition: form-data; name=\"firmware\"; \
filename=\"VdMot-Revamped_2.1.7-revamped_ESP32-WT32-ETH01.bin\"\r\n\
Content-Type: application/octet-stream\r\n\r\n"[..],
        image.as_slice(),
        b"\r\n------WebKitFormBoundary7MA4YWxkTrZu0gW--\r\n",
    ]
    .concat();
    let expected = vec![
        file(
            b"firmware",
            b"VdMot-Revamped_2.1.7-revamped_ESP32-WT32-ETH01.bin",
        ),
        data(&image),
        Ev::FileEnd,
        Ev::Done,
    ];
    let segments: Vec<&[u8]> = body.chunks(1460).collect();
    let (events, parser) = parse(ct, &segments);
    assert_eq!(events, expected);
    assert!(parser.is_done());
    assert!(!parser.is_error());
    assert_eq!(parse_all(ct, &body), expected);

    // the STM image upload: FormData { image: file } without MD5
    let body = b"------WebKitFormBoundaryQ\r\n\
Content-Disposition: form-data; name=\"image\"; filename=\"stm217.bin\"\r\n\
Content-Type: application/octet-stream\r\n\r\n\
\x01\x02\r\n------WebKitFormBoundaryQ--\r\n";
    assert_eq!(
        parse_all(
            b"multipart/form-data; boundary=----WebKitFormBoundaryQ",
            body
        ),
        [
            file(b"image", b"stm217.bin"),
            data(b"\x01\x02"),
            Ev::FileEnd,
            Ev::Done
        ]
    );
}

#[test]
fn curl_upload_with_an_md5_field_before_the_file() {
    // curl -F "md5=<hex>" -F "file=@fw.bin" http://vdmot/api/ota/esp
    let ct = b"multipart/form-data; boundary=------------------------d74496d66958873e";
    let body = b"--------------------------d74496d66958873e\r\n\
Content-Disposition: form-data; name=\"md5\"\r\n\r\n\
9e107d9d372bb6826bd81d3542a419d6\r\n\
--------------------------d74496d66958873e\r\n\
Content-Disposition: form-data; name=\"file\"; filename=\"fw.bin\"\r\n\
Content-Type: application/octet-stream\r\n\r\n\
\xE9\x05\r\n--\r\n\
--------------------------d74496d66958873e--\r\n";
    let expected = [
        field(b"md5", b"9e107d9d372bb6826bd81d3542a419d6"),
        file(b"file", b"fw.bin"),
        data(b"\xE9\x05\r\n--"),
        Ev::FileEnd,
        Ev::Done,
    ];
    assert_eq!(parse_all(ct, body), expected);
    let (events, parser) = parse(ct, &[&body[..100], &body[100..200], &body[200..]]);
    assert_eq!(events, expected);
    assert!(parser.is_done());
}

// ---------------------------------------------------------------- multipart: framing

/// File data with every partial delimiter of boundary "XyZ" that is data after all.
const TRICKY: &[u8] = b"\0\xFF\r\n\r\n-\r\n--\r\n--X\r\n--Xy\r\n--XyZQ\r\n--XyZ\rQ\
\r\n--XyZ\r\r\n--XyZZ\r\r\n--Xy-\r";

fn sample_body() -> Vec<u8> {
    [
        &b"--XyZ\r\nContent-Disposition: form-data; name=\"md5\"\r\n\r\na\r\nb\r\n"[..],
        b"--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"f.bin\"\r\n",
        b"Content-Type: application/octet-stream\r\n\r\n",
        TRICKY,
        b"\r\n--XyZ\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nafter\r\n--XyZ--\r\n",
        b"epilogue\r\n--XyZ\r\nContent-Disposition: form-data; name=\"late\"\r\n\r\nlate\r\n",
        b"--XyZ--\r\n",
    ]
    .concat()
}

fn sample_events() -> Vec<Ev> {
    vec![
        field(b"md5", b"a\r\nb"),
        file(b"file", b"f.bin"),
        data(TRICKY),
        Ev::FileEnd,
        field(b"note", b"after"),
        Ev::Done,
    ]
}

#[test]
fn every_split_point_of_a_body_gives_the_same_events() {
    let ct = b"multipart/form-data; boundary=XyZ";
    let body = sample_body();
    let expected = sample_events();
    assert_eq!(parse_all(ct, &body), expected);
    for at in 0..=body.len() {
        let (events, parser) = parse(ct, &[&body[..at], &body[at..]]);
        assert_eq!(events, expected, "split at {at}");
        assert!(parser.is_done());
    }
    let bytes: Vec<&[u8]> = body.chunks(1).collect();
    assert_eq!(parse(ct, &bytes).0, expected);
}

#[test]
fn file_data_is_passed_on_as_slices_of_the_input() {
    let mut parser = started(CT_B);
    parser.feed(&file_body(b"", b""), |_| {});
    // "\r\n" in the chunk turned out to be data: one slice; "\r\n--" at its end is held back
    let chunk = b"abc\r\ndef\r\n--";
    let mut slices = Vec::new();
    parser.feed(chunk, |e| {
        if let Event::FileData(d) = e {
            slices.push((d.as_ptr(), d.len()));
        }
    });
    assert_eq!(slices, [(chunk.as_ptr(), 8)]);
    // the held bytes are data after all: passed on from the parser, then the new byte
    let next = b"x\r\n--b--";
    let mut parts = Vec::new();
    parser.feed(next, |e| {
        if let Event::FileData(d) = e {
            parts.push((d.to_vec(), next.as_ptr_range().contains(&d.as_ptr())));
        }
    });
    assert_eq!(parts, [(b"\r\n--".to_vec(), false), (b"x".to_vec(), true)]);
    assert!(parser.is_done());
}

#[test]
fn a_body_that_ends_before_its_close_delimiter_is_not_done() {
    let start = [file(b"file", b"x.bin"), data(b"abc")];
    let cases: [(&[u8], usize); 7] = [
        (b"", 2),
        (b"\r", 2),
        (b"\r\n--", 2),
        (b"\r\n--b", 2),
        (b"\r\n--b\r", 2),
        (b"\r\n--b\r\n", 3),
        (
            b"\r\n--b\r\nContent-Disposition: form-data; name=\"x\"\r\n",
            3,
        ),
    ];
    for (tail, n) in cases {
        let body = file_body(b"abc", tail);
        let (events, parser) = parse(CT_B, &[&body]);
        let mut expected = start.to_vec();
        if n == 3 {
            expected.push(Ev::FileEnd);
        }
        assert_eq!(events, expected, "{tail:?}");
        assert!(!parser.is_done(), "{tail:?}");
        assert!(!parser.is_error(), "{tail:?}");
    }
    // the next part's field without its delimiter: no field
    let body = file_body(
        b"abc",
        b"\r\n--b\r\nContent-Disposition: form-data; name=\"x\"\r\n\r\n12",
    );
    let (events, parser) = parse(CT_B, &[&body]);
    assert_eq!(events, [start[0].clone(), start[1].clone(), Ev::FileEnd]);
    assert!(!parser.is_done());
}

#[test]
fn the_close_delimiter_ends_the_body_at_its_first_dash() {
    // the library looked at the first '-' only; everything after the close delimiter is ignored
    for tail in [
        &b"\r\n--b-"[..],
        b"\r\n--b--",
        b"\r\n--b--\r\n",
        b"\r\n--b-X\r\n--b\r\nContent-Disposition: form-data; name=\"y\"\r\n\r\n1\r\n--b--\r\n",
        b"\r\n--b--\r\nepilogue\r\n--b--\r\n",
    ] {
        let body = file_body(b"abc", tail);
        let expected = [file(b"file", b"x.bin"), data(b"abc"), Ev::FileEnd, Ev::Done];
        let (events, mut parser) = parse(CT_B, &[&body]);
        assert_eq!(events, expected, "{tail:?}");
        assert!(parser.is_done());
        let mut more = 0;
        parser.feed(b"\r\n--b\r\n\r\n1\r\n--b--", |_| more += 1);
        assert_eq!(more, 0);
        assert!(parser.is_done());
    }
}

#[test]
fn delimiter_bytes_without_crlf_or_dash_after_them_are_data() {
    // the library ended the part at the boundary match and then wrote these bytes back through
    // the freed item buffer of the file: a crash of the C++ firmware
    for (inner, as_data) in [
        (&b"1\r\n--bX2"[..], &b"1\r\n--bX2"[..]),
        (b"1\r\n--b\rX2", b"1\r\n--b\rX2"),
        (b"1\r\n--bb2", b"1\r\n--bb2"),
        (b"\r\n--b\r\r\n--bX", b"\r\n--b\r\r\n--bX"),
        (b"\r\r\n--c\r", b"\r\r\n--c\r"),
        (b"\r\n\r\n", b"\r\n\r\n"),
        (b"-\r\n-", b"-\r\n-"),
    ] {
        let body = file_body(inner, b"\r\n--b--\r\n");
        assert_eq!(
            parse_all(CT_B, &body),
            [
                file(b"file", b"x.bin"),
                data(as_data),
                Ev::FileEnd,
                Ev::Done
            ],
            "{inner:?}"
        );
    }
    // in a form field too: one field with all of it (the library added a second one)
    let body = b"--b\r\nContent-Disposition: form-data; name=\"md5\"\r\n\r\nx\r\n--bY\r\n--b-";
    assert_eq!(
        parse_all(CT_B, body),
        [field(b"md5", b"x\r\n--bY"), Ev::Done]
    );
}

#[test]
fn browser_boundary_that_starts_with_dashes() {
    let ct = b"multipart/form-data; boundary=----WebKitFormBoundaryABC";
    let inner = b"\r\n------\r\n----WebKitFormBoundaryABC\r\n\
\r\n------WebKitFormBoundaryAB\r\n------WebKitFormBoundaryABCD\r\n-------WebKitFormBoundaryABC";
    let body = [
        &b"------WebKitFormBoundaryABC\r\n"[..],
        FILE_HEADERS,
        b"\r\n",
        inner,
        b"\r\n------WebKitFormBoundaryABC--\r\n",
    ]
    .concat();
    let expected = [file(b"file", b"x.bin"), data(inner), Ev::FileEnd, Ev::Done];
    assert_eq!(parse_all(ct, &body), expected);
    let bytes: Vec<&[u8]> = body.chunks(1).collect();
    assert_eq!(parse(ct, &bytes).0, expected);
}

#[test]
fn two_files_and_fields_after_the_file() {
    let body = b"--b\r\n\
Content-Disposition: form-data; name=\"md5\"\r\n\r\nm1\r\n--b\r\n\
Content-Disposition: form-data; name=\"f1\"; filename=\"a.bin\"\r\n\
Content-Type: application/octet-stream\r\n\r\nAAA\r\n--b\r\n\
Content-Disposition: form-data; name=\"f2\"; filename=\"b.bin\"\r\n\
Content-Type: application/octet-stream\r\n\r\nBB\r\n--b\r\n\
Content-Disposition: form-data; name=\"MD5\"\r\n\r\nm2\r\n--b--\r\n";
    let expected = [
        field(b"md5", b"m1"),
        file(b"f1", b"a.bin"),
        data(b"AAA"),
        Ev::FileEnd,
        file(b"f2", b"b.bin"),
        data(b"BB"),
        Ev::FileEnd,
        field(b"MD5", b"m2"),
        Ev::Done,
    ];
    assert_eq!(parse_all(CT_B, body), expected);
    let bytes: Vec<&[u8]> = body.chunks(1).collect();
    assert_eq!(parse(CT_B, &bytes).0, expected);
}

#[test]
fn an_empty_file_part_gives_start_and_end_without_data() {
    // the library made no handleUpload call for it
    let body = file_body(b"", b"\r\n--b--\r\n");
    assert_eq!(
        parse_all(CT_B, &body),
        [file(b"file", b"x.bin"), Ev::FileEnd, Ev::Done]
    );
    let body = b"--b\r\nContent-Disposition: form-data; name=\"e\"\r\n\r\n\r\n--b--\r\n";
    assert_eq!(parse_all(CT_B, body), [field(b"e", b""), Ev::Done]);
}

#[test]
fn a_quoted_boundary_and_one_without_parameter_name() {
    let body = b"--a b\r\nContent-Disposition: form-data; name=\"k\"\r\n\r\nv\r\n--a b--\r\n";
    assert_eq!(
        parse_all(b"multipart/form-data; boundary=\"a b\"", body),
        [field(b"k", b"v"), Ev::Done]
    );
    // no '=' in the value: the whole value is the boundary
    let body = b"--multipart/form-data\r\n\r\nv\r\n--multipart/form-data--";
    assert_eq!(
        parse_all(b"multipart/form-data", body),
        [field(b"", b"v"), Ev::Done]
    );
}

#[test]
fn a_body_that_does_not_start_with_its_first_line_is_a_parse_error() {
    let valid = b"--b\r\nContent-Disposition: form-data; name=\"k\"\r\n\r\nv\r\n--b--\r\n";
    for start in [
        &b"preamble\r\n"[..],
        b"\r\n",
        b" ",
        b"-x",
        b"--c\r\n",
        b"--b\n",
        b"--b\rX",
        b"--bb\r\n",
        b"--b-",
    ] {
        let mut parser = started(CT_B);
        assert!(!parser.is_error());
        let mut events = Vec::new();
        parser.feed(start, |e| record(&mut events, e));
        assert!(parser.is_error(), "{start:?}");
        parser.feed(valid, |e| record(&mut events, e));
        assert!(events.is_empty(), "{start:?}");
        assert!(parser.is_error());
        assert!(!parser.is_done());
    }
    // the first line across chunks
    let (events, parser) = parse(CT_B, &[b"-", b"-b", b"\r", b"\n\r\nv\r\n--b-"]);
    assert_eq!(events, [field(b"", b"v"), Ev::Done]);
    assert!(parser.is_done());
}

#[test]
fn the_parser_holds_about_0_6_kb() {
    // delimiter 76, header line 256, name, filename and value 160, lengths and state: 560 B on
    // the 64-bit host
    assert!(std::mem::size_of::<Multipart>() <= 600);
}

#[test]
fn a_parser_is_in_its_error_state_until_started_and_a_start_forgets_the_last_body() {
    let mut parser = Box::new(Multipart::EMPTY);
    assert!(parser.is_error());
    let mut events = Vec::new();
    parser.feed(b"--b\r\n\r\nv\r\n--b-", |e| record(&mut events, e));
    assert!(events.is_empty());
    // a body that leaves name, filename, a file part and half a header line behind
    parser.start(CT_B);
    assert!(!parser.is_error());
    parser.feed(
        b"--b\r\nContent-Disposition: form-data; name=\"f\"; filename=\"x\"\r\n\r\nab\r\n--b\r\nX-Hal",
        |e| record(&mut events, e),
    );
    assert_eq!(events, [file(b"f", b"x"), data(b"ab"), Ev::FileEnd]);
    // the next body: its own boundary, a part without headers is a form field without name
    parser.start(b"multipart/form-data; boundary=c");
    events.clear();
    parser.feed(b"--c\r\n\r\nv\r\n--c-", |e| record(&mut events, e));
    assert_eq!(events, [field(b"", b"v"), Ev::Done]);
    assert!(parser.is_done());
    // a start with an invalid boundary
    parser.start(b"multipart/form-data; boundary=");
    assert!(parser.is_error());
    assert!(!parser.is_done());
    events.clear();
    parser.feed(b"--c\r\n\r\nv\r\n--c-", |e| record(&mut events, e));
    assert!(events.is_empty());
}

#[test]
fn an_invalid_boundary_leaves_the_parser_in_its_error_state() {
    let b71 = "7".repeat(71);
    let ct71 = format!("multipart/form-data; boundary={b71}");
    for ct in [
        &b"multipart/form-data; boundary="[..],
        b"multipart/form-data; boundary=\"\"",
        ct71.as_bytes(),
    ] {
        let parser = started(ct);
        assert!(parser.is_error(), "{ct:?}");
        assert!(!parser.is_done());
    }
    let body = format!("--{b71}\r\n\r\nv\r\n--{b71}--\r\n");
    let (events, parser) = parse(ct71.as_bytes(), &[body.as_bytes()]);
    assert!(events.is_empty());
    assert!(parser.is_error());
    // 70 bytes are fine
    let b70 = &b71[..70];
    let body = format!("--{b70}\r\n\r\nv\r\n--{b70}--\r\n");
    let ct70 = format!("multipart/form-data; boundary={b70}");
    assert_eq!(
        parse_all(ct70.as_bytes(), body.as_bytes()),
        [field(b"", b"v"), Ev::Done]
    );
}

// ---------------------------------------------------------------- multipart: part headers

/// The events of a body with boundary "b" whose first part has `headers` (lines without the
/// empty line) and the data "D".
fn one_part(headers: &[u8]) -> Vec<Ev> {
    parse_all(CT_B, &[&b"--b\r\n"[..], headers, b"\r\nD\r\n--b-"].concat())
}

#[test]
fn part_headers_match_their_names_ignoring_case() {
    assert_eq!(
        one_part(b"content-disposition: form-data; name=\"f\"; filename=\"x\"\r\n"),
        [file(b"f", b"x"), data(b"D"), Ev::FileEnd, Ev::Done]
    );
    assert_eq!(
        one_part(b"CONTENT-DISPOSITION: form-data; name=\"f\"\r\nCONTENT-TYPE: x\r\n"),
        [file(b"f", b""), data(b"D"), Ev::FileEnd, Ev::Done]
    );
    // the parameter names inside are case-sensitive
    assert_eq!(
        one_part(b"Content-Disposition: form-data; Name=\"f\"; FileName=\"x\"\r\n"),
        [field(b"", b"D"), Ev::Done]
    );
}

#[test]
fn a_content_type_line_makes_the_part_a_file() {
    let disposition: &[u8] = b"Content-Disposition: form-data; name=\"md5\"\r\n";
    for line in [
        &b"Content-Type: text/plain"[..],
        b"Content-Type:",
        b"content-type x",
        b"Content-Types: x",
    ] {
        let headers = [disposition, line, b"\r\n"].concat();
        assert_eq!(
            one_part(&headers),
            [file(b"md5", b""), data(b"D"), Ev::FileEnd, Ev::Done],
            "{line:?}"
        );
    }
    // 12 bytes are not enough, and the name must start the line
    for line in [
        &b"Content-Type"[..],
        b"X-Content-Type: x",
        b"Content-Typo: x",
    ] {
        let headers = [disposition, line, b"\r\n"].concat();
        assert_eq!(
            one_part(&headers),
            [field(b"md5", b"D"), Ev::Done],
            "{line:?}"
        );
    }
}

#[test]
fn name_and_filename_carry_over_to_parts_that_set_none() {
    let body = b"--b\r\n\
Content-Disposition: form-data; name=\"f1\"; filename=\"a.bin\"\r\n\r\nA\r\n--b\r\n\
Content-Type: application/octet-stream\r\n\r\nB\r\n--b\r\n\
\r\nC\r\n--b\r\n\
Content-Disposition: form-data; name=\"f2\"\r\n\r\nD\r\n--b--\r\n";
    assert_eq!(
        parse_all(CT_B, body),
        [
            file(b"f1", b"a.bin"),
            data(b"A"),
            Ev::FileEnd,
            file(b"f1", b"a.bin"),
            data(b"B"),
            Ev::FileEnd,
            field(b"f1", b"C"),
            field(b"f2", b"D"),
            Ev::Done,
        ]
    );
}

#[test]
fn content_disposition_is_read_with_the_library_offsets() {
    let cases: Vec<(&[u8], Ev)> = vec![
        (b"form-data; name=\"md5\"", field(b"md5", b"D")),
        (
            b"form-data; filename=\"x.bin\"; name=\"f\"",
            file(b"f", b"x.bin"),
        ),
        (
            b"form-data; name=\"f\"; size=10; filename=\"x\"",
            file(b"f", b"x"),
        ),
        // "; " assumed after the first ';'
        (b"form-data;name=\"x\"", field(b"", b"D")),
        // quotes assumed: one unquoted byte by luck of substring's swap, two give ""
        (b"form-data; name=x; filename=y", file(b"x", b"y")),
        (b"form-data; name=xy; filename=ab", file(b"", b"")),
        (b"form-data; name=", field(b"=", b"D")),
        // a ';' in a quoted filename ends it
        (
            b"form-data; name=\"f\"; filename=\"a;b.bin\"",
            file(b"f", b""),
        ),
        // no ';': the rest is the line without its first byte, no name in it
        (b"form-data", field(b"", b"D")),
        (b"form-data;", field(b"", b"D")),
        // a ';' that starts the rest ends the segments
        (b"form-data; ;;name=\"x\"", field(b"", b"D")),
        // the name runs to the first '=' of the rest, even past a ';'
        (b"form-data; foo; name=\"x\"", field(b"x", b"D")),
        (b"form-data; foo=\"1\"; name=\"x\"", field(b"x", b"D")),
        // NUL bytes are bytes (the library's strchr/strcmp stopped at them)
        (b"form-data; name=\"a\0b\"", field(b"a\0b", b"D")),
    ];
    for (params, first) in cases {
        let headers = [&b"Content-Disposition: "[..], params, b"\r\n"].concat();
        let events = one_part(&headers);
        assert_eq!(events.first(), Some(&first), "{params:?}");
    }
}

#[test]
fn header_lines_drop_cr_bytes_and_end_at_lf() {
    // bare LF line ends, a CR inside a line is dropped, a line of CRs is empty
    let body = b"--b\r\nContent-Disposition: form-data; name=\"a\rb\"\n\r\r\nv\r\n--b-";
    assert_eq!(parse_all(CT_B, body), [field(b"ab", b"v"), Ev::Done]);
    let body = b"--b\r\nX-A: 1\nContent-Disposition: form-data; name=\"c\"\n\nv\r\n--b-";
    assert_eq!(parse_all(CT_B, body), [field(b"c", b"v"), Ev::Done]);
}

// ---------------------------------------------------------------- multipart: bounds

#[test]
fn names_values_and_filenames_are_kept_to_their_bounds() {
    let n32 = "n".repeat(NAME_MAX);
    let n33 = "n".repeat(NAME_MAX + 1);
    let f64 = "f".repeat(FILENAME_MAX);
    let f65 = "f".repeat(FILENAME_MAX + 1);
    let v64 = vec![b'v'; VALUE_MAX];
    let v65 = vec![b'v'; VALUE_MAX + 1];
    let body = [
        format!("--b\r\nContent-Disposition: form-data; name=\"{n32}\"\r\n\r\n").as_bytes(),
        v64.as_slice(),
        format!("\r\n--b\r\nContent-Disposition: form-data; name=\"{n33}\"\r\n\r\n").as_bytes(),
        v65.as_slice(),
        format!(
            "\r\n--b\r\nContent-Disposition: form-data; name=\"x\"; filename=\"{f64}\"\r\n\r\n"
        )
        .as_bytes(),
        format!(
            "\r\n--b\r\nContent-Disposition: form-data; name=\"x\"; filename=\"{f65}\"\r\n\r\n"
        )
        .as_bytes(),
        b"\r\n--b--",
    ]
    .concat();
    let expected = vec![
        Ev::Field(n32.clone().into_bytes(), 32, v64.clone(), 64),
        Ev::Field(n32.clone().into_bytes(), 33, v64.clone(), 65),
        file(b"x", f64.as_bytes()),
        Ev::FileEnd,
        Ev::FileStart(b"x".to_vec(), 1, f64.clone().into_bytes(), 65),
        Ev::FileEnd,
        Ev::Done,
    ];
    assert_eq!(parse_all(CT_B, &body), expected);
    let bytes: Vec<&[u8]> = body.chunks(1).collect();
    assert_eq!(parse(CT_B, &bytes).0, expected);
    // a long value keeps its first bytes and counts all of them, also across chunks
    let long: Vec<u8> = (0..1000u32).map(|i| b'a' + (i % 26) as u8).collect();
    let body = [
        &b"--b\r\nContent-Disposition: form-data; name=\"md5\"\r\n\r\n"[..],
        long.as_slice(),
        b"\r\n--b-",
    ]
    .concat();
    let expected = [
        Ev::Field(b"md5".to_vec(), 3, long[..VALUE_MAX].to_vec(), 1000),
        Ev::Done,
    ];
    assert_eq!(parse_all(CT_B, &body), expected);
    let pieces: Vec<&[u8]> = body.chunks(7).collect();
    assert_eq!(parse(CT_B, &pieces).0, expected);
}

#[test]
fn bounded_text_is_a_name_only_when_whole_and_equal() {
    let whole = Bounded {
        kept: b"md5",
        len: 3,
    };
    assert!(whole.is(b"md5"));
    assert!(!whole.is(b"MD5"));
    assert!(!whole.is(b"md"));
    assert!(!whole.is(b"md5x"));
    let cut = Bounded {
        kept: b"md5",
        len: 4,
    };
    assert!(!cut.is(b"md5"));
    assert!(!cut.is(b"md5x"));
}

#[test]
fn a_header_line_is_read_as_its_first_256_bytes() {
    // "Content-Disposition: form-data; name=\"" (38) + pad + "\"; filename=\"x.bin\"" (19)
    let line = |pad: usize| {
        format!(
            "Content-Disposition: form-data; name=\"{}\"; filename=\"x.bin\"",
            "p".repeat(pad)
        )
    };
    assert_eq!(line(199).len(), HEADER_LINE_MAX);
    let events = one_part(format!("{}\r\n", line(199)).as_bytes());
    assert_eq!(
        events[0],
        Ev::FileStart(vec![b'p'; NAME_MAX], 199, b"x.bin".to_vec(), 5)
    );
    // one byte more: the closing quote is cut, so the filename loses its last byte
    let events = one_part(format!("{}\r\n", line(200)).as_bytes());
    assert_eq!(
        events[0],
        Ev::FileStart(vec![b'p'; NAME_MAX], 200, b"x.bi".to_vec(), 4)
    );
    // the filename beyond the cut: no file; the name runs to the cut
    let events = one_part(format!("{}\r\n", line(300)).as_bytes());
    assert_eq!(
        events[0],
        Ev::Field(vec![b'p'; NAME_MAX], 217, b"D".to_vec(), 1)
    );
    // a long line of another header does not disturb the next one
    let junk = format!("X-Junk: {}\r\n", "j".repeat(1000));
    let headers = [
        junk.as_bytes(),
        b"Content-Disposition: form-data; name=\"k\"\r\n",
    ]
    .concat();
    assert_eq!(one_part(&headers), [field(b"k", b"D"), Ev::Done]);
}

// ---------------------------------------------------------------- arbitrary bytes

/// xorshift32 with a fixed seed.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        self.next() as usize % n
    }
}

fn check_bounds(events: &[Ev], body_len: usize) {
    let mut out = 0;
    for event in events {
        match event {
            Ev::Field(name, name_len, value, value_len) => {
                assert!(name.len() <= NAME_MAX && name.len() <= *name_len);
                assert!(value.len() <= VALUE_MAX && value.len() <= *value_len);
                out += value_len;
            }
            Ev::FileStart(name, name_len, filename, filename_len) => {
                assert!(name.len() <= NAME_MAX && name.len() <= *name_len);
                assert!(filename.len() <= FILENAME_MAX && filename.len() <= *filename_len);
            }
            Ev::Data(bytes) => out += bytes.len(),
            Ev::FileEnd | Ev::Done => {}
        }
    }
    // every data byte is a body byte, passed on once
    assert!(out <= body_len, "{out} > {body_len}");
}

#[test]
fn arbitrary_bodies_never_panic_and_give_no_more_bytes_than_they_hold() {
    let mut rng = Rng(0x9E37_79B9);
    let boundaries: [&[u8]; 5] = [b"b", b"-", b"--b-", b"XyZ", b"\r"];
    let fragments: [&[u8]; 14] = [
        b"--",
        b"-",
        b"\r\n",
        b"\r",
        b"\n",
        b";",
        b"=",
        b"\"",
        b"\0",
        b"Content-Disposition: form-data; name=\"f\"; filename=\"x\"",
        b"content-disposition: form-data; name=\"m\"",
        b"Content-Type: a",
        b"\r\n\r\n",
        b"\r\n--",
    ];
    for _ in 0..3000 {
        let boundary = boundaries[rng.below(boundaries.len())];
        let mut body = Vec::new();
        if rng.below(8) != 0 {
            body.extend_from_slice(&[&b"--"[..], boundary, b"\r\n"].concat());
        }
        for _ in 0..rng.below(48) {
            match rng.below(4) {
                0 => body.extend_from_slice(boundary),
                1 => body.push(rng.next() as u8),
                _ => body.extend_from_slice(fragments[rng.below(fragments.len())]),
            }
        }
        let ct = [&b"multipart/form-data; boundary="[..], boundary].concat();
        let (whole, parser) = parse(&ct, &[&body]);
        check_bounds(&whole, body.len());
        let mut chunks = Vec::new();
        let mut at = 0;
        while at < body.len() {
            let end = (at + 1 + rng.below(17)).min(body.len());
            chunks.push(&body[at..end]);
            at = end;
        }
        let (split, split_parser) = parse(&ct, &chunks);
        assert_eq!(split, whole, "{body:?}");
        assert_eq!(split_parser.is_done(), parser.is_done());
        assert_eq!(split_parser.is_error(), parser.is_error());
    }
    let alphabet = b"%+&=?;\"aF0x \0\xFF";
    for _ in 0..3000 {
        let text: Vec<u8> = (0..rng.below(24))
            .map(|_| alphabet[rng.below(alphabet.len())])
            .collect();
        let mut out = vec![0u8; text.len()];
        assert!(url_decode(&text, &mut out) <= text.len());
        let (path, query) = split_target(&text);
        assert!(path.len() + query.len() <= text.len());
        for name in [&b"a"[..], b"", b"a=", b"%"] {
            let mut value = [0u8; 4];
            match query_param(query, name, &mut value) {
                Some(len) => assert!(len <= query.len()),
                None => assert!(!has_query_param(query, name)),
            }
        }
        assert!(text.starts_with(media_type(&text)));
        assert!(boundary(&text).is_none_or(|b| !b.is_empty() && b.len() <= BOUNDARY_MAX));
    }
}
