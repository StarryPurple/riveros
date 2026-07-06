#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_pop, cxl_rx_push,
               shm_alloc_page, shm_free_page, shm_gc_collect, msg_seal, msg_verify};

const MEALS: usize = 3;

fn make_msg(tag: u8, val: usize) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0] = tag; m[1..9].copy_from_slice(&(val as u64).to_le_bytes()); m
}
fn read_msg(m: &[u8; 60]) -> (u8, usize) {
    let mut b = [0u8; 8]; b.copy_from_slice(&m[1..9]); (m[0], u64::from_le_bytes(b) as usize)
}

// Node1: 1 philosopher, competes with Node0's philosopher for 2 chopsticks
#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== CXL Dining Philosophers — Node 1 ===");

    shm_gc_collect();

    let mut meals_eaten = 0usize;
    for meal in 0..MEALS {
        // Wait for Node0 to signal this meal round
        let mut sig = [0u8; 60];
        loop {
            if cxl_tx_pop(&mut sig) == 0 {
                if read_msg(&sig).0 == 1 { break; }
            }
        }

        // Try to eat: pick up 2 chopsticks with backoff
        let (c1, c2) = loop {
            let a = shm_alloc_page();
            if a < 0 { continue; }
            let b = shm_alloc_page();
            if b >= 0 { break (a as usize, b as usize); }
            shm_free_page(a as usize);
            shm_gc_collect();
            for _ in 0..50 { core::hint::spin_loop(); }
        };

        println!("  [N1] eating: chopsticks {}/{}", c1, c2);
        for _ in 0..100 { core::hint::spin_loop(); }
        meals_eaten += 1;

        shm_free_page(c1);
        shm_free_page(c2);
        shm_gc_collect();
        println!("  [N1] done, chopsticks released");

        // Signal Node0: I'm done
        while cxl_rx_push(&make_msg(2, meal)) != 0 {}
    }
    println!("  Node 1 eaten {} meals", meals_eaten);

    let ok = meals_eaten == MEALS;
    println!("  philosopher: {}", if ok { "PASS" } else { "FAIL" });
    if ok { 0 } else { 1 }
}
