#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{shm_alloc_page, shm_free_page, shm_ref_page, shm_gc_collect};

fn check(cond: bool, msg: &str) -> i32 {
    if cond { println!("  PASS: {}", msg); 0 }
    else    { println!("  FAIL: {}", msg); 1 }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== SHM Comprehensive Test ===");
    let mut f = 0;

    // 1. Alloc / Free / Re-alloc
    println!("--- Alloc/Free cycle ---");
    let p0 = shm_alloc_page();
    f += check(p0 >= 0, "alloc_page returns valid idx");
    if p0 >= 0 { shm_free_page(p0 as usize); }
    f += check(shm_gc_collect() >= 0, "gc_collect runs");
    let p1 = shm_alloc_page();
    f += check(p1 >= 0, "re-alloc after free succeeds");
    if p1 >= 0 { shm_free_page(p1 as usize); }

    // 2. Reference counting
    println!("--- Reference counting ---");
    let p3 = shm_alloc_page();
    if p3 < 0 { panic!("alloc"); }
    let p3 = p3 as usize;

    shm_ref_page(p3);
    shm_free_page(p3);  // ref 2->1
    shm_free_page(p3);  // ref 1->0 -> GC pending
    shm_gc_collect();
    let p4 = shm_alloc_page();
    f += check(p4 >= 0, "alloc after ref/free/gc");
    if p4 >= 0 { shm_free_page(p4 as usize); }

    // 3. Multiple pages
    println!("--- Multiple pages ---");
    let mut all_ok = true;
    let mut pages = [0usize; 16];
    let mut count = 0;
    for i in 0..16 {
        let idx = shm_alloc_page();
        if idx < 0 { all_ok = false; break; }
        pages[i] = idx as usize;
        count += 1;
    }
    f += check(all_ok, "allocate 16 pages");
    for i in 0..count { shm_free_page(pages[i]); }
    shm_gc_collect();

    // Result
    println!("--- Summary ---");
    if f == 0 { println!("All tests passed!"); 0 }
    else      { println!("{} test(s) failed.", f); 1 }
}
