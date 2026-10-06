//! Fake NVS: typed entries per namespace (persistent, owned by the board) and the failure knobs
//! of a boot.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use super::{lock, Journal};
use crate::port::{Nvs, NvsInt, NvsNamespace};

/// Type of an NVS entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NvsType {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    Str,
    Blob,
}

impl NvsType {
    fn of(kind: NvsInt) -> NvsType {
        match kind {
            NvsInt::U8 => NvsType::U8,
            NvsInt::I8 => NvsType::I8,
            NvsInt::U16 => NvsType::U16,
            NvsInt::I16 => NvsType::I16,
            NvsInt::U32 => NvsType::U32,
            NvsInt::I32 => NvsType::I32,
            NvsInt::I64 => NvsType::I64,
        }
    }
    fn int_bytes(self) -> usize {
        match self {
            NvsType::U8 | NvsType::I8 => 1,
            NvsType::U16 | NvsType::I16 => 2,
            NvsType::U32 | NvsType::I32 => 4,
            NvsType::U64 | NvsType::I64 => 8,
            NvsType::Str | NvsType::Blob => 0,
        }
    }
    fn signed(self) -> bool {
        matches!(
            self,
            NvsType::I8 | NvsType::I16 | NvsType::I32 | NvsType::I64
        )
    }
}

/// One entry: integers little endian, strings with their NUL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NvsEntry {
    pub(crate) ty: NvsType,
    pub(crate) bytes: Vec<u8>,
}

impl NvsEntry {
    fn int(&self) -> Option<i64> {
        let n = self.ty.int_bytes();
        if n == 0 || self.bytes.len() != n {
            return None;
        }
        let mut raw = [0u8; 8];
        raw[..n].copy_from_slice(&self.bytes);
        let u = u64::from_le_bytes(raw);
        Some(if self.ty.signed() {
            let shift = 64 - 8 * n as u32;
            ((u << shift) as i64) >> shift
        } else {
            u as i64
        })
    }
}

/// Persistent content: namespace -> key -> entry.
pub(crate) type NvsStore = BTreeMap<String, BTreeMap<String, NvsEntry>>;

/// Failure knobs and wear counters of one boot.
#[derive(Default)]
pub(crate) struct NvsKnobs {
    /// Every open fails (nvs_flash_init failed).
    pub(crate) init_failed: bool,
    /// Namespaces whose open fails.
    pub(crate) fail_open: BTreeSet<String>,
    /// Keys whose set fails (NOT_ENOUGH_SPACE): nothing is stored.
    pub(crate) fail_set: BTreeSet<String>,
    /// Every set, remove and erase fails at its commit: nothing changes.
    pub(crate) fail_commit: bool,
    /// `erase_all` fails.
    pub(crate) fail_erase: bool,
    /// Successful sets per "ns/key".
    pub(crate) sets: BTreeMap<String, u32>,
    /// Opens so far and handles open now.
    pub(crate) opens: u32,
    pub(crate) open_handles: u32,
}

/// The NVS partition of a boot: the board's store with this boot's knobs.
#[derive(Clone)]
pub(crate) struct FakeNvs {
    store: Arc<Mutex<NvsStore>>,
    knobs: Arc<Mutex<NvsKnobs>>,
    journal: Journal,
}

/// The largest key NVS takes (15 characters).
const KEY_MAX: usize = 15;

impl FakeNvs {
    pub(crate) fn new(store: Arc<Mutex<NvsStore>>, journal: Journal) -> Self {
        FakeNvs {
            store,
            knobs: Arc::default(),
            journal,
        }
    }
    /// The knobs of this boot.
    pub(crate) fn knobs(&self) -> std::sync::MutexGuard<'_, NvsKnobs> {
        lock(&self.knobs)
    }
    /// Stores an entry directly (test setup).
    pub(crate) fn put(&self, ns: &str, key: &str, ty: NvsType, bytes: &[u8]) {
        lock(&self.store).entry(ns.to_string()).or_default().insert(
            key.to_string(),
            NvsEntry {
                ty,
                bytes: bytes.to_vec(),
            },
        );
    }
    pub(crate) fn set_u8(&self, ns: &str, key: &str, v: u8) {
        self.put(ns, key, NvsType::U8, &[v]);
    }
    pub(crate) fn set_i8(&self, ns: &str, key: &str, v: i8) {
        self.put(ns, key, NvsType::I8, &v.to_le_bytes());
    }
    pub(crate) fn set_u16(&self, ns: &str, key: &str, v: u16) {
        self.put(ns, key, NvsType::U16, &v.to_le_bytes());
    }
    pub(crate) fn set_i16(&self, ns: &str, key: &str, v: i16) {
        self.put(ns, key, NvsType::I16, &v.to_le_bytes());
    }
    pub(crate) fn set_u32(&self, ns: &str, key: &str, v: u32) {
        self.put(ns, key, NvsType::U32, &v.to_le_bytes());
    }
    pub(crate) fn set_i32(&self, ns: &str, key: &str, v: i32) {
        self.put(ns, key, NvsType::I32, &v.to_le_bytes());
    }
    pub(crate) fn set_u64(&self, ns: &str, key: &str, v: u64) {
        self.put(ns, key, NvsType::U64, &v.to_le_bytes());
    }
    pub(crate) fn set_i64(&self, ns: &str, key: &str, v: i64) {
        self.put(ns, key, NvsType::I64, &v.to_le_bytes());
    }
    pub(crate) fn set_str(&self, ns: &str, key: &str, v: &[u8]) {
        let mut b = v.to_vec();
        b.push(0);
        self.put(ns, key, NvsType::Str, &b);
    }
    pub(crate) fn set_blob(&self, ns: &str, key: &str, v: &[u8]) {
        self.put(ns, key, NvsType::Blob, v);
    }
    /// The entry, if any.
    pub(crate) fn get(&self, ns: &str, key: &str) -> Option<NvsEntry> {
        lock(&self.store).get(ns)?.get(key).cloned()
    }
    pub(crate) fn has(&self, ns: &str, key: &str) -> bool {
        self.get(ns, key).is_some()
    }
    /// Integer value of any integer type; 0 when missing.
    pub(crate) fn get_i(&self, ns: &str, key: &str) -> i64 {
        self.get(ns, key).and_then(|e| e.int()).unwrap_or(0)
    }
    /// Bytes of a blob; empty when missing.
    pub(crate) fn get_blob(&self, ns: &str, key: &str) -> Vec<u8> {
        self.get(ns, key).map(|e| e.bytes).unwrap_or_default()
    }
    /// Keys of a namespace in order.
    pub(crate) fn keys(&self, ns: &str) -> Vec<String> {
        lock(&self.store)
            .get(ns)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }
}

