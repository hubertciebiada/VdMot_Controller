//! Request target, query and multipart parsing (AsyncWebServer_WT32_ETH01 1.6.2 behaviour).
//!
//! The esp_http_server adapter hands the glue the raw request target, header values without
//! leading blanks and the body in chunks of any size ([`crate::port::HttpRequest`]). This module
//! gives `web_server` what the library (`WebRequest.cpp`) gave the C++ handlers:
//!
//! | Library | Here |
//! |---|---|
//! | `_parseReqHead`: `url()` and the GET parameters | [`split_target`], [`url_decode`] |
//! | `_addGetParams` + `getParam(name)`, `hasParam(name)` | [`query_param`], [`has_query_param`] |
//! | `_parseReqHeader`, Content-Type: `contentType()`, `_isMultipart`, `_boundary` | [`media_type`], [`is_multipart`], [`boundary`] |
//! | `_parseMultipartPostByte`: POST parameters and `handleUpload` calls | [`Multipart`], [`Event`] |
//!
//! The Arduino `String` semantics the library relied on are kept: `indexOf` gives -1 for a
//! missing byte (so `indexOf(c) + 2` is 1), `substring(left, right)` swaps its bounds when `left`
//! is the larger one, cuts `right` to the length and gives "" for a `left` at or past the end.
//! No heap, bounded state, no panic on any input; bytes are never assumed to be UTF-8.
//!
//! Changes against the library (docs/rust/PORT-NOTES.md):
//! - A multipart body ends at its close delimiter ("\r\n--" boundary and one '-'); the bytes after
//!   it are ignored (RFC 2046). The library expected exactly "--\r\n" there (it set Content-Length
//!   to that), so it never answered a body without the final CRLF or with an epilogue. A body that
//!   ends before the close delimiter leaves the parser unfinished ([`Multipart::is_done`]).
//! - A part ends only once its delimiter is confirmed by "\r\n" or '-'; otherwise the delimiter
//!   bytes are data. The library ended the part at the boundary match and wrote the bytes back
//!   into the ended part when neither followed: for a file through its freed (NULL) buffer, a
//!   crash of the C++ firmware; for a form field as a second parameter of the same name.
//! - The boundary has 1 to [`BOUNDARY_MAX`] bytes (RFC 2046); otherwise the parser is in its error
//!   state at once. With an empty boundary the library never ended the first part, with 256 bytes
//!   or more its `uint8_t` write-back loops never ended.
//! - Part header lines are kept to [`HEADER_LINE_MAX`] bytes; names, values and filenames to
//!   [`NAME_MAX`], [`VALUE_MAX`] and [`FILENAME_MAX`] bytes plus their full lengths. The library's
//!   `String`s had no bound.
//! - A NUL byte in a part header line is a byte like any other; the library's `indexOf`, `==` and
//!   `equalsIgnoreCase` (`strchr`, `strcmp`) stopped at it.

/// Longest boundary accepted (RFC 2046). A longer or an empty one puts [`Multipart`] into its
/// error state.
pub const BOUNDARY_MAX: usize = 70;
/// Bytes kept of a part header line; a longer line is read as its first `HEADER_LINE_MAX` bytes.
pub const HEADER_LINE_MAX: usize = 256;
/// Bytes kept of the field name of a part (form field or file).
pub const NAME_MAX: usize = 32;
/// Bytes kept of the value of a form field (an MD5 field is valid with 32).
pub const VALUE_MAX: usize = 64;
/// Bytes kept of the filename of a file part (an STM image name has at most 31 + ".bin").
pub const FILENAME_MAX: usize = 64;

/// "\r\n--" + boundary + "\r\n".
const DELIMITER_MAX: usize = BOUNDARY_MAX + 6;

// ---------------------------------------------------------------- target and query

/// Splits the raw request target into path and query at its first '?', as `_parseReqHead`
/// did: only when that '?' is not the first byte (`index > 0`). Otherwise the whole target is
/// the path and the query is empty. Neither part is decoded.
pub fn split_target(target: &[u8]) -> (&[u8], &[u8]) {
    match split_once(target, b'?') {
        Some((path, query)) if !path.is_empty() => (path, query),
        _ => (target, &[]),
    }
}

