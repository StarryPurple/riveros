#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_push, cxl_rx_pop, shm_alloc_page, shm_free_page, shm_gc_collect, msg_seal, msg_verify};

const BULK_PAGES: usize = 128;
const TAG_PAGE: u8 = 0;
const TAG_BULK_DONE: u8 = 2;
const TAG_ACK: u8 = 1;

fn make_msg(idx: usize, tag: u8) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0..4].copy_from_slice(&(idx as u32).to_le_bytes());
    m[4] = tag;
    msg_seal(&mut m, 0);
    m
}

fn make_bulk_done(count: usize) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[4] = TAG_BULK_DONE;
    m[5..7].copy_from_slice(&(count as u16).to_le_bytes());
    m
}

fn read_msg(msg: &[u8; 60]) -> (usize, u8) {
    assert!(msg_verify(msg).is_some(), "checksum");
    let idx = u32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]) as usize;
    (idx, msg[4])
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Node 0: Bulk Page Transfer (Sender) ===");

    let mut pages = [0usize; BULK_PAGES];
    for i in 0..BULK_PAGES {
        let r = shm_alloc_page();
        if r < 0 { panic!("alloc failed at {}", i); }
        pages[i] = r as usize;
    }
    println!("  allocated {} pages", BULK_PAGES);

    for i in 0..BULK_PAGES {
        let msg = make_msg(pages[i], TAG_PAGE);
        while cxl_tx_push(&msg) != 0 {}
    }
    println!("  sent {} pages via ring", BULK_PAGES);

    let done = make_bulk_done(BULK_PAGES);
    while cxl_tx_push(&done) != 0 {}
    println!("  sent bulk-done signal");

    let mut reply = [0u8; 60];
    loop {
        if cxl_rx_pop(&mut reply) == 0 {
            let (_, kind) = read_msg(&reply);
            if kind == TAG_ACK { break; }
        }
    }
    println!("  received bulk ack from Node 1");

    let mut freed_count = 0usize;
    for &p in &pages {
        shm_free_page(p);
        freed_count += 1;
    }
    println!("  freed {} pages (refcnt -> 1, held by Node 1)", freed_count);

    let gc = shm_gc_collect();
    println!("  gc_collect freed {} pages", gc);

    let verify = shm_alloc_page();
    let ok = verify >= 0;
    if ok {
        shm_free_page(verify as usize);
        shm_gc_collect();
    }
    println!("  allocator functional after bulk: {}", if ok { "PASS" } else { "FAIL" });
    println!("=== Node 0 done ===");
    if ok { 0 } else { 1 }
}
