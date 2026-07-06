#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_pop, cxl_rx_push, shm_ref_page, shm_free_page, shm_gc_collect, msg_seal, msg_verify};

const ROUNDS: usize = 12;

fn make_msg(idx: usize, tag: u8) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0..4].copy_from_slice(&(idx as u32).to_le_bytes());
    m[4] = tag;
    msg_seal(&mut m, 1);
    m
}

fn read_msg(msg: &[u8; 60]) -> (usize, u8) {
    assert!(msg_verify(msg).is_some(), "checksum");
    let idx = u32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]) as usize;
    (idx, msg[4])
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Node 1: Roundtrip Stress ({} rounds) ===", ROUNDS);

    for r in 0..ROUNDS {
        let mut msg = [0u8; 60];
        loop {
            if cxl_tx_pop(&mut msg) == 0 {
                let (idx, kind) = read_msg(&msg);
                if kind == 0 {
                    shm_ref_page(idx);
                    println!("  [r{}] got page {} -> ref'd", r, idx);
                    let ack = make_msg(idx, 1);
                    while cxl_rx_push(&ack) != 0 {}
                    println!("  [r{}] acked page {}", r, idx);

                    shm_free_page(idx);
                    println!("  [r{}] freed page {} (refcnt->0)", r, idx);
                    break;
                }
            }
        }
    }

    shm_gc_collect();
    println!("=== Node 1 done ===");
    0
}
