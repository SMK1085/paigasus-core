// SPDX-License-Identifier: Apache-2.0

//! SMA-700 R8: the heap bytes per entry of `InMemoryReplayStore` at the default capacity
//! (200 000). A measurement, not a gate: `#[ignore]`, run by hand with
//! `cargo nextest run -p paigasus-iam --test dpop_replay_memory --run-ignored only --no-capture`.
//! Its own binary, because the counting allocator below applies to the whole binary.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

use paigasus_iam::adapters::dpop_replay::InMemoryReplayStore;
use paigasus_iam_core::{NewProof, ProofKey, RecordOutcome, ReplayStore};

/// Counts the live heap bytes of this test binary.
struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_add(new_size as isize - layout.size() as isize, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn id16(n: u64) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&n.to_le_bytes());
    out
}

/// Fills a store with `N` entries and prints the heap bytes per entry. `jkts` and `subjects` are
/// the number of distinct values; `N` means every entry has its own.
fn measure(label: &str, jkts: u64, subjects: u64) -> isize {
    const N: usize = 200_000;
    let store = InMemoryReplayStore::new(N, N, N);
    let before = LIVE.load(Ordering::SeqCst);
    for i in 0..N as u64 {
        // Removal seconds spread over 120 s: the default window shape.
        let remove_at = 1_000 + (i % 120) as i64;
        let entry = NewProof {
            key: ProofKey(id16(i)),
            subject: id16(1_000_000 + i % subjects),
            jkt: id16(2_000_000 + i % jkts),
            expires_at: remove_at,
            follow_up_deadline: remove_at,
            follow_up_digest: [0u8; 32],
        };
        assert_eq!(store.record(entry, 1_000), RecordOutcome::Fresh);
    }
    let bytes = LIVE.load(Ordering::SeqCst) - before;
    let per_entry = bytes / N as isize;
    println!("SMA-700 R8 [{label}]: {N} entries use {bytes} heap bytes, {per_entry} bytes per entry, {} MiB", bytes / (1024 * 1024));
    assert!(per_entry > 0);
    drop(store);
    per_entry
}

#[test]
#[ignore = "SMA-700 R8 measurement; run by hand with --run-ignored only"]
fn bytes_per_entry_at_the_default_capacity() {
    const N: u64 = 200_000;
    measure("brief: 1000 jkts, 500 subjects", 1_000, 500);
    measure("distinct jkt, 500 subjects", N, 500);
    measure("distinct jkt and subject", N, N);
}
