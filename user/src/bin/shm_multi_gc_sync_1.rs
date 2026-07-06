#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_pop, cxl_rx_push, shm_ref_page, shm_unref_page, shm_gc_collect, msg_seal, msg_verify};

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
    println!("=== Node 1: GC Sync Test ===");

    // Pre-sync: advance VC[1] so Node0's all_past can later succeed
    shm_gc_collect();

    let mut pages = [0usize; 2];

    for i in 0..2 {
        let mut msg = [0u8; 60];
        loop {
            if cxl_tx_pop(&mut msg) == 0 {
                let (idx, kind) = read_msg(&msg);
                if kind == 0 {
                    shm_ref_page(idx);
                    pages[i] = idx;
                    println!("  got page {} -> ref'd", idx);
                    let ack = make_msg(idx, 1);
                    while cxl_rx_push(&ack) != 0 {}
                    println!("  acked page {}", idx);
                    break;
                }
            }
        }
    }

    // Wait for DONE
    let mut done = [0u8; 60];
    loop {
        if cxl_tx_pop(&mut done) == 0 {
            let (_, kind) = read_msg(&done);
            if kind == 9 { break; }
        }
    }
    println!("  received DONE from Node 0");

    // Release our refs (rc drops by 1 each, owner=Node0 drops the rest)
    shm_unref_page(pages[0]);
    println!("  unref page {} (refcnt -> 2)", pages[0]);
    shm_unref_page(pages[1]);
    println!("  unref page {} (refcnt -> 1)", pages[1]);

    // GC collect to advance VC[1], helping Node0's all_past check
    shm_gc_collect();
    println!("  gc collect done (VC[1] advanced)");

    let done_ack = make_msg(0, 9);
    while cxl_rx_push(&done_ack) != 0 {}
    println!("  sent DONE ack to Node 0");
    println!("=== Node 1 done ===");
    0
}