/// `urlDecode` of the library (the path and every query name and value): '+' is a space, also in
/// the path; '%' and the two bytes after it are one byte, the low byte of
/// `strtol("0x" + c1 + c2, NULL, 16)`, which reads hex digits up to the first other byte
/// ("%4G" is 0x04, "%G4" and "%-1" are 0x00); a '%' with fewer than two bytes after it stays as it
/// is. Writes what fits into `out` and returns the full decoded length, which is never more than
/// `text.len()`. The result may hold NUL bytes ("%00", "%G0"): the C++ handlers read some values
/// as C strings, which end at the first NUL.
pub fn url_decode(text: &[u8], out: &mut [u8]) -> usize {
    let mut len = 0;
    for byte in UrlDecoded(text) {
        if let Some(slot) = out.get_mut(len) {
            *slot = byte;
        }
        len += 1;
    }
    len
}

/// `getParam(name)` of a GET parameter (`_addGetParams`): the query splits at every '&'; an item
/// is the name up to its first '=' (the whole item without one) and the value after it ("" for
/// "a=" and "a"); names and values are decoded with [`url_decode`]; empty names count; the first
/// parameter whose decoded name equals `name` byte for byte wins. Writes what fits of its decoded
/// value into `out` and returns the full length of the value; `None` when no parameter has that
/// name. `query` is the query part of [`split_target`].
pub fn query_param(query: &[u8], name: &[u8], out: &mut [u8]) -> Option<usize> {
    find_param(query, name).map(|raw| url_decode(raw, out))
}

/// `hasParam(name)` of a GET parameter: [`query_param`] finds one.
pub fn has_query_param(query: &[u8], name: &[u8]) -> bool {
    find_param(query, name).is_some()
}

/// The raw value of the first parameter of `query` whose decoded name is `name`.
fn find_param<'a>(query: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let mut rest = query;
    // a '&' at the end does not start another (empty) item
    while !rest.is_empty() {
        let (item, next) = split_once(rest, b'&').unwrap_or((rest, &[]));
        let (raw_name, raw_value) = split_once(item, b'=').unwrap_or((item, &[]));
        if UrlDecoded(raw_name).eq(name.iter().copied()) {
            return Some(raw_value);
        }
        rest = next;
    }
    None
}

/// The bytes before and after the first `separator` of `s`; `None` without one.
fn split_once(s: &[u8], separator: u8) -> Option<(&[u8], &[u8])> {
    let at = s.iter().position(|&b| b == separator)?;
    let (head, tail) = s.split_at(at);
    Some((head, tail.get(1..).unwrap_or_default()))
}

/// The decoded bytes of `urlDecode` (see [`url_decode`]).
struct UrlDecoded<'a>(&'a [u8]);

impl Iterator for UrlDecoded<'_> {
    type Item = u8;

    fn next(&mut self) -> Option<u8> {
        let (&byte, tail) = self.0.split_first()?;
        self.0 = tail;
        match (byte, tail) {
            (b'%', [hi, lo, rest @ ..]) => {
                self.0 = rest;
                Some(percent_byte(*hi, *lo))
            }
            (b'+', _) => Some(b' '),
            _ => Some(byte),
        }
    }
}

/// Low byte of `strtol("0x" + hi + lo, NULL, 16)`: hex digits up to the first other byte.
fn percent_byte(hi: u8, lo: u8) -> u8 {
    match (hex_digit(hi), hex_digit(lo)) {
        (Some(h), Some(l)) => h * 16 + l,
        (Some(h), None) => h,
        (None, _) => 0,
    }
}

/// Value of a hex digit (0-9, a-f, A-F).
fn hex_digit(byte: u8) -> Option<u8> {
    char::from(byte)
        .to_digit(16)
        .and_then(|d| u8::try_from(d).ok())
}

// ---------------------------------------------------------------- Content-Type

/// `contentType()`: the Content-Type value up to its first ';' (the whole value without one),
/// not trimmed.
pub fn media_type(content_type: &[u8]) -> &[u8] {
    split_once(content_type, b';').map_or(content_type, |(media, _)| media)
}

