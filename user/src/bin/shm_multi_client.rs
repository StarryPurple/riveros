#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_pop, cxl_rx_push, shm_ref_page, shm_free_page, shm_gc_collect, msg_seal, msg_verify};

const TAG_PAGE: u8 = 0;
const TAG_ACK:  u8 = 1;
const TAG_DONE: u8 = 9;

fn make_msg(idx: usize, kind: u8) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0..4].copy_from_slice(&(idx as u32).to_le_bytes());
    m[4] = kind;
    msg_seal(&mut m, 1);
    m
}

fn read_idx(msg: &[u8; 60]) -> (usize, u8) {
    assert!(msg_verify(msg).is_some(), "checksum");
    let idx = u32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]) as usize;
    (idx, msg[4])
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Multi-QEMU Client (Instance 1) ===");
    println!("Waiting for server messages...");

    shm_gc_collect();

    let mut stored = [0usize; 64];
    let mut count = 0usize;

    loop {
        let mut msg = [0u8; 60];
        if cxl_tx_pop(&mut msg) == 0 {
            let (idx, kind) = read_idx(&msg);
            if kind == TAG_PAGE {
                shm_ref_page(idx);
                stored[count] = idx;
                count += 1;
                println!("[client] received+ref page idx={} (total {})", idx, count);

                let ack = make_msg(idx, TAG_ACK);
                while cxl_rx_push(&ack) != 0 {}
                println!("[client] sent ack for idx={}", idx);
            } else if kind == TAG_DONE {
                println!("[client] received DONE ({} pages held)", count);
                break;
            }
        }
    }

    // Release all held pages
    for i in 0..count {
        shm_free_page(stored[i]);
    }
    println!("[client] freed {} pages (refcnt -> 0)", count);

    let freed = shm_gc_collect();
    println!("[client] gc_collect freed {} pages", freed);

    let done_ack = make_msg(0, TAG_DONE);
    while cxl_rx_push(&done_ack) != 0 {}
    println!("[client] sent DONE ack");

    println!("=== Client finished ===");
    0
}
