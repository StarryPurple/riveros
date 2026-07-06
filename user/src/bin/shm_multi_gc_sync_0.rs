#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_push, cxl_rx_pop, shm_alloc_page, shm_free_page, shm_ref_page, shm_gc_collect, msg_seal, msg_verify};

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
    println!("=== Node 0: GC Sync Test ===");

    // Pre-sync: advance VC[0] so it's at a known baseline
    shm_gc_collect();

    let p0 = shm_alloc_page();
    assert!(p0 >= 0, "alloc p0 failed");
    let p0 = p0 as usize;
    println!("  alloc p0={}", p0);

    // Self-ref to simulate multiple ref holders: rc 1->2
    shm_ref_page(p0);
    println!("  self-ref p0 (refcnt 1->2)");

    let msg = make_msg(p0, 0);
    while cxl_tx_push(&msg) != 0 {}
    println!("  sent p0 to Node 1");

    let mut reply = [0u8; 60];
    loop {
        if cxl_rx_pop(&mut reply) == 0 {
            let (idx, kind) = read_msg(&reply);
            if kind == 1 && idx == p0 { break; }
        }
    }
    println!("  got ack: Node 1 ref'd p0 (refcnt 2->3)");

    let p1 = shm_alloc_page();
    assert!(p1 >= 0, "alloc p1 failed");
    let p1 = p1 as usize;
    println!("  alloc p1={}", p1);

    let msg = make_msg(p1, 0);
    while cxl_tx_push(&msg) != 0 {}
    println!("  sent p1 to Node 1");

    let mut reply2 = [0u8; 60];
    loop {
        if cxl_rx_pop(&mut reply2) == 0 {
            let (idx, kind) = read_msg(&reply2);
            if kind == 1 && idx == p1 { break; }
        }
    }
    println!("  got ack: Node 1 ref'd p1 (refcnt 1->2)");

    // Send DONE so Node1 releases its refs
    let done = make_msg(0, 9);
    while cxl_tx_push(&done) != 0 {}
    println!("  sent DONE to Node 1");

    // Wait for Node1 to finish — it unrefs and signals back
    let mut ack = [0u8; 60];
    loop {
        if cxl_rx_pop(&mut ack) == 0 {
            let (_, kind) = read_msg(&ack);
            if kind == 9 { break; }
        }
    }
    println!("  Node 1 done (unref'd its refs)");

    // Now Node0 drops the LAST reference for both pages.
    // p0: rc was 3→2 (Node1 unref) → free(rc=1) → enters GC
    // p1: rc was 2→1 (Node1 unref) → free(rc=0) → enters GC
    shm_free_page(p0);
    println!("  freeing p0 (refcnt -> 1)");
    shm_free_page(p0);
    println!("  freeing p0 (refcnt -> 0, enters GC)");
    shm_free_page(p1);
    println!("  freeing p1 (refcnt -> 0, enters GC)");

    let freed = shm_gc_collect();
    println!("  gc_collect freed {} pages", freed);

    let verify = shm_alloc_page();
    let ok = verify >= 0;
    if ok {
        shm_free_page(verify as usize);
        shm_gc_collect();
    }
    println!("  allocator: {}", if ok { "PASS" } else { "FAIL" });
    if ok { 0 } else { 1 }
}
