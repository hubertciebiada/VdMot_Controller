//! Fake LittleFS: a tree of nodes (persistent, owned by the board), open handles with EBUSY
//! semantics, scripted failures per operation and path, block-granular usage.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use super::{lock, Journal};
use crate::port::{Fs, FsEntry, FsFile, OpenMode};

/// One node of the tree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FsNode {
    pub(crate) dir: bool,
    pub(crate) data: Vec<u8>,
}

/// Persistent content: the partition holds a LittleFS, and its nodes by absolute path ("/" is
/// implicit).
#[derive(Clone, Debug)]
pub(crate) struct FsStore {
    pub(crate) formatted: bool,
    pub(crate) nodes: BTreeMap<String, FsNode>,
}

impl Default for FsStore {
    fn default() -> Self {
        FsStore {
            formatted: true,
            nodes: BTreeMap::new(),
        }
    }
}

/// A scripted failure: the next `count` operations `op` (open, read, write, seek, rename,
/// remove, mkdir) on `path` ("" = any path) fail.
#[derive(Clone, Debug)]
pub(crate) struct FsFailure {
    pub(crate) op: &'static str,
    pub(crate) path: String,
    pub(crate) count: u32,
}

/// Knobs and counters of one boot.
pub(crate) struct FsKnobs {
    pub(crate) mounted: bool,
    /// `mount` of a formatted partition succeeds.
    pub(crate) mount_ok: bool,
    pub(crate) format_ok: bool,
    pub(crate) mounts: u32,
    pub(crate) formats: u32,
    /// The spiffs partition of the board's default table.
    pub(crate) total_bytes: u32,
    pub(crate) block_size: u32,
    /// Superblocks.
    pub(crate) base_used: u32,
    pub(crate) failures: Vec<FsFailure>,
    pub(crate) opens: u32,
    pub(crate) write_opens: u32,
    pub(crate) writes: u32,
    pub(crate) bytes_written: u64,
    pub(crate) renames: u32,
    pub(crate) removes: u32,
    /// Open handles per path.
    pub(crate) open: BTreeMap<String, u32>,
}

impl Default for FsKnobs {
    fn default() -> Self {
        FsKnobs {
            mounted: false,
            mount_ok: true,
            format_ok: true,
            mounts: 0,
            formats: 0,
            total_bytes: 0x17_0000,
            block_size: 4096,
            base_used: 2 * 4096,
            failures: Vec::new(),
            opens: 0,
            write_opens: 0,
            writes: 0,
            bytes_written: 0,
            renames: 0,
            removes: 0,
            open: BTreeMap::new(),
        }
    }
}

/// The LittleFS partition of a boot.
#[derive(Clone)]
pub(crate) struct FakeFs {
    store: Arc<Mutex<FsStore>>,
    knobs: Arc<Mutex<FsKnobs>>,
    journal: Journal,
}

fn parent_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) | None => "/",
        Some(i) => &path[..i],
    }
}

fn blocks(len: usize, block: u32) -> u32 {
    (len as u32).div_ceil(block)
}