/// `_isMultipart`: the Content-Type value starts with "multipart/" (case-sensitive).
pub fn is_multipart(content_type: &[u8]) -> bool {
    content_type.starts_with(b"multipart/")
}

/// `_boundary`: the Content-Type value after its first '=' (the whole value without one), every
/// '"' removed. `None` when that is empty or longer than [`BOUNDARY_MAX`] (RFC 2046; the library
/// took any length).
pub fn boundary(content_type: &[u8]) -> Option<heapless::Vec<u8, BOUNDARY_MAX>> {
    let value = split_once(content_type, b'=').map_or(content_type, |(_, after)| after);
    let mut out = heapless::Vec::new();
    for &byte in value {
        if byte != b'"' {
            out.push(byte).ok()?;
        }
    }
    (!out.is_empty()).then_some(out)
}

/// "\r\n--" + [`boundary`] + "\r\n".
fn delimiter(content_type: &[u8]) -> Option<heapless::Vec<u8, DELIMITER_MAX>> {
    let boundary = boundary(content_type)?;
    let mut out = heapless::Vec::new();
    out.extend_from_slice(b"\r\n--").ok()?;
    out.extend_from_slice(&boundary).ok()?;
    out.extend_from_slice(b"\r\n").ok()?;
    Some(out)
}

// ---------------------------------------------------------------- multipart

/// A name, value or filename of a part: the bytes kept (at most the bound of its kind:
/// [`NAME_MAX`], [`VALUE_MAX`], [`FILENAME_MAX`]) and the full length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounded<'a> {
    /// The first bytes, up to the bound.
    pub kept: &'a [u8],
    /// The full length; more than `kept.len()` when the text was longer than the bound.
    pub len: usize,
}

impl Bounded<'_> {
    /// The whole text is `text`, byte for byte (`String ==` with a name without NUL).
    pub fn is(&self, text: &[u8]) -> bool {
        self.len == text.len() && self.kept == text
    }
}

/// What a [`Multipart`] parser found, in body order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event<'a> {
    /// A form field (a part without filename and without Content-Type) ended: the library's
    /// POST parameter (`getParam(name, true)`), here with bounded copies.
    Field {
        /// The part's name (carried over from an earlier part when its headers set none).
        name: Bounded<'a>,
        /// The part's data.
        value: Bounded<'a>,
    },
    /// The headers of a file part ended (a filename in its Content-Disposition, or a
    /// Content-Type header). The library called `handleUpload` first with the first file byte:
    /// an empty file part gives `FileStart` and `FileEnd` without `FileData`, where the library
    /// made no call at all, so the C++ handler began no upload for it and answered "no file in
    /// request" when no other file came.
    FileStart {
        /// The part's name.
        name: Bounded<'a>,
        /// The filename ("" or carried over from an earlier part when its headers set none).
        filename: Bounded<'a>,
    },
    /// Bytes of the file, never empty, in body order: slices of the input, or of the parser
    /// for delimiter bytes held back over a chunk end that turned out to be data.
    FileData(&'a [u8]),
    /// The delimiter after the file part was confirmed (the library's final `handleUpload`).
    FileEnd,
    /// The close delimiter: the multipart body is complete, later bytes are ignored.
    Done,
}

/// Parser state (the library's `_multiParseState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// The first line "--" boundary "\r\n" (`EXPECT_BOUNDARY`): index of the next byte in the
    /// delimiter, which holds that line from index 2.
    Preamble(usize),
    /// Part header lines up to an empty line (`PARSE_HEADERS`).
    Headers,
    /// Part data (`WAIT_FOR_RETURN1`).
    Data,
    /// Part data and then this many bytes of "\r\n--" boundary "\r\n" (`EXPECT_FEED1` up to
    /// `EXPECT_FEED2`); after "\r\n--" boundary a '-' closes the body (`DASH3_OR_RETURN2`).
    Delimiter(usize),
    /// After the close delimiter (`PARSING_FINISHED`): every byte is ignored.
    Done,
    /// No valid boundary, or a body that does not start with its first line (`PARSE_ERROR`):
    /// every byte is ignored.
    Error,
}

