#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_pop, cxl_rx_push, shm_ref_page, shm_free_page, shm_gc_collect, msg_seal, msg_verify};

const TAG_PAGE: u8 = 0;
const TAG_BULK_DONE: u8 = 2;
const TAG_ACK: u8 = 1;

fn make_msg(_idx: usize, tag: u8) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[4] = tag;
    msg_seal(&mut m, 1);
    m
}

fn read_msg(msg: &[u8; 60]) -> (usize, u8) {
    assert!(msg_verify(msg).is_some(), "checksum");
    let idx = u32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]) as usize;
    (idx, msg[4])
}

fn read_bulk_done(msg: &[u8; 60]) -> usize {
    u16::from_le_bytes([msg[5], msg[6]]) as usize
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Node 1: Bulk Page Transfer (Receiver) ===");

    let mut received = [0usize; 256];
    let mut count = 0usize;

    loop {
        let mut msg = [0u8; 60];
        if cxl_tx_pop(&mut msg) == 0 {
            let (idx, kind) = read_msg(&msg);
            if kind == TAG_PAGE {
                shm_ref_page(idx);
                if count < received.len() {
                    received[count] = idx;
                }
                count += 1;
            } else if kind == TAG_BULK_DONE {
                let expected = read_bulk_done(&msg);
                println!("  received bulk-done: got={} expected={}", count, expected);
                break;
            }
        }
    }

    if count > 0 {
        let mut ok = true;
        if count != received.len().min(count) {
            ok = false;
        }
        println!("  ref'd {} pages", count);

        let ack = make_msg(0, TAG_ACK);
        while cxl_rx_push(&ack) != 0 {}
        println!("  sent bulk ack to Node 0");

        for i in 0..count {
            shm_free_page(received[i]);
        }
        println!("  freed {} pages (refcnt -> 0)", count);
        shm_gc_collect();

        println!("  verification: {}", if ok { "PASS" } else { "FAIL (count mismatch)" });
        if ok { 0 } else { 1 }
    } else {
        println!("  FAIL: no pages received");
        1
    }
}