impl FakeFs {
    pub(crate) fn new(store: Arc<Mutex<FsStore>>, journal: Journal) -> Self {
        FakeFs {
            store,
            knobs: Arc::default(),
            journal,
        }
    }
    pub(crate) fn knobs(&self) -> MutexGuard<'_, FsKnobs> {
        lock(&self.knobs)
    }
    /// Fails the next `count` operations `op` on `path` ("" = any).
    pub(crate) fn fail(&self, op: &'static str, path: &str, count: u32) {
        lock(&self.knobs).failures.push(FsFailure {
            op,
            path: path.to_string(),
            count,
        });
    }
    /// Writes a file directly, creating its parents (test setup).
    pub(crate) fn put(&self, path: &str, content: &[u8]) {
        let mut s = lock(&self.store);
        Self::mkdirs(&mut s, parent_of(path));
        s.nodes.insert(
            path.to_string(),
            FsNode {
                dir: false,
                data: content.to_vec(),
            },
        );
    }
    fn mkdirs(s: &mut FsStore, path: &str) {
        if path == "/" || path.is_empty() {
            return;
        }
        Self::mkdirs(s, parent_of(path));
        s.nodes.entry(path.to_string()).or_insert(FsNode {
            dir: true,
            data: Vec::new(),
        });
    }
    /// Content of a file; `None` when missing or a directory.
    pub(crate) fn read(&self, path: &str) -> Option<Vec<u8>> {
        let s = lock(&self.store);
        s.nodes.get(path).filter(|n| !n.dir).map(|n| n.data.clone())
    }
    /// Every path in the tree, in order.
    pub(crate) fn paths(&self) -> Vec<String> {
        lock(&self.store).nodes.keys().cloned().collect()
    }
    /// The partition holds a LittleFS (persistent).
    pub(crate) fn set_formatted(&self, formatted: bool) {
        lock(&self.store).formatted = formatted;
    }
    /// Handles open now.
    pub(crate) fn open_handles(&self) -> u32 {
        lock(&self.knobs).open.values().sum()
    }
    fn used(&self, s: &FsStore, k: &FsKnobs) -> u32 {
        k.base_used
            + s.nodes
                .values()
                .map(|n| {
                    if n.dir {
                        k.block_size
                    } else {
                        blocks(n.data.len(), k.block_size) * k.block_size
                    }
                })
                .sum::<u32>()
    }
    fn should_fail(k: &mut FsKnobs, op: &str, path: &str) -> bool {
        let hit = k
            .failures
            .iter_mut()
            .find(|f| f.op == op && (f.path.is_empty() || f.path == path) && f.count > 0);
        match hit {
            Some(f) => {
                f.count -= 1;
                true
            }
            None => false,
        }
    }
    fn exists_in(s: &FsStore, path: &str) -> bool {
        path == "/" || s.nodes.contains_key(path)
    }
    fn is_dir(s: &FsStore, path: &str) -> bool {
        path == "/" || s.nodes.get(path).is_some_and(|n| n.dir)
    }
}

impl Fs for FakeFs {
    type File = FakeFile;
    fn mount(&self) -> bool {
        let s = lock(&self.store);
        let mut k = lock(&self.knobs);
        k.mounts += 1;
        k.mounted = s.formatted && k.mount_ok;
        self.journal.note(format!("fs mount {}", k.mounted));
        k.mounted
    }
    fn format(&self) -> bool {
        let mut s = lock(&self.store);
        let mut k = lock(&self.knobs);
        k.formats += 1;
        self.journal.note("fs format");
        if !k.format_ok {
            return false;
        }
        s.nodes.clear();
        s.formatted = true;
        true
    }
    fn open(&self, path: &str, mode: OpenMode) -> Option<FakeFile> {
        let mut s = lock(&self.store);
        let mut k = lock(&self.knobs);
        k.opens += 1;
        if !k.mounted || Self::should_fail(&mut k, "open", path) {
            return None;
        }
        match s.nodes.get(path) {
            Some(n) if n.dir => return None,
            Some(_) => {}
            None => {
                if mode == OpenMode::Read || !Self::is_dir(&s, parent_of(path)) {
                    return None;
                }
            }
        }
        let mut pos = 0;
        if mode != OpenMode::Read {
            k.write_opens += 1;
            let node = s.nodes.entry(path.to_string()).or_default();
            if mode == OpenMode::Write {
                node.data.clear();
            } else {
                pos = node.data.len();
            }
        }
        *k.open.entry(path.to_string()).or_default() += 1;
        Some(FakeFile {
            fs: self.clone(),
            path: path.to_string(),
            mode,
            pos,
        })
    }
    fn exists(&self, path: &str) -> bool {
        let s = lock(&self.store);
        let k = lock(&self.knobs);
        k.mounted && Self::exists_in(&s, path)
    }
    fn mkdir(&self, path: &str) -> bool {
        let mut s = lock(&self.store);
        let mut k = lock(&self.knobs);
        if !k.mounted
            || Self::should_fail(&mut k, "mkdir", path)
            || Self::exists_in(&s, path)
            || !Self::is_dir(&s, parent_of(path))
        {
            return false;
        }
        s.nodes.insert(
            path.to_string(),
            FsNode {
                dir: true,
                data: Vec::new(),
            },
        );
        true
    }
    fn remove(&self, path: &str) -> bool {
        let mut s = lock(&self.store);
        let mut k = lock(&self.knobs);
        if !k.mounted
            || Self::should_fail(&mut k, "remove", path)
            || k.open.get(path).copied().unwrap_or(0) > 0
        {
            return false;
        }
        let prefix = format!("{path}/");
        if s.nodes.keys().any(|p| p.starts_with(&prefix)) {
            return false; // a directory that is not empty
        }
        if s.nodes.remove(path).is_none() {
            return false;
        }
        k.removes += 1;
        self.journal.note(format!("fs remove {path}"));
        true
    }
    fn rename(&self, from: &str, to: &str) -> bool {
        let mut s = lock(&self.store);
        let mut k = lock(&self.knobs);
        let busy = |p: &str| k.open.get(p).copied().unwrap_or(0) > 0;
        if !k.mounted || busy(from) || busy(to) {
            return false;
        }
        if Self::should_fail(&mut k, "rename", from) {
            return false;
        }
        let Some(node) = s.nodes.remove(from) else {
            return false;
        };
        s.nodes.insert(to.to_string(), node);
        k.renames += 1;
        self.journal.note(format!("fs rename {from} {to}"));
        true
    }
    fn list(&self, dir: &str, visit: &mut dyn FnMut(&FsEntry) -> bool) {
        let entries: Vec<(String, FsNode)> = {
            let s = lock(&self.store);
            if !lock(&self.knobs).mounted {
                return;
            }
            let prefix = if dir == "/" {
                "/".to_string()
            } else {
                format!("{dir}/")
            };
            s.nodes
                .iter()
                .filter_map(|(p, n)| {
                    let rest = p.strip_prefix(&prefix)?;
                    (!rest.is_empty() && !rest.contains('/')).then(|| (rest.to_string(), n.clone()))
                })
                .collect()
        };
        for (name, node) in entries {
            let e = FsEntry {
                name: name.as_bytes(),
                size: if node.dir { 0 } else { node.data.len() as u32 },
                dir: node.dir,
            };
            if !visit(&e) {
                break;
            }
        }
    }
    fn usage(&self) -> (u32, u32) {
        let s = lock(&self.store);
        let k = lock(&self.knobs);
        if !k.mounted {
            return (0, 0);
        }
        (k.total_bytes, self.used(&s, &k))
    }
}