/// A bounded copy: the first `N` bytes of a text and its full length.
#[derive(Debug)]
struct Text<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Text<N> {
    const EMPTY: Self = Self {
        bytes: [0; N],
        len: 0,
    };

    fn clear(&mut self) {
        self.len = 0;
    }

    /// Appends what fits of `data`; the full length grows by all of it.
    fn push(&mut self, data: &[u8]) {
        let kept = self.len.min(N);
        for (slot, &byte) in self.bytes.iter_mut().skip(kept).zip(data) {
            *slot = byte;
        }
        self.len = self.len.saturating_add(data.len());
    }

    fn set(&mut self, data: &[u8]) {
        self.clear();
        self.push(data);
    }

    fn view(&self) -> Bounded<'_> {
        Bounded {
            kept: self.bytes.get(..self.len.min(N)).unwrap_or_default(),
            len: self.len,
        }
    }
}

/// The library's item state (`_itemName`, `_itemFilename`, `_itemIsFile`, `_itemValue`). Name and
/// filename are not reset between parts: a part whose headers set none carries the earlier one.
#[derive(Debug)]
struct Part {
    name: Text<NAME_MAX>,
    filename: Text<FILENAME_MAX>,
    value: Text<VALUE_MAX>,
    file: bool,
}

impl Part {
    const EMPTY: Self = Self {
        name: Text::EMPTY,
        filename: Text::EMPTY,
        value: Text::EMPTY,
        file: false,
    };

    /// One part header line without CR and LF bytes: a line longer than 12 bytes that starts
    /// with "Content-Type" makes the part a file; one that starts with "Content-Disposition" sets
    /// name and filename. Both prefixes are matched ignoring ASCII case.
    fn header(&mut self, line: &[u8]) {
        if line.len() > 12 && starts_with_ignore_case(line, b"content-type") {
            self.file = true;
        } else if starts_with_ignore_case(line, b"content-disposition") {
            // the library also wanted more than 19 bytes; 19 bytes have no '=' to find a name
            self.disposition(line);
        }
    }

    /// Content-Disposition the way the library read it: the text from two bytes after the first
    /// ';' (it assumed "; "), then for each ';'-separated segment while the ';' is not the first
    /// byte, and for the rest after the last one: the name is the text up to the first '=' of
    /// the rest, the value runs from two bytes after that '=' (it assumed a quote) to one byte
    /// before the ';' or the end (the closing quote), as `substring` cuts them. So `name="x"`
    /// gives "x", a ';' inside a quoted filename ends the filename there, `name=x` gives "x"
    /// and `name=xy` gives "".
    fn disposition(&mut self, line: &[u8]) {
        let mut rest = substring_from(line, index_of(line, b';').map_or(1, |at| at + 2));
        loop {
            let semicolon = index_of(rest, b';').filter(|&at| at > 0);
            let end = match semicolon {
                Some(at) => at - 1,
                None => rest.len().saturating_sub(1),
            };
            let equals = index_of(rest, b'=');
            let name = substring(rest, 0, equals.unwrap_or(rest.len()));
            let value = substring(rest, equals.map_or(1, |at| at + 2), end);
            self.assign(name, value);
            match semicolon {
                Some(at) => rest = substring_from(rest, at + 2),
                None => break,
            }
        }
    }

    fn assign(&mut self, name: &[u8], value: &[u8]) {
        if name == b"name" {
            self.name.set(value);
        } else if name == b"filename" {
            self.filename.set(value);
            self.file = true;
        }
    }

    /// Part data: a file passes it on (never empty), a form field keeps what fits.
    fn data<F: FnMut(Event<'_>)>(&mut self, bytes: &[u8], on_event: &mut F) {
        if !self.file {
            self.value.push(bytes);
        } else if !bytes.is_empty() {
            on_event(Event::FileData(bytes));
        }
    }
}

