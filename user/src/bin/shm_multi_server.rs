#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_push, cxl_rx_pop, shm_alloc_page, shm_free_page, shm_gc_collect};

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
    println!("=== Multi-QEMU Server (Instance 0) ===");
    println!("Waiting for client on ring buffer...");

    for i in 0..ITERS {
        let p = shm_alloc_page();
        if p < 0 { panic!("alloc failed"); }
        let p = p as usize;
        println!("[server] alloc page idx={} (iter {})", p, i);

        // Send page index via Ring 0 (Server->Client)
        let msg = make_msg(p, 0);
        while cxl_tx_push(&msg) != 0 {}
        println!("[server] sent idx {} -> ring 0", p);

        // Wait for ack via Ring 1 (Client->Server)
        let mut reply = [0u8; 60];
        loop {
            if cxl_rx_pop(&mut reply) == 0 {
                let (ack_idx, kind) = read_idx(&reply);
                if kind == 1 && ack_idx == p {
                    println!("[server] received ack for idx={}", p);
                    break;
                }
            }
        }

        // Free (client ref'd it, so refcnt 2->1 -> still allocated)
        shm_free_page(p);
        println!("[server] free page idx={} (refcnt still 1)", p);
    }

    println!("Server done. GC collect...");
    shm_gc_collect();
    println!("=== Server finished ===");
    0
}
