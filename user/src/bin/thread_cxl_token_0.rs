#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_push, cxl_rx_pop, shm_alloc_page, shm_free_page, shm_gc_collect, get_time};

const TOTAL_TOKENS: usize = 20;

fn make_msg(tag: u8, val: usize) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0] = tag;
    m[1..9].copy_from_slice(&(val as u64).to_le_bytes());
    m
}
fn read_msg(m: &[u8; 60]) -> (u8, usize) {
    let mut b = [0u8; 8]; b.copy_from_slice(&m[1..9]);
    (m[0], u64::from_le_bytes(b) as usize)
}

// Node0: Token Server
// RX ring (Client->Server): requests come in here
// TX ring (Server->Client): responses go out here
#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== CXL Token Bucket — Server (Node 0) ===");
    println!("  pool: {} pages", TOTAL_TOKENS);

    let mut pool = [0usize; TOTAL_TOKENS];
    for i in 0..TOTAL_TOKENS {
        let p = shm_alloc_page();
        assert!(p >= 0, "alloc token {} failed", i);
        pool[i] = p as usize;
    }
    let mut available = TOTAL_TOKENS;
    let mut granted = 0usize;
    let mut returned = 0usize;
    println!("  pool ready, {} tokens available", available);

    let t0 = get_time();
    loop {
        let mut msg = [0u8; 60];
        if cxl_rx_pop(&mut msg) == 0 {       // Client -> Server (RX ring)
            let (tag, val) = read_msg(&msg);
            match tag {
                1 => { // REQUEST: client wants val tokens
                    let grant = val.min(available);
                    for _ in 0..grant {
                        available -= 1;
                        let reply = make_msg(2, pool[available]);
                        while cxl_tx_push(&reply) != 0 {} // Server -> Client (TX ring)
                    }
                    let done = make_msg(3, grant);
                    while cxl_tx_push(&done) != 0 {}
                    granted += grant;
                }
                4 => { // RETURN
                    pool[available] = val;
                    available += 1;
                    returned += 1;
                }
                9 => { // DONE
                    println!("  client done: granted={} returned={}", granted, returned);
                    while cxl_tx_push(&make_msg(9, 0)) != 0 {}
                    break;
                }
                _ => {}
            }
        }
    }
    let ms = get_time() - t0;

    for i in 0..available {
        shm_free_page(pool[i]);
    }
    shm_gc_collect();
    println!("  reclaimed {} remaining, done in {} ms", available, ms);

    let check = shm_alloc_page();
    let ok = check >= 0;
    if ok { shm_free_page(check as usize); shm_gc_collect(); }
    println!("  server: {}", if ok { "PASS" } else { "FAIL" });
    if ok { 0 } else { 1 }
}
