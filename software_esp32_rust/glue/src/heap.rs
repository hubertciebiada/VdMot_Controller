//! Boot boxes and fallible heap blocks (C++ `boot_alloc.h`; docs/rust/GLUE-DESIGN-ESP.md 2.4).
//!
//! Long-lived blocks are allocated once at boot and never freed ([`boot_block`]); out of memory
//! there aborts, as the C++ `bootAlloc()` did (a reset). Bluetooth is off
//! (`CONFIG_BT_ENABLED=n`), so unlike the C++ no controller DRAM has to be released first.
//! After boot every allocation is a transient block that may fail: the [`HeapGate`] grants it
//! (the test fake scripts refusals and records the sizes) and `try_reserve_exact` takes it, so a
//! failure gives the C++ out-of-memory outcome (the `new (std::nothrow)` that returned nullptr).

use core::ops::{Deref, DerefMut};

use crate::port::HeapGate;

/// A heap block that holds one `T` (a discovery context, a profile copy, a working set part).
pub struct Block<T>(Box<[T; 1]>);

impl<T> Deref for Block<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0[0]
    }
}

impl<T> DerefMut for Block<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0[0]
    }
}

/// Allocates a long-lived block at boot; out of memory aborts (a reset), as the C++
/// `bootAlloc()`. `make` builds the value (it may pass through the stack once).
pub fn boot_block<T>(make: impl FnOnce() -> T) -> Block<T> {
    Block(Box::new([make()]))
}

/// A transient block after boot: `None` when the gate refuses `size_of::<T>()` bytes or the heap
/// has no such block; `make` runs only after the allocation succeeded.
pub fn try_block<T>(gate: &impl HeapGate, make: impl FnOnce() -> T) -> Option<Block<T>> {
    if !gate.grant(core::mem::size_of::<T>()) {
        return None;
    }
    let mut v: Vec<T> = Vec::new();
    v.try_reserve_exact(1).ok()?;
    v.push(make());
    let b: Box<[T; 1]> = v.into_boxed_slice().try_into().ok()?;
    Some(Block(b))
}

/// A transient buffer of `len` zero bytes (a JSON body, the config blob buffers, a chunk
/// buffer): `None` when the gate refuses `len` bytes or the heap has no such block.
pub fn try_bytes(gate: &impl HeapGate, len: usize) -> Option<Vec<u8>> {
    if !gate.grant(len) {
        return None;
    }
    let mut v = Vec::new();
    v.try_reserve_exact(len).ok()?;
    v.resize(len, 0);
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::FakeHeap;

    #[derive(Debug, PartialEq)]
    struct Ctx {
        a: u32,
        b: [u8; 60],
    }

    #[test]
    fn boot_block_holds_the_value() {
        let mut b = boot_block(|| Ctx { a: 7, b: [1; 60] });
        assert_eq!(b.a, 7);
        b.a = 9;
        assert_eq!(*b, Ctx { a: 9, b: [1; 60] });
    }

    #[test]
    fn try_block_asks_the_gate_for_the_size_and_builds_only_after() {
        let heap = FakeHeap::default();
        heap.state().next.extend([false, true]);
        let mut built = 0;
        assert!(try_block(&heap, || {
            built += 1;
            Ctx { a: 1, b: [0; 60] }
        })
        .is_none());
        assert_eq!(built, 0);
        let mut b = try_block(&heap, || Ctx { a: 2, b: [3; 60] }).unwrap();
        b.b[59] = 4;
        assert_eq!((b.a, b.b[0], b.b[59]), (2, 3, 4));
        assert_eq!(heap.state().refused, vec![64]);
        assert_eq!(heap.state().granted, vec![64]);
    }

    #[test]
    fn try_bytes_gives_zeroed_buffers_of_the_size_or_none() {
        let heap = FakeHeap::default();
        let v = try_bytes(&heap, 8192).unwrap();
        assert_eq!(v.len(), 8192);
        assert!(v.iter().all(|&b| b == 0));
        assert_eq!(try_bytes(&heap, 0).unwrap().len(), 0);
        heap.state().fail_all = true;
        assert!(try_bytes(&heap, 16).is_none());
        heap.state().fail_all = false;
        // the gate grants, the heap has no such block
        assert!(try_bytes(&heap, usize::MAX).is_none());
        assert_eq!(heap.state().granted, vec![8192, 0, usize::MAX]);
        assert_eq!(heap.state().refused, vec![16]);
    }
}