impl Nvs for FakeNvs {
    type Ns = FakeNs;
    fn open(&self, namespace: &str, write: bool) -> Option<FakeNs> {
        let mut k = lock(&self.knobs);
        k.opens += 1;
        if k.init_failed || k.fail_open.contains(namespace) {
            return None;
        }
        {
            let mut store = lock(&self.store);
            if !store.contains_key(namespace) {
                if !write {
                    return None; // ESP_ERR_NVS_NOT_FOUND
                }
                store.insert(namespace.to_string(), BTreeMap::new());
            }
        }
        k.open_handles += 1;
        Some(FakeNs {
            nvs: self.clone(),
            ns: namespace.to_string(),
            write,
        })
    }
}

/// An open namespace of [`FakeNvs`].
pub(crate) struct FakeNs {
    nvs: FakeNvs,
    ns: String,
    write: bool,
}

impl Drop for FakeNs {
    fn drop(&mut self) {
        lock(&self.nvs.knobs).open_handles -= 1;
    }
}

impl FakeNs {
    fn entry(&self, key: &str) -> Option<NvsEntry> {
        self.nvs.get(&self.ns, key)
    }
    /// The write steps of `nvs_set_*` + `nvs_commit`; false leaves the store unchanged.
    fn store(&self, key: &str, entry: NvsEntry) -> bool {
        let mut k = lock(&self.nvs.knobs);
        if !self.write || key.is_empty() || key.len() > KEY_MAX {
            return false;
        }
        if k.fail_set.contains(key) || k.fail_commit {
            return false;
        }
        lock(&self.nvs.store)
            .entry(self.ns.clone())
            .or_default()
            .insert(key.to_string(), entry);
        *k.sets.entry(format!("{}/{}", self.ns, key)).or_default() += 1;
        self.nvs
            .journal
            .note(format!("nvs set {}/{}", self.ns, key));
        true
    }
}

impl NvsNamespace for FakeNs {
    fn get_int(&self, key: &str, kind: NvsInt) -> Option<i64> {
        let e = self.entry(key)?;
        if e.ty != NvsType::of(kind) {
            return None;
        }
        e.int()
    }
    fn set_int(&mut self, key: &str, kind: NvsInt, v: i64) -> bool {
        let ty = NvsType::of(kind);
        let bytes = v.to_le_bytes()[..ty.int_bytes()].to_vec();
        self.store(key, NvsEntry { ty, bytes })
    }
    fn blob_len(&self, key: &str) -> Option<usize> {
        let e = self.entry(key)?;
        (e.ty == NvsType::Blob).then_some(e.bytes.len())
    }
    fn get_blob(&self, key: &str, out: &mut [u8]) -> Option<usize> {
        let e = self.entry(key)?;
        if e.ty != NvsType::Blob || e.bytes.len() > out.len() {
            return None;
        }
        out[..e.bytes.len()].copy_from_slice(&e.bytes);
        Some(e.bytes.len())
    }
    fn set_blob(&mut self, key: &str, data: &[u8]) -> bool {
        self.store(
            key,
            NvsEntry {
                ty: NvsType::Blob,
                bytes: data.to_vec(),
            },
        )
    }
    fn str_len(&self, key: &str) -> Option<usize> {
        let e = self.entry(key)?;
        (e.ty == NvsType::Str).then_some(e.bytes.len())
    }
    fn get_str(&self, key: &str, out: &mut [u8]) -> Option<usize> {
        let e = self.entry(key)?;
        let n = e.bytes.len().checked_sub(1)?;
        if e.ty != NvsType::Str || n > out.len() {
            return None;
        }
        out[..n].copy_from_slice(&e.bytes[..n]);
        Some(n)
    }
    fn remove(&mut self, key: &str) -> bool {
        let k = lock(&self.nvs.knobs);
        if !self.write || k.fail_commit {
            return false;
        }
        let removed = lock(&self.nvs.store)
            .get_mut(&self.ns)
            .and_then(|m| m.remove(key))
            .is_some();
        if removed {
            self.nvs
                .journal
                .note(format!("nvs remove {}/{}", self.ns, key));
        }
        removed
    }
    fn erase_all(&mut self) -> bool {
        let k = lock(&self.nvs.knobs);
        if !self.write || k.fail_commit || k.fail_erase {
            return false;
        }
        if let Some(m) = lock(&self.nvs.store).get_mut(&self.ns) {
            m.clear();
        }
        self.nvs.journal.note(format!("nvs erase {}", self.ns));
        true
    }
}
