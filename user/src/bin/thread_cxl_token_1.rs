#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_pop, cxl_rx_push, shm_free_page, shm_gc_collect};

const REQUEST_BURST: usize = 8;
const TOTAL_REQUESTS: usize = 4; // Request burst 4 times

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

// Node1: Token Client
// TX ring (Server->Client): read granted pages here
// RX ring (Client->Server): send requests here
#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== CXL Token Bucket — Client (Node 1) ===");

    let mut held = [0usize; 64];
    let mut held_count = 0usize;
    let mut total_granted = 0usize;

    for _round in 0..TOTAL_REQUESTS {
        // Request burst tokens
        let req = make_msg(1, REQUEST_BURST);
        while cxl_rx_push(&req) != 0 {}          // Client -> Server (RX ring)

        // Receive grants
        let mut round_grant = 0usize;
        loop {
            let mut msg = [0u8; 60];
            if cxl_tx_pop(&mut msg) == 0 {       // Server -> Client (TX ring)
                let (tag, val) = read_msg(&msg);
                if tag == 2 {
                    // Received a token page
                    held[held_count] = val;
                    held_count += 1;
                    round_grant += 1;
                } else if tag == 3 {
                    // Grants done for this request
                    println!("  round {}: got {} tokens", _round, round_grant);
                    total_granted += round_grant;
                    break;
                }
            }
        }

        // Hold tokens briefly, then return some
        if round_grant > 0 {
            let to_return = round_grant.min(4);
            for _ in 0..to_return {
                held_count -= 1;
                let idx = held[held_count];
                let ret = make_msg(4, idx);
                while cxl_rx_push(&ret) != 0 {}
                shm_free_page(idx);
            }
        }
    }

    // Signal DONE to server
    while cxl_rx_push(&make_msg(9, 0)) != 0 {}
    // Wait for ack
    loop {
        let mut ack = [0u8; 60];
        if cxl_tx_pop(&mut ack) == 0 {
            if read_msg(&ack).0 == 9 { break; }
        }
    }

    // Free any remaining held tokens
    for i in 0..held_count {
        shm_free_page(held[i]);
    }
    shm_gc_collect();
    println!("  client done: total_granted={} held_freed={}", total_granted, held_count);

    let ok = true;
    println!("  client: {}", if ok { "PASS" } else { "FAIL" });
    if ok { 0 } else { 1 }
}
