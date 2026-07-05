#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_ring_push, cxl_ring_pop, shm_ref_page};

const ITERS: usize = 5;

fn make_msg(idx: usize, kind: u8) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0..4].copy_from_slice(&(idx as u32).to_le_bytes());
    m[4] = kind;
    m
}

fn read_idx(msg: &[u8; 60]) -> (usize, u8) {
    let idx = u32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]) as usize;
    (idx, msg[4])
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Multi-QEMU Client (Instance 1) ===");
    println!("Waiting for server messages...");

    for i in 0..ITERS {
        let mut msg = [0u8; 60];
        loop {
            if cxl_ring_pop(&mut msg) == 0 {
                let (idx, kind) = read_idx(&msg);
                if kind == 0 {
                    println!("[client] received page idx={} (iter {})", idx, i);

                    shm_ref_page(idx);
                    println!("[client] ref page idx={} (refcnt++)", idx);

                    let ack = make_msg(idx, 1);
                    while cxl_ring_push(&ack) != 0 {}
                    println!("[client] sent ack for idx={}", idx);
                    break;
                }
            }
        }
    }

    println!("=== Client finished ===");
    0
}
