#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::{LockFreeRing, sys_ring_create, sys_ring_destroy, thread_create, waittid, exit};

const MSG_SIZE: usize = 64;
const RING_CAP: usize = 4096;
const ITERS: usize = 200;

fn writer_spin(arg: usize) -> ! {
    let ring = unsafe { &*(arg as *const LockFreeRing) };
    let mut buf = [0u8; MSG_SIZE];
    for i in 0..ITERS {
        buf[0] = (i & 0xFF) as u8;
        buf[1] = ((i >> 8) & 0xFF) as u8;
        ring.push_spin(&buf);
    }
    exit(0)
}

fn reader_spin(arg: usize) -> ! {
    let ring = unsafe { &*(arg as *const LockFreeRing) };
    let mut buf = [0u8; MSG_SIZE];
    for _ in 0..ITERS {
        ring.pop_spin(&mut buf);
    }
    exit(0)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Ring Buffer Basic Test ===");

    // ── 1. Create ring via syscall ──
    let mut vaddr: usize = 0;
    let fd = sys_ring_create(RING_CAP, &mut vaddr as *mut usize);
    assert!(fd >= 0, "sys_ring_create failed: {}", fd);
    assert!(vaddr != 0, "vaddr is null");
    println!("  ring created: fd={}, vaddr={:#x}", fd, vaddr);

    let ring = LockFreeRing::new(vaddr as *mut u8);

    // ── 2. Sequential round-trip ──
    println!("--- Round-trip ---");
    let msg = [0x42u8; MSG_SIZE];
    ring.try_push(&msg).expect("push failed");
    let mut out = [0u8; MSG_SIZE];
    let n = ring.try_pop(&mut out).expect("pop failed");
    assert_eq!(n, MSG_SIZE);
    assert_eq!(out, msg);
    println!("  round-trip: OK");

    // ── 3. Busy-poll thread test ──
    println!("--- Thread busy-poll ---");
    let r_ptr = &ring as *const LockFreeRing as usize;
    let w = thread_create(linker_symbol_addr!(writer_spin), r_ptr);
    let r = thread_create(linker_symbol_addr!(reader_spin), r_ptr);
    waittid(w as usize);
    waittid(r as usize);
    println!("  busy-poll: OK ({} messages)", ITERS);

    // ── 4. Empty/full detection ──
    println!("--- Boundary ---");
    assert!(ring.is_empty(), "ring should be empty after full drain");
    // Fill completely
    let msg_fill = [0xBBu8; MSG_SIZE];
    for _ in 0..(RING_CAP / MSG_SIZE) {
        ring.try_push(&msg_fill).expect("fill should succeed");
    }
    assert!(ring.is_full(), "ring should be full after filling");
    // One more should fail
    assert!(ring.try_push(&msg_fill).is_err(), "extra push should fail");
    // Drain completely
    let mut drain = [0u8; MSG_SIZE];
    for _ in 0..(RING_CAP / MSG_SIZE) {
        ring.try_pop(&mut drain).expect("drain should succeed");
    }
    assert!(ring.is_empty(), "ring should be empty after drain");
    assert_eq!(ring.used(), 0);
    println!("  boundary: OK");

    // ── 5. Cleanup ──
    sys_ring_destroy(fd as usize);
    println!("=== All tests passed ===");
    0
}
