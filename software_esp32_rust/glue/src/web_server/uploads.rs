//! The uploads (C++ `onUpload`, `beginUpload`, `failUpload`, `finishUpload`, `uploadMd5`): an
//! STM image into storage, an ESP image into the update slot, streamed in the handler from the
//! request body through the response buffer and the multipart parser (design 4.5, decision
//! 7.3).
//!
//! One file per request: an upload begins at the first data of a file part (a part without
//! data begins nothing), a second file part fails the request. The first failure wins and
//! aborts what storage or OTA hold; the answer comes when the body is complete. A client that
//! goes away (or stalls for [`BODY_TIMEOUTS`] receive timeouts in a row) aborts the upload and
//! gets no answer. The loop sleeps one tick per chunk, so the idle task of core 0 runs during
//! long flash writes.

use super::views::write_image;
use super::*;
use crate::http_parse::{Event as Part, FILENAME_MAX, VALUE_MAX};
use crate::storage::{image_result_name, ImageEntry};

/// Bytes of an MD5 value that are read (32 hex digits are valid; longer values stay invalid).
const MD5_MAX: usize = 64;
/// The answer of a stored image (C++ a 160-byte buffer): a longer one is `{}`.
const IMAGE_ANSWER_SIZE: usize = 160;

/// Where an upload goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// POST /api/stm/images: storage, /stm/<name>.bin.
    StmImage,
    /// POST /api/ota/esp: the update slot.
    EspOta,
}

