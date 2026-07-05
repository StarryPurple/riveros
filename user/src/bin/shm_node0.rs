#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_push, cxl_rx_pop, shm_alloc_page, shm_free_page, shm_ref_page, shm_gc_collect};

fn make_msg(idx: usize, tag: u8) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0..4].copy_from_slice(&(idx as u32).to_le_bytes());
    m[4] = tag;
    m
}
fn read_msg(msg: &[u8; 60]) -> (usize, u8) {
    let idx = u32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]) as usize;
    (idx, msg[4])
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Node 0: Coordinator ===");

    // Allocate 2 pages
    let a = { let r = shm_alloc_page(); if r < 0 { panic!("alloc a"); } r as usize };
    let b = { let r = shm_alloc_page(); if r < 0 { panic!("alloc b"); } r as usize };
    println!("  alloc a={} b={}", a, b);

    // Send a to Node 1, wait for ack
    let msg = make_msg(a, 0);
    while cxl_tx_push(&msg) != 0 {}
    println!("  sent a={} to Node 1", a);
    let mut reply = [0u8; 60];
    loop {
        if cxl_rx_pop(&mut reply) == 0 {
            let (idx, kind) = read_msg(&reply);
            if kind == 1 && idx == a { break; }
        }
    }
    println!("  got ack for a={}", a);

    // Send b to Node 1, wait for ack
    let msg = make_msg(b, 0);
    while cxl_tx_push(&msg) != 0 {}
    println!("  sent b={} to Node 1", b);
    loop {
        if cxl_rx_pop(&mut reply) == 0 {
            let (idx, kind) = read_msg(&reply);
            if kind == 1 && idx == b { break; }
        }
    }
    println!("  got ack for b={}", b);

    // Free both (refcnt from 2 to 1 — not freed, Node 1 still holds)
    shm_free_page(a);
    shm_free_page(b);
    println!("  freed a b (refcnt -> 1, held by Node 1)");

    // Wait for Node 2 to also check (just sleep a bit)
    let freed = shm_gc_collect();
    println!("  gc_collect freed {} pages", freed);
    println!("=== Node 0 done ===");
    0
}
