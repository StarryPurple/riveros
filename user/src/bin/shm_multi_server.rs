#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_push, cxl_rx_pop, shm_alloc_page, shm_free_page, shm_gc_collect};

const ITERS: usize = 5;
const TAG_PAGE: u8 = 0;
const TAG_ACK:  u8 = 1;
const TAG_DONE: u8 = 9;

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
    println!("=== Multi-QEMU Server (Instance 0) ===");
    println!("Waiting for client on ring buffer...");

    shm_gc_collect();

    for i in 0..ITERS {
        let p = shm_alloc_page();
        if p < 0 { panic!("alloc failed"); }
        let p = p as usize;
        println!("[server] alloc page idx={} (iter {})", p, i);

        let msg = make_msg(p, TAG_PAGE);
        while cxl_tx_push(&msg) != 0 {}
        println!("[server] sent idx {} -> ring 0", p);

        let mut reply = [0u8; 60];
        loop {
            if cxl_rx_pop(&mut reply) == 0 {
                let (ack_idx, kind) = read_idx(&reply);
                if kind == TAG_ACK && ack_idx == p {
                    println!("[server] received ack for idx={}", p);
                    break;
                }
            }
        }

        shm_free_page(p);
        println!("[server] free page idx={} (refcnt still 1)", p);
    }

    // Tell client all pages have been sent; it should now release its refs
    let done = make_msg(0, TAG_DONE);
    while cxl_tx_push(&done) != 0 {}
    println!("[server] sent DONE to client");

    // Wait for client to finish cleanup
    let mut ack = [0u8; 60];
    loop {
        if cxl_rx_pop(&mut ack) == 0 {
            let (_, kind) = read_idx(&ack);
            if kind == TAG_DONE { break; }
        }
    }
    println!("[server] client finished cleanup");

    // GC collect — client pushed ref->0 entries, our VC advances to free them
    let freed = shm_gc_collect();
    println!("[server] gc_collect freed {} pages", freed);

    let verify = shm_alloc_page();
    let ok = verify >= 0;
    if ok {
        shm_free_page(verify as usize);
        shm_gc_collect();
    }
    println!("[server] allocator: {}", if ok { "PASS" } else { "FAIL" });
    println!("=== Server finished ===");
    if ok { 0 } else { 1 }
}