/// The upload of one request (C++ `Upload`).
struct Upload {
    kind: Kind,
    /// Content-Length of the request: storage's space check, the OTA size check.
    announced: usize,
    /// Storage or OTA holds this upload: a failure aborts it (C++ `kind != None`).
    open: bool,
    /// A file part delivered data: the upload began (C++ `started`).
    started: bool,
    /// The image is stored or selected for boot.
    done: bool,
    /// The first failure: status and text (C++ `failCode`, `error`).
    failure: Option<(u16, &'static str)>,
    /// The stored image (STM).
    info: ImageEntry,
    /// The current file part delivered data.
    part_data: bool,
    /// The filename of the current file part.
    filename: heapless::Vec<u8, FILENAME_MAX>,
    /// The form fields "md5" and "MD5" before the upload began (the first of each).
    md5_field: Option<heapless::Vec<u8, VALUE_MAX>>,
    md5_upper_field: Option<heapless::Vec<u8, VALUE_MAX>>,
}

impl Upload {
    fn new(kind: Kind, announced: usize) -> Self {
        Upload {
            kind,
            announced,
            open: false,
            started: false,
            done: false,
            failure: None,
            info: ImageEntry::default(),
            part_data: false,
            filename: heapless::Vec::new(),
            md5_field: None,
            md5_upper_field: None,
        }
    }
}

/// The expected MD5 of an ESP image in `out`, its length (0 = none given): the first of query
/// `md5`, form field `md5`, query `MD5`, form field `MD5`, header `X-Update-MD5`, header
/// `X-MD5` that is present (an empty one included), as C++ `uploadMd5`.
fn upload_md5(req: &dyn HttpRequest, up: &Upload, out: &mut [u8; MD5_MAX]) -> usize {
    let n = md5_of(req, b"md5", &up.md5_field, out)
        .or_else(|| md5_of(req, b"MD5", &up.md5_upper_field, out))
        .or_else(|| req.header("X-Update-MD5", out))
        .or_else(|| req.header("X-MD5", out));
    n.map_or(0, |n| n.min(MD5_MAX))
}

/// The query parameter `name`, else the form field of that name, in `out`: its length.
fn md5_of(
    req: &dyn HttpRequest,
    name: &[u8],
    field: &Option<heapless::Vec<u8, VALUE_MAX>>,
    out: &mut [u8],
) -> Option<usize> {
    if let Some(n) = query_param(split_target(req.target()).1, name, out) {
        return Some(n);
    }
    let v = field.as_ref()?;
    let n = v.len().min(out.len());
    out.get_mut(..n)?.copy_from_slice(v.get(..n)?);
    Some(n)
}

impl<'a, P, N, F, G, S, O, H> Web<'a, P, N, F, G, S, O, H>
where
    P: Platform,
    N: Nvs,
    F: Fs,
    G: HeapGate,
    S: StorageHost,
    O: OtaHost,
    H: WebHost,
{
    /// POST /api/stm/images and /api/ota/esp (the limits were checked): the body streams
    /// through the multipart parser into storage or OTA; the answer when it is complete.
    pub(super) fn upload(
        &mut self,
        req: &mut dyn HttpRequest,
        w: &mut WorkSet,
        h: &Head<'_>,
        route: ApiRoute,
    ) {
        let kind = if route == ApiRoute::StmImageUpload {
            Kind::StmImage
        } else {
            Kind::EspOta
        };
        let Some(buf) = response_buf(&mut w.response, self.ports.gate) else {
            return out_of_memory(req);
        };
        w.multipart.start(h.ctype.unwrap_or_default());
        let mut up = Upload::new(kind, h.len);
        if self.receive(req, &mut w.multipart, buf, &mut up) {
            self.finish_upload(req, &mut up);
        } else {
            self.fail(&mut up, 499, "client disconnected");
        }
    }

    /// The body through the multipart parser, one tick of sleep per chunk: true when it is
    /// complete (its end, or the close delimiter: esp_http_server drops the rest), false when
    /// the client went away or stalled.
    fn receive(
        &mut self,
        req: &mut dyn HttpRequest,
        multipart: &mut Multipart,
        buf: &mut [u8],
        up: &mut Upload,
    ) -> bool {
        let mut stalls = 0;
        loop {
            match req.read_body(buf) {
                BodyRead::Data(n) if n > 0 => {
                    stalls = 0;
                    let data = buf.get(..n).unwrap_or_default();
                    let r: &dyn HttpRequest = req;
                    multipart.feed(data, |part| self.on_part(r, up, part));
                    self.ports.clock.sleep_ms(1);
                    if multipart.is_done() {
                        return true;
                    }
                }
                BodyRead::Data(_) | BodyRead::Timeout => {
                    stalls += 1;
                    if stalls >= BODY_TIMEOUTS {
                        return false;
                    }
                }
                BodyRead::End => return true,
                BodyRead::Closed => return false,
            }
        }
    }

    /// One event of the multipart body (C++ the library's `handleUpload` calls and POST
    /// parameters).
    fn on_part(&mut self, req: &dyn HttpRequest, up: &mut Upload, part: Part<'_>) {
        match part {
            Part::Field { name, value } => {
                if up.started {
                    return; // only fields before the file count
                }
                let field = if name.is(b"md5") {
                    &mut up.md5_field
                } else if name.is(b"MD5") {
                    &mut up.md5_upper_field
                } else {
                    return;
                };
                if field.is_none() {
                    *field = heapless::Vec::from_slice(value.kept).ok();
                }
            }
            Part::FileStart { filename, .. } => {
                up.part_data = false;
                up.filename.clear();
                let _ = up.filename.extend_from_slice(filename.kept);
            }
            Part::FileData(data) => {
                if !up.part_data {
                    up.part_data = true;
                    self.first_data(req, up);
                }
                self.write(up, data);
            }
            Part::FileEnd => {
                if up.part_data {
                    self.end(up);
                }
            }
            Part::Done => {}
        }
    }

    /// The first data of a file part (C++ `index == 0`): the upload begins, or a second file
    /// fails the request.
    fn first_data(&mut self, req: &dyn HttpRequest, up: &mut Upload) {
        if up.started {
            return self.fail(up, 400, "one file per request");
        }
        up.started = true;
        match up.kind {
            Kind::StmImage => {
                let r = self.storage.image_upload_begin(&up.filename, up.announced);
                if r != ImageResult::Ok {
                    return self.fail(up, image_http_code(r), image_result_name(r));
                }
            }
            Kind::EspOta => {
                let mut md5 = [0u8; MD5_MAX];
                let n = upload_md5(req, up, &mut md5);
                if !self
                    .upload
                    .upload_begin(up.announced, md5.get(..n).unwrap_or_default())
                {
                    let e = self.upload.upload_error();
                    // "busy" and "image on trial" (an upload would overwrite the fallback)
                    let code = if matches!(e, "busy" | "image on trial") {
                        409
                    } else {
                        400
                    };
                    return self.fail(up, code, e);
                }
            }
        }
        up.open = true;
    }

    /// File data: written to storage or OTA; a failure there ends the upload.
    fn write(&mut self, up: &mut Upload, data: &[u8]) {
        if up.failure.is_some() {
            return;
        }
        match up.kind {
            Kind::StmImage => {
                let r = self.storage.image_upload_write(data);
                if r != ImageResult::Ok {
                    up.open = false; // storage removed the part file
                    self.fail(up, image_http_code(r), image_result_name(r));
                }
            }
            Kind::EspOta => {
                if !self.upload.upload_write(data) {
                    up.open = false; // ota aborted itself
                    let e = self.upload.upload_error();
                    self.fail(up, 500, e);
                }
            }
        }
    }

    /// The end of the file part (C++ the `final` call): the image is stored, or verified and
    /// selected for boot.
    fn end(&mut self, up: &mut Upload) {
        if up.failure.is_some() {
            return;
        }
        up.open = false;
        match up.kind {
            Kind::StmImage => match self.storage.image_upload_end() {
                Ok(info) => {
                    up.info = info;
                    up.done = true;
                }
                Err(r) => self.fail(up, image_http_code(r), image_result_name(r)),
            },
            Kind::EspOta => {
                if self.upload.upload_end(true) {
                    up.done = true;
                } else {
                    let e = self.upload.upload_error();
                    self.fail(up, 500, e);
                }
            }
        }
    }

    /// C++ `failUpload`: the first failure wins and aborts what storage or OTA hold.
    fn fail(&mut self, up: &mut Upload, code: u16, error: &'static str) {
        if up.failure.is_some() {
            return;
        }
        up.failure = Some((code, error));
        if !up.open {
            return;
        }
        up.open = false;
        match up.kind {
            Kind::StmImage => self.storage.image_upload_abort(),
            Kind::EspOta => {
                self.upload.upload_end(false);
            }
        }
    }

    /// The answer when the body is complete (C++ `finishUpload`).
    fn finish_upload(&mut self, req: &mut dyn HttpRequest, up: &mut Upload) {
        if !up.started {
            return send_error(req, 400, "bad_request", b"no file in request");
        }
        if !up.done {
            self.fail(up, 400, "incomplete file");
        }
        if let Some((code, error)) = up.failure {
            return send_error(req, code, "upload_failed", error.as_bytes());
        }
        if up.kind == Kind::EspOta {
            return send(req, 200, JSON, b"{\"result\":\"ok\",\"restart\":true}");
        }
        let mut buf = [0u8; IMAGE_ANSWER_SIZE];
        let mut jw = JsonWriter::new(&mut buf);
        write_image(&mut jw, &up.info, true);
        let body: &[u8] = if jw.complete() { jw.as_bytes() } else { b"{}" };
        send(req, 201, JSON, body);
    }
}
