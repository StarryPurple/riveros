#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::vec::Vec;
use user_lib::{shm_alloc_page, shm_free_page, thread_create, waittid, exit, yield_};

const N: usize = 200;

fn worker(_arg: usize) -> ! {
    for _ in 0..3 {
        let mut pages = Vec::new();
        for _ in 0..N {
            match shm_alloc_page() {
                idx if idx >= 0 => pages.push(idx as usize),
                _ => break,
            }
            yield_();
        }
        for &p in &pages {
            shm_free_page(p);
        }
    }
    exit(0)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Stress: Concurrent Alloc/Free ===");

    let t1 = thread_create(linker_symbol_addr!(worker), 0);
    let t2 = thread_create(linker_symbol_addr!(worker), 0);
    waittid(t1 as usize);
    waittid(t2 as usize);

    // Freelist should be usable after concurrent work
    let check = shm_alloc_page();
    if check >= 0 {
        shm_free_page(check as usize);
        println!("  PASS: freelist intact after concurrent alloc/free");
        0
    } else {
        println!("  FAIL: freelist corrupted");
        1
    }
}
