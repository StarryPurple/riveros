#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::{read_volatile, write_volatile};
use user_lib::{cxl_tx_push, cxl_tx_pop, cxl_rx_push, cxl_rx_pop,
               sys_ring_create_cross, get_instance_id, msg_seal, msg_verify};

const DATA_BASE:    usize = 128;  // safely past any kernel-initialised RingHeader
const PATTERN_SIZE: usize = 4096;
const RESP_OFFSET:  usize = 4096; // second 4KB — well clear of pattern area

fn make_ctrl(tag: u8, offset: u32, len: u32, sender: usize) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0] = tag;
    m[1..5].copy_from_slice(&offset.to_le_bytes());
    m[5..9].copy_from_slice(&len.to_le_bytes());
    msg_seal(&mut m, sender);
    m
}
fn read_ctrl(m: &[u8; 60]) -> (u8, u32, u32) {
    assert!(msg_verify(m).is_some(), "checksum");
    let off = u32::from_le_bytes([m[1], m[2], m[3], m[4]]) as usize;
    let len = u32::from_le_bytes([m[5], m[6], m[7], m[8]]) as usize;
    (m[0], off as u32, len as u32)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let mut cross_va: usize = 0;
    let r = sys_ring_create_cross(&mut cross_va);
    if r < 0 {
        println!("  ring_cross_create failed (run on single node?)");
        return 1;
    }
    let base = unsafe { (cross_va as *mut u8).add(DATA_BASE) };

    let me = get_instance_id();
    if me == 0 {
        run_master(base)
    } else {
        run_worker(base)
    }
}

fn run_master(base: *mut u8) -> i32 {
    println!("=== CXL Data Plane — Node 0 (Master) ===");
    println!("  cross VA: {:#x}, data at offset {}", base as usize - DATA_BASE, DATA_BASE);

    // Write pattern via volatile (data plane — no ring copy)
    for i in 0..PATTERN_SIZE {
        unsafe { write_volatile(base.add(i), (i as u8).wrapping_mul(0xAB)); }
    }

    // Notify worker via control plane
    let ctrl = make_ctrl(1, 0, PATTERN_SIZE as u32, 0);
    while cxl_tx_push(&ctrl) != 0 {}
    println!("  wrote pattern, sent notification");

    // Wait for response notification
    let mut m = [0u8; 60];
    loop {
        if cxl_rx_pop(&mut m) == 0 {
            assert!(msg_verify(&m).is_some(), "checksum");
            if m[0] == 2 { break; }
        }
    }
    let (_, off, _) = read_ctrl(&m);
    println!("  worker done, response at base+{}", off);

    // Read response via volatile
    let resp_off = off as usize;
    let mut ok = true;
    for i in 0..PATTERN_SIZE {
        let v = unsafe { read_volatile(base.add(resp_off + i)) };
        let exp = (i as u8).wrapping_mul(0xCD);
        if v != exp {
            if i < 5 { println!("  byte {}: got {:02x} exp {:02x}", i, v, exp); }
            ok = false;
            break;
        }
    }
    println!("  response verified: {}", if ok { "PASS" } else { "FAIL" });
    if ok { 0 } else { 1 }
}

fn run_worker(base: *mut u8) -> i32 {
    println!("=== CXL Data Plane — Node 1 (Worker) ===");
    println!("  cross VA: {:#x}, data at offset {}", base as usize - DATA_BASE, DATA_BASE);

    // Wait for master notification
    let mut m = [0u8; 60];
    loop {
        if cxl_tx_pop(&mut m) == 0 {
            assert!(msg_verify(&m).is_some(), "checksum");
            if m[0] == 1 { break; }
        }
    }
    let (_, off, len) = read_ctrl(&m);
    println!("  got notification: base+{}, len={}", off, len);

    // Read pattern via volatile
    let data_off = off as usize;
    let mut ok = true;
    for i in 0..len as usize {
        let v = unsafe { read_volatile(base.add(data_off + i)) };
        let exp = (i as u8).wrapping_mul(0xAB);
        if v != exp {
            if i < 5 { println!("  byte {}: got {:02x} exp {:02x}", i, v, exp); }
            ok = false;
            break;
        }
    }
    println!("  pattern verified: {}", if ok { "PASS (direct LD from CXL)" } else { "FAIL" });

    // Write response via volatile
    for i in 0..PATTERN_SIZE {
        unsafe { write_volatile(base.add(RESP_OFFSET + i), (i as u8).wrapping_mul(0xCD)); }
    }

    // Notify master
    let ctrl = make_ctrl(2, RESP_OFFSET as u32, PATTERN_SIZE as u32, 1);
    while cxl_rx_push(&ctrl) != 0 {}
    println!("  wrote response, sent notification");
    0
}
