#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::shm_alloc_page;

/// Verify that the cross-ring reserved pages (indices 16096..16100)
/// are NOT allocatable through Step5's shm_alloc_page().
/// Allocating many consecutive pages should never return an index
/// in the reserved range.
#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Step 3↔5 Integration Test ===");
    let mut failures = 0;

    // Allocate N pages — the allocator returns indices from the freelist.
    // The reserved pages (16096..=16100) should never appear.
    const N: usize = 500;
    for i in 0..N {
        let idx = shm_alloc_page();
        if idx < 0 {
            println!("  OOM at iteration {}", i);
            break;
        }
        let idx = idx as usize;
        if (16096..16101).contains(&idx) {
            println!("  FAIL: allocated reserved page idx={}", idx);
            failures += 1;
        }
    }

    if failures == 0 {
        println!("  ✓ No reserved pages were allocated ({} pages checked)", N);
    }

    // Free them all (best-effort; GC handles the rest)
    // Not strictly needed since GC collect can clean up later.

    if failures == 0 {
        println!("=== Integration test passed ===");
        0
    } else {
        println!("=== Integration test FAILED ===");
        1
    }
}
