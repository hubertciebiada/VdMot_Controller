//! LittleFS file management rules (GET/DELETE /api/files): path syntax, the kind of a file,
//! whether the dashboard may delete it, and the /api/files document (port of
//! `vdm/file_manager.h`). Hardware-free; the walk and the removal are storage glue.

use crate::common::{is_printable_text, Text};
use crate::json_writer::JsonWriter;

pub const FS_PATH_MAX: usize = 95;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// /stm/<name>.bin: deleted through DELETE /api/stm/images/<name>
    StmImage = 0,
    /// /stm/<name>.part: an aborted upload
    UploadPart = 1,
    /// /log/*: the event log
    Log = 2,
    /// /sys/*: config backups, import report, discovery list
    Internal = 3,
    /// /HADiscovery.cfg: kept for a rollback to the legacy firmware
    LegacyHaList = 4,
    /// /<name>.bin (any case): an STM image the legacy firmware uploaded
    LegacyImage = 5,
    Other = 6,
}

impl FileKind {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::StmImage),
            1 => Some(Self::UploadPart),
            2 => Some(Self::Log),
            3 => Some(Self::Internal),
            4 => Some(Self::LegacyHaList),
            5 => Some(Self::LegacyImage),
            6 => Some(Self::Other),
            _ => None,
        }
    }
}

/// A file directly below `dir` ("/stm/"), no deeper level.
fn directly_in(p: &[u8], dir: &[u8]) -> bool {
    p.strip_prefix(dir)
        .is_some_and(|rest| !rest.contains(&b'/'))
}

/// '/' first, 2..FS_PATH_MAX bytes, printable text (UTF-8 allowed), segments separated by
/// single '/', none of them "." or "..", no trailing '/'.
pub fn fs_path_valid(p: &[u8]) -> bool {
    let [b'/', rest @ ..] = p else {
        return false;
    };
    if p.len() < 2 || p.len() > FS_PATH_MAX || !is_printable_text(p) {
        return false;
    }
    // "//" or a trailing '/' give an empty segment
    rest.split(|&c| c == b'/')
        .all(|seg| !matches!(seg, [] | [b'.'] | [b'.', b'.']))
}

/// Kind of a valid path (the first matching row of [`FileKind`] wins).
pub fn classify_fs_path(p: &[u8]) -> FileKind {
    if directly_in(p, b"/stm/") {
        if p.ends_with(b".part") {
            return FileKind::UploadPart;
        }
        if p.ends_with(b".bin") {
            return FileKind::StmImage;
        }
    }
    if p.starts_with(b"/log/") {
        return FileKind::Log;
    }
    if p.starts_with(b"/sys/") {
        return FileKind::Internal;
    }
    if p == b"/HADiscovery.cfg" {
        return FileKind::LegacyHaList;
    }
    if is_legacy_image_file(p) {
        return FileKind::LegacyImage;
    }
    FileKind::Other
}

pub fn file_deletable(k: FileKind) -> bool {
    matches!(
        k,
        FileKind::UploadPart | FileKind::LegacyImage | FileKind::Other
    )
}

/// "stm_image", "upload_part", "log", "internal", "legacy_ha_list", "legacy_image", "other".
pub fn file_kind_name(k: FileKind) -> &'static str {
    match k {
        FileKind::StmImage => "stm_image",
        FileKind::UploadPart => "upload_part",
        FileKind::Log => "log",
        FileKind::Internal => "internal",
        FileKind::LegacyHaList => "legacy_ha_list",
        FileKind::LegacyImage => "legacy_image",
        FileKind::Other => "other",
    }
}

/// Detail of the 403 answer for a protected kind; "" when deletable.
pub fn file_protect_reason(k: FileKind) -> &'static str {
    match k {
        FileKind::StmImage => "use DELETE /api/stm/images/<name>",
        FileKind::Log => "event log",
        FileKind::Internal => "internal file",
        FileKind::LegacyHaList => "kept for a rollback to the legacy firmware",
        FileKind::UploadPart | FileKind::LegacyImage | FileKind::Other => "",
    }
}

/// A root-level file whose name ends in ".bin" (case-insensitive), with at least one character
/// before the suffix.
pub fn is_legacy_image_file(p: &[u8]) -> bool {
    const SUFFIX: &[u8; 4] = b".bin";
    p.len() >= SUFFIX.len() + 2
        && directly_in(p, b"/")
        && p.get(p.len() - SUFFIX.len()..)
            .is_some_and(|tail| tail.eq_ignore_ascii_case(SUFFIX))
}

/// One file of the /api/files list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileEntry {
    pub path: Text<FS_PATH_MAX>,
    pub size: u32,
}

/// `{"total":<bytes>,"used":<bytes>,"truncated":false,"files":[{"path":"/stm/x.bin",
/// "size":98304,"kind":"stm_image","deletable":false},...]}`. Returns `jw.ok()`.
pub fn write_files_json(
    jw: &mut JsonWriter<'_>,
    files: &[FileEntry],
    total: u32,
    used: u32,
    truncated: bool,
) -> bool {
    jw.begin_object();
    jw.kv("total", total);
    jw.kv("used", used);
    jw.kv("truncated", truncated);
    jw.key("files");
    jw.begin_array();
    for f in files {
        let k = classify_fs_path(&f.path);
        jw.begin_object();
        jw.kv("path", &f.path);
        jw.kv("size", f.size);
        jw.kv("kind", file_kind_name(k));
        jw.kv("deletable", file_deletable(k));
        jw.end_object();
    }
    jw.end_array();
    jw.end_object();
    jw.ok()
}

#[cfg(test)]
mod tests;
