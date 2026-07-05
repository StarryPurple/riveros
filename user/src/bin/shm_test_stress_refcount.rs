#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{shm_alloc_page, shm_free_page, shm_ref_page, shm_gc_collect};

const REFS: usize = 50;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Stress: RefCount High Count ===");

    let p = shm_alloc_page();
    if p < 0 { panic!("alloc"); }
    let p = p as usize;
    println!("  alloc page idx={} (refcnt=1)", p);

    // Ref N times -> refcnt = 1 + N
    for _ in 0..REFS {
        shm_ref_page(p);
    }
    println!("  ref'd {} times (refcnt={})", REFS, REFS + 1);

    // Free N+1 times -> refcnt -> 0 -> GC pending
    for i in 0..=REFS {
        shm_free_page(p);
    }
    println!("  freed {} times (refcnt -> 0)", REFS + 1);

    let freed = shm_gc_collect();
    println!("  gc_collect freed {} pages", freed);

    // Verify alloc still works
    let p2 = shm_alloc_page();
    if p2 >= 0 {
        println!("  PASS: re-alloc after high refcount works");
        0
    } else {
        println!("  FAIL: re-alloc failed");
        1
    }
}
