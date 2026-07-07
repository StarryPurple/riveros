#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::vec::Vec;
use user_lib::{shm_alloc_page, shm_free_page, shm_ref_page, shm_gc_collect};

const N: usize = 500;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Stress: GC Batch Collection ===");
    let mut pages = Vec::new();

    // Allocate N pages, each with refcnt>1 so they go to GC pending
    for _ in 0..N {
        match shm_alloc_page() {
            idx if idx >= 0 => pages.push(idx as usize),
            _ => { println!("  OOM at {}", pages.len()); break; }
        }
    }
    println!("  allocated {} pages", pages.len());

    // Ref each once, then free twice -> refcnt hits 0 -> enters GC pending
    for &p in &pages {
        shm_ref_page(p);
        shm_free_page(p);
        shm_free_page(p);
    }

    let freed = shm_gc_collect();
    println!("  gc_collect freed {} pages", freed);

    // Verify: alloc again — should get fresh pages
    let check = shm_alloc_page();
    if check >= 0 {
        shm_free_page(check as usize);
        println!("  PASS: alloc after batch GC works");
        0
    } else {
        println!("  FAIL: alloc after batch GC failed");
        1
    }
}
