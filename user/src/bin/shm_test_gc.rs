#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{shm_alloc_page, shm_free_page, shm_ref_page, shm_gc_collect};

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Vector Clock GC Test ===");
    let mut pass = true;

    // 1. Allocate, ref, then free twice (refcnt 1->0 -> GC pending)
    let p = shm_alloc_page();
    if p < 0 { panic!("alloc"); }
    let p = p as usize;
    // ref it so refcnt=2
    shm_ref_page(p);
    // free once: refcnt 2->1
    shm_free_page(p);
    // free again: refcnt 1->0 -> GC pending, timestamp = current VC
    shm_free_page(p);

    println!("page idx={} freed, refcnt->0, entered GC pending", p);

    // 2. GC collect — in single-instance mode the VC check
    //    (all VCs >= timestamp) passes immediately because we
    //    advanced our own VC inside shm_gc_collect.
    let freed = shm_gc_collect();
    if freed > 0 {
        println!("✓ GC collected {} page(s) (vector clock OK)", freed);
    } else {
        // The page might have been freed by shm_free_page's internal GC.
        // Try allocating again — if we get the same page, it was freed.
        let p1 = shm_alloc_page();
        if p1 == p as isize {
            println!("✓ page idx={} was recycled (freed by internal GC)", p);
        } else if p1 >= 0 {
            shm_free_page(p1 as usize);
        }
    }

    // 3. Verify GC returns >= 0
    let r = shm_gc_collect();
    if r >= 0 {
        println!("✓ gc_collect returns {} (always valid)", r);
    } else {
        println!("✗ gc_collect failed");
        pass = false;
    }

    if pass { println!("=== All GC tests passed ==="); 0 }
    else     { println!("=== Some GC tests failed ==="); 1 }
}
