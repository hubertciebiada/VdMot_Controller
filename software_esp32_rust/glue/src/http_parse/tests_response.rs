//! Tests of the response framing of `http_parse`: the reason phrases, heads and chunk lines of
//! AsyncWebServer_WT32_ETH01 1.6.2 (`WebResponses.cpp`) that the esp_http_server adapter puts on
//! the wire.

use super::*;

fn head(
    status: u16,
    content_type: &str,
    length: Option<usize>,
    headers: &[(&str, &str)],
) -> String {
    let mut out = [0u8; 512];
    let n = response_head(status, content_type, length, headers, &mut out).expect("fits");
    String::from_utf8(out[..n].to_vec()).expect("ASCII")
}

#[test]
fn reason_phrases_are_the_table_of_the_library() {
    let table: [(u16, &str); 40] = [
        (100, "Continue"),
        (101, "Switching Protocols"),
        (200, "OK"),
        (201, "Created"),
        (202, "Accepted"),
        (203, "Non-Authoritative Information"),
        (204, "No Content"),
        (205, "Reset Content"),
        (206, "Partial Content"),
        (300, "Multiple Choices"),
        (301, "Moved Permanently"),
        (302, "Found"),
        (303, "See Other"),
        (304, "Not Modified"),
        (305, "Use Proxy"),
        (307, "Temporary Redirect"),
        (400, "Bad Request"),
        (401, "Unauthorized"),
        (402, "Payment Required"),
        (403, "Forbidden"),
        (404, "Not Found"),
        (405, "Method Not Allowed"),
        (406, "Not Acceptable"),
        (407, "Proxy Authentication Required"),
        (408, "Request Time-out"),
        (409, "Conflict"),
        (410, "Gone"),
        (411, "Length Required"),
        (412, "Precondition Failed"),
        (413, "Request Entity Too Large"),
        (414, "Request-URI Too Large"),
        (415, "Unsupported Media Type"),
        (416, "Requested range not satisfiable"),
        (417, "Expectation Failed"),
        (500, "Internal Server Error"),
        (501, "Not Implemented"),
        (502, "Bad Gateway"),
        (503, "Service Unavailable"),
        (504, "Gateway Time-out"),
        (505, "HTTP Version not supported"),
    ];
    for (code, phrase) in table {
        assert_eq!(reason_phrase(code), phrase, "{code}");
    }
    // every other code has none: 507 (insufficient storage of the uploads) included
    for code in (0..=u16::MAX).filter(|c| !table.iter().any(|(t, _)| t == c)) {
        assert_eq!(reason_phrase(code), "", "{code}");
    }
}

#[test]
fn head_of_a_document_has_the_length_the_type_and_the_headers_in_order() {
    assert_eq!(
        head(
            200,
            "application/json",
            Some(42),
            &[("Cache-Control", "no-store")]
        ),
        "HTTP/1.1 200 OK\r\nContent-Length: 42\r\nContent-Type: application/json\r\n\
         Cache-Control: no-store\r\nAccept-Ranges: none\r\n\r\n"
    );
    assert_eq!(
        head(
            200,
            "text/html; charset=utf-8",
            Some(1234),
            &[
                ("Content-Encoding", "gzip"),
                ("ETag", "\"1a2b3c4d\""),
                ("Cache-Control", "no-cache"),
            ]
        ),
        "HTTP/1.1 200 OK\r\nContent-Length: 1234\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Encoding: gzip\r\nETag: \"1a2b3c4d\"\r\nCache-Control: no-cache\r\n\
         Accept-Ranges: none\r\n\r\n"
    );
}

#[test]
fn head_without_a_type_has_no_content_type_line() {
    assert_eq!(
        head(304, "", Some(0), &[]),
        "HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\nAccept-Ranges: none\r\n\r\n"
    );
    assert_eq!(
        head(204, "", Some(0), &[("X-A", "1")]),
        "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nX-A: 1\r\nAccept-Ranges: none\r\n\r\n"
    );
}

#[test]
fn head_of_a_code_without_a_phrase_keeps_the_space() {
    assert_eq!(
        head(507, "application/json", Some(5), &[]),
        "HTTP/1.1 507 \r\nContent-Length: 5\r\nContent-Type: application/json\r\n\
         Accept-Ranges: none\r\n\r\n"
    );
}

#[test]
fn head_of_a_chunked_response_has_no_length_and_ends_with_the_transfer_encoding() {
    assert_eq!(
        head(
            200,
            "text/plain",
            None,
            &[("Content-Disposition", "attachment; filename=\"events.log\"")]
        ),
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
         Content-Disposition: attachment; filename=\"events.log\"\r\nAccept-Ranges: none\r\n\
         Transfer-Encoding: chunked\r\n\r\n"
    );
    assert_eq!(
        head(200, "", None, &[]),
        "HTTP/1.1 200 OK\r\nAccept-Ranges: none\r\nTransfer-Encoding: chunked\r\n\r\n"
    );
}

#[test]
fn head_that_does_not_fit_is_none_and_the_buffer_keeps_room_for_the_nul() {
    let full = head(
        404,
        "application/json",
        Some(7),
        &[("Cache-Control", "no-store")],
    );
    let n = full.len();
    // the core buffer convention: n bytes of text need n + 1 bytes
    let mut out = vec![0u8; n + 1];
    assert_eq!(
        response_head(
            404,
            "application/json",
            Some(7),
            &[("Cache-Control", "no-store")],
            &mut out
        ),
        Some(n)
    );
    assert_eq!(&out[..n], full.as_bytes());
    for cut in [n, n - 1, 20, 1, 0] {
        let mut out = vec![0u8; cut];
        assert_eq!(
            response_head(
                404,
                "application/json",
                Some(7),
                &[("Cache-Control", "no-store")],
                &mut out
            ),
            None,
            "{cut}"
        );
    }
    // the empty line alone does not fit either
    let bare = head(200, "", Some(0), &[]);
    let mut out = vec![0u8; bare.len()];
    assert_eq!(response_head(200, "", Some(0), &[], &mut out), None);
}

#[test]
fn chunk_size_lines_are_lowercase_hex() {
    let mut out = [0u8; CHUNK_LINE_MAX];
    for (len, line) in [
        (1usize, "1\r\n"),
        (10, "a\r\n"),
        (255, "ff\r\n"),
        (4096, "1000\r\n"),
        (0xABCDEF, "abcdef\r\n"),
        (usize::MAX, "ffffffffffffffff\r\n"),
    ] {
        let n = chunk_size_line(len, &mut out);
        assert_eq!(&out[..n], line.as_bytes(), "{len}");
    }
    assert_eq!(CRLF, b"\r\n");
    assert_eq!(LAST_CHUNK, b"0\r\n\r\n");
    // the longest line (16 hex digits on the 64-bit host) fits with the NUL
    assert_eq!(CHUNK_LINE_MAX, 16 + 2 + 1);
}
