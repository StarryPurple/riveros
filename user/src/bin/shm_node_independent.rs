#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{shm_alloc_page, shm_free_page, shm_gc_collect};

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Node 2: Independent ===");

    // Allocate 3 pages independently, then free 2
    let mut pages = [0usize; 3];
    for (i, p) in pages.iter_mut().enumerate() {
        match shm_alloc_page() {
            idx if idx >= 0 => *p = idx as usize,
            _ => { println!("  OOM at iter {}", i); break; }
        }
    }
    println!("  allocated {:?}", &pages[..3]);

    // Free 2 of them
    shm_free_page(pages[0]);
    println!("  freed page {}", pages[0]);
    shm_free_page(pages[1]);
    println!("  freed page {}", pages[1]);

    // GC
    let freed = shm_gc_collect();
    println!("  gc_collect freed {} pages", freed);

    // Keep page 2 (don't free)
    println!("  page {} intentionally leaked (test verification)", pages[2]);

    // Verify allocator still works
    let check = shm_alloc_page();
    if check >= 0 {
        shm_free_page(check as usize);
        println!("  PASS: allocator functional");
        0
    } else {
        println!("  FAIL: allocator broken");
        1
    }
}
