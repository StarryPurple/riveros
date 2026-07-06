#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{shm_alloc_page, shm_free_page, shm_ref_page, shm_gc_collect};

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== SHM Reference Counting Test ===");
    let mut pass = true;

    // 1. Allocate (refcnt=1, owner=me)
    let p0 = shm_alloc_page();
    assert!(p0 >= 0, "alloc failed: {}", p0);
    let p0 = p0 as usize;
    println!("[1] alloc idx={} (refcnt=1)", p0);

    // 2. Ref by another "instance" (simulated)
    shm_ref_page(p0);
    println!("[2] ref        idx={} (refcnt->2)", p0);

    // 3. Release once (refcnt 2->1, stays allocated)
    shm_free_page(p0);
    println!("[3] free (1/2) idx={} (refcnt->1, still allocated)", p0);

    // 4. Release again (refcnt 1->0 -> GC pending -> GC collect inside free_page)
    shm_free_page(p0);
    println!("[4] free (2/2) idx={} (refcnt->0, GC collected)", p0);

    // 5. Explicit GC collect — should find nothing
    let freed = shm_gc_collect();
    println!("[5] gc_collect: {} pages (0 = already freed in step 4)", freed);

    // 6. Re-allocate — should get our page back
    let p1 = shm_alloc_page();
    assert!(p1 >= 0, "re-alloc failed: {}", p1);
    let p1 = p1 as usize;
    if p1 == p0 {
        println!("[6] re-alloc idx={} (same page -> freed & recycled ✓)", p0);
    } else {
        println!("[6] re-alloc idx={} (different page, freelist had other entries)", p1);
        pass = false;
    }

    // Cleanup
    shm_free_page(p1);
    shm_gc_collect();

    if pass {
        println!("=== All tests passed ===");
        0
    } else {
        println!("=== Some checks failed ===");
        1
    }
}