/// An open file of [`FakeFs`].
pub(crate) struct FakeFile {
    fs: FakeFs,
    path: String,
    mode: OpenMode,
    pos: usize,
}

impl Drop for FakeFile {
    fn drop(&mut self) {
        let mut k = lock(&self.fs.knobs);
        if let Some(n) = k.open.get_mut(&self.path) {
            *n -= 1;
            if *n == 0 {
                k.open.remove(&self.path);
            }
        }
    }
}

impl FsFile for FakeFile {
    fn read(&mut self, out: &mut [u8]) -> usize {
        let s = lock(&self.fs.store);
        let mut k = lock(&self.fs.knobs);
        if self.mode != OpenMode::Read || FakeFs::should_fail(&mut k, "read", &self.path) {
            return 0;
        }
        let Some(node) = s.nodes.get(&self.path) else {
            return 0;
        };
        let rest = node.data.get(self.pos..).unwrap_or(&[]);
        let n = rest.len().min(out.len());
        out[..n].copy_from_slice(&rest[..n]);
        self.pos += n;
        n
    }
    fn write(&mut self, data: &[u8]) -> usize {
        let mut s = lock(&self.fs.store);
        let mut k = lock(&self.fs.knobs);
        if self.mode == OpenMode::Read || data.is_empty() {
            return 0;
        }
        if FakeFs::should_fail(&mut k, "write", &self.path) {
            return 0;
        }
        let len_now = s.nodes.get(&self.path).map_or(0, |n| n.data.len());
        if self.mode == OpenMode::Append {
            self.pos = len_now;
        }
        // the blocks of this file may grow as far as the free space allows
        let own = blocks(len_now, k.block_size) * k.block_size;
        let others = self.fs.used(&s, &k) - own;
        let room = k.total_bytes.saturating_sub(others);
        let max_size = (room / k.block_size * k.block_size) as usize;
        let n = data.len().min(max_size.saturating_sub(self.pos));
        if n == 0 {
            return 0;
        }
        let node = s.nodes.entry(self.path.clone()).or_default();
        if self.pos + n > node.data.len() {
            node.data.resize(self.pos + n, 0);
        }
        node.data[self.pos..self.pos + n].copy_from_slice(&data[..n]);
        self.pos += n;
        k.writes += 1;
        k.bytes_written += n as u64;
        n
    }
    fn seek(&mut self, pos: u32) -> bool {
        let s = lock(&self.fs.store);
        let mut k = lock(&self.fs.knobs);
        if FakeFs::should_fail(&mut k, "seek", &self.path) {
            return false;
        }
        let len = s.nodes.get(&self.path).map_or(0, |n| n.data.len());
        if pos as usize > len {
            return false;
        }
        self.pos = pos as usize;
        true
    }
    fn size(&self) -> u32 {
        lock(&self.fs.store)
            .nodes
            .get(&self.path)
            .map_or(0, |n| n.data.len() as u32)
    }
}
