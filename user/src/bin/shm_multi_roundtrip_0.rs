#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_push, cxl_rx_pop, shm_alloc_page, shm_free_page, shm_gc_collect, msg_seal, msg_verify};

const ROUNDS: usize = 12;

fn make_msg(idx: usize, tag: u8) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0..4].copy_from_slice(&(idx as u32).to_le_bytes());
    m[4] = tag;
    msg_seal(&mut m, 0);
    m
}

fn read_msg(msg: &[u8; 60]) -> (usize, u8) {
    assert!(msg_verify(msg).is_some(), "checksum");
    let idx = u32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]) as usize;
    (idx, msg[4])
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Node 0: Roundtrip Stress ({} rounds) ===", ROUNDS);

    for r in 0..ROUNDS {
        let p = shm_alloc_page();
        assert!(p >= 0, "alloc failed in round {}", r);
        let p = p as usize;

        let msg = make_msg(p, 0);
        while cxl_tx_push(&msg) != 0 {}
        println!("  [r{}] alloc page {} -> sent to Node 1", r, p);

        let mut reply = [0u8; 60];
        loop {
            if cxl_rx_pop(&mut reply) == 0 {
                let (idx, kind) = read_msg(&reply);
                if kind == 1 && idx == p { break; }
            }
        }
        println!("  [r{}] got ack for page {}", r, p);

        shm_free_page(p);
        println!("  [r{}] freed page {} (refcnt->1)", r, p);
    }

    let freed = shm_gc_collect();
    println!("  gc_collect freed {} pages", freed);

    let check = shm_alloc_page();
    let ok = check >= 0;
    if ok {
        shm_free_page(check as usize);
        shm_gc_collect();
    }
    println!("  final alloc: {}", if ok { "PASS" } else { "FAIL" });
    if ok { println!("=== Node 0 done ==="); 0 } else { println!("=== Node 0 FAILED ==="); 1 }
}