/// `s` starts with `prefix`, ASCII case ignored (`equalsIgnoreCase` of the C locale).
fn starts_with_ignore_case(s: &[u8], prefix: &[u8]) -> bool {
    s.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// `indexOf(byte)`: `None` for the library's -1.
fn index_of(s: &[u8], byte: u8) -> Option<usize> {
    s.iter().position(|&b| b == byte)
}

/// `substring(left, right)`: the bounds swapped when `left > right`, `right` cut to the length,
/// "" when `left` is at or past the end.
fn substring(s: &[u8], left: usize, right: usize) -> &[u8] {
    let end = left.max(right).min(s.len());
    s.get(left.min(right)..end).unwrap_or_default()
}

/// `substring(left)`: from `left` to the end, "" when `left` is at or past the end.
fn substring_from(s: &[u8], left: usize) -> &[u8] {
    s.get(left..).unwrap_or_default()
}

/// Streaming multipart/form-data parser with the state machine of the library's
/// `_parseMultipartPostByte`, fed with the body in chunks of any size: every split of a body
/// gives the same events (only the cuts between [`Event::FileData`] slices move). No heap; the
/// state is bounded (about 0.6 KB, so it belongs in a boot box or the web working set:
/// [`Multipart::EMPTY`], then [`Multipart::start`] for each body).
///
/// - The body starts with "--" boundary "\r\n"; anything else (a preamble, another first line) is
///   a parse error: nothing is emitted from then on ([`Multipart::is_error`]).
/// - Part header lines (CR bytes dropped, LF ends a line) run up to an empty line; see
///   [`Event::FileStart`] for what makes a part a file.
/// - Part data runs up to "\r\n--" boundary; then "\r\n" ends the part and the next part's headers
///   follow, a '-' ends the part and the multipart body ([`Event::Done`]; the library looked at
///   the first '-' only). Anything else makes the delimiter bytes data, and the byte is read again
///   as data (the library's write-back), so a CR in it may start the delimiter.
#[derive(Debug)]
pub struct Multipart {
    /// "\r\n--" boundary "\r\n"; empty without a valid boundary.
    delimiter: heapless::Vec<u8, DELIMITER_MAX>,
    state: State,
    /// The header line read so far.
    line: Text<HEADER_LINE_MAX>,
    part: Part,
}

/// Part data of the chunk being fed that is not passed on yet.
#[derive(Clone, Copy, Debug, Default)]
struct Pending {
    /// First data byte of the current part in the chunk.
    run: Option<usize>,
    /// First byte of a delimiter candidate that started in the chunk (`run <= candidate`). The
    /// bytes of a candidate that started in an earlier chunk are those of the delimiter.
    candidate: Option<usize>,
}

impl Multipart {
    /// A parser in its error state, until [`Multipart::start`].
    pub const EMPTY: Self = Self {
        delimiter: heapless::Vec::new(),
        state: State::Error,
        line: Text::EMPTY,
        part: Part::EMPTY,
    };

    /// Readies the parser for the body of a request with the Content-Type value `content_type`
    /// (the caller checked [`is_multipart`]); nothing of an earlier body is kept. Without a valid
    /// [`boundary`] the parser is in its error state at once.
    pub fn start(&mut self, content_type: &[u8]) {
        match delimiter(content_type) {
            Some(delimiter) => {
                self.delimiter = delimiter;
                self.state = State::Preamble(2);
            }
            None => {
                self.delimiter.clear();
                self.state = State::Error;
            }
        }
        self.line.clear();
        self.part = Part::EMPTY;
    }

    /// The close delimiter was seen.
    pub fn is_done(&self) -> bool {
        self.state == State::Done
    }

    /// No valid boundary, or the body did not start with "--" boundary "\r\n" (also before
    /// [`Multipart::start`]).
    pub fn is_error(&self) -> bool {
        self.state == State::Error
    }

    /// Parses the next chunk of the body and calls `on_event` for every event in it, in body
    /// order. Data before a possible delimiter at the end of the chunk is passed on; the bytes
    /// that may start the delimiter are held back until a later chunk decides.
    pub fn feed<F: FnMut(Event<'_>)>(&mut self, input: &[u8], mut on_event: F) {
        let mut pending = Pending::default();
        for (at, &byte) in input.iter().enumerate() {
            match self.state {
                State::Preamble(next) => self.preamble_byte(next, byte),
                State::Headers => self.header_byte(byte, &mut on_event),
                State::Data => self.data_byte(at, byte, &mut pending),
                State::Delimiter(matched) => {
                    self.delimiter_byte(input, at, byte, matched, &mut pending, &mut on_event);
                }
                State::Done | State::Error => break,
            }
        }
        let end = match self.state {
            State::Data => Some(input.len()),
            State::Delimiter(_) => pending.candidate,
            _ => None,
        };
        self.pass_on(input, pending.run, end, &mut on_event);
    }

    /// `EXPECT_BOUNDARY`: the body must start with "--" boundary "\r\n".
    fn preamble_byte(&mut self, next: usize, byte: u8) {
        self.state = if self.delimiter.get(next) != Some(&byte) {
            State::Error
        } else if next + 1 == self.delimiter.len() {
            State::Headers
        } else {
            State::Preamble(next + 1)
        };
    }

    /// `PARSE_HEADERS`: CR bytes are dropped, LF ends a line, an empty line ends the headers.
    fn header_byte<F: FnMut(Event<'_>)>(&mut self, byte: u8, on_event: &mut F) {
        match byte {
            b'\n' if self.line.len == 0 => {
                self.state = State::Data;
                self.part.value.clear();
                if self.part.file {
                    on_event(Event::FileStart {
                        name: self.part.name.view(),
                        filename: self.part.filename.view(),
                    });
                }
            }
            b'\n' => {
                self.part.header(self.line.view().kept);
                self.line.clear();
            }
            b'\r' => {}
            _ => self.line.push(&[byte]),
        }
    }

    /// `WAIT_FOR_RETURN1`: data; a CR may start the delimiter.
    fn data_byte(&mut self, at: usize, byte: u8, pending: &mut Pending) {
        if pending.run.is_none() {
            pending.run = Some(at);
        }
        if byte == b'\r' {
            pending.candidate = Some(at);
            self.state = State::Delimiter(1);
        }
    }

    /// `EXPECT_FEED1` up to `EXPECT_FEED2`: `matched` bytes of the delimiter are behind.
    fn delimiter_byte<F: FnMut(Event<'_>)>(
        &mut self,
        input: &[u8],
        at: usize,
        byte: u8,
        matched: usize,
        pending: &mut Pending,
        on_event: &mut F,
    ) {
        let full = self.delimiter.len();
        if byte == b'-' && matched + 2 == full {
            // "\r\n--" boundary "-": the close delimiter
            self.end_part(input, *pending, on_event);
            self.state = State::Done;
            on_event(Event::Done);
        } else if self.delimiter.get(matched) == Some(&byte) {
            if matched + 1 == full {
                // "\r\n--" boundary "\r\n": the next part's headers follow
                self.end_part(input, *pending, on_event);
                *pending = Pending::default();
                self.state = State::Headers;
                self.part.file = false;
            } else {
                self.state = State::Delimiter(matched + 1);
            }
        } else {
            // no delimiter: its bytes are data, this byte is read again as data
            if pending.candidate.is_none() {
                let held = self.delimiter.get(..matched).unwrap_or_default();
                self.part.data(held, on_event);
            }
            pending.candidate = None;
            self.state = State::Data;
            self.data_byte(at, byte, pending);
        }
    }

    /// The part data `input[from..to]`, when both are known.
    fn pass_on<F: FnMut(Event<'_>)>(
        &mut self,
        input: &[u8],
        from: Option<usize>,
        to: Option<usize>,
        on_event: &mut F,
    ) {
        if let (Some(from), Some(to)) = (from, to) {
            self.part
                .data(input.get(from..to).unwrap_or_default(), on_event);
        }
    }

    /// The delimiter after the current part is confirmed: the part's data in this chunk before
    /// it, then the end of the part.
    fn end_part<F: FnMut(Event<'_>)>(&mut self, input: &[u8], pending: Pending, on_event: &mut F) {
        self.pass_on(input, pending.run, pending.candidate, on_event);
        if self.part.file {
            on_event(Event::FileEnd);
        } else {
            on_event(Event::Field {
                name: self.part.name.view(),
                value: self.part.value.view(),
            });
        }
    }
}

#[cfg(test)]
mod tests;
