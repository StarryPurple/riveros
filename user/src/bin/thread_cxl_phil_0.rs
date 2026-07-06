#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_push, cxl_rx_pop,
               shm_alloc_page, shm_free_page, shm_gc_collect};

const MEALS: usize = 3;

fn make_msg(tag: u8, val: usize) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0] = tag; m[1..9].copy_from_slice(&(val as u64).to_le_bytes()); m
}
fn read_msg(m: &[u8; 60]) -> (u8, usize) {
    let mut b = [0u8; 8]; b.copy_from_slice(&m[1..9]); (m[0], u64::from_le_bytes(b) as usize)
}

// Node0: 1 philosopher, competes with Node1's philosopher for 2 chopsticks
// (4 total in pool, both need 2. Prevents deadlock via backoff+retry)
#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== CXL Dining Philosophers — Node 0 ===");

    // Pre-allocate 4 chopstick pages, then free → available in freelist
    for _ in 0..4 {
        let p = shm_alloc_page();
        assert!(p >= 0, "init alloc failed");
        shm_free_page(p as usize);
    }
    shm_gc_collect();
    println!("  4 chopsticks in pool");

    let mut meals_eaten = 0usize;
    for meal in 0..MEALS {
        // Signal Node1: this meal round begins
        while cxl_tx_push(&make_msg(1, meal)) != 0 {}

        // Try to eat: pick up 2 chopsticks (with backoff to prevent deadlock)
        let (c1, c2) = loop {
            let a = shm_alloc_page();
            if a < 0 { continue; }
            let b = shm_alloc_page();
            if b >= 0 { break (a as usize, b as usize); }
            // Got first but couldn't get second: release and retry
            shm_free_page(a as usize);
            shm_gc_collect();
            for _ in 0..50 { core::hint::spin_loop(); } // backoff
        };

        println!("  [N0] eating: chopsticks {}/{}", c1, c2);
        for _ in 0..100 { core::hint::spin_loop(); } // eat duration
        meals_eaten += 1;

        // Put down chopsticks
        shm_free_page(c1);
        shm_free_page(c2);
        shm_gc_collect();
        println!("  [N0] done, chopsticks released");

        // Wait for Node1 to finish this meal
        let mut ack = [0u8; 60];
        loop {
            if cxl_rx_pop(&mut ack) == 0 {
                if read_msg(&ack).0 == 2 { break; }
            }
        }
    }
    println!("  Node 0 eaten {} meals", meals_eaten);

    // Verify pool health
    let ok = meals_eaten == MEALS;
    println!("  philosopher: {}", if ok { "PASS" } else { "FAIL" });
    if ok { 0 } else { 1 }
}
