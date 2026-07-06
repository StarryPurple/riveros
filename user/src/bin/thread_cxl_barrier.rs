#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use user_lib::{cxl_mmap, cxl_munmap, cxl_tx_push, cxl_rx_pop, cxl_tx_pop, cxl_rx_push,
               thread_create, waittid, exit, yield_, get_time, get_instance_id};

const ROUNDS: usize = 10;
const LOCAL_THREADS: usize = 3;

struct DistBarrier {
    round: AtomicUsize,
    arrived: AtomicUsize,
    released: AtomicBool,
}

static BARR_PTR: AtomicUsize = AtomicUsize::new(0);
static LOCAL_DONE: [AtomicBool; LOCAL_THREADS] = [
    AtomicBool::new(false), AtomicBool::new(false), AtomicBool::new(false),
];
static START: AtomicBool = AtomicBool::new(false);

fn make_msg(tag: u8, val: usize) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0] = tag;
    m[1..9].copy_from_slice(&(val as u64).to_le_bytes());
    m
}
fn read_msg(m: &[u8; 60]) -> (u8, usize) {
    let mut b = [0u8; 8]; b.copy_from_slice(&m[1..9]);
    (m[0], u64::from_le_bytes(b) as usize)
}

// ----- Instance 0: Coordinator -----
fn coord_worker(id: usize) -> ! {
    while !START.load(Ordering::Acquire) { yield_(); }
    let b = unsafe { &*(BARR_PTR.load(Ordering::Relaxed) as *const DistBarrier) };

    for _ in 0..ROUNDS {
        // Local barrier: count arrivals
        let cnt = b.arrived.fetch_add(1, Ordering::AcqRel) + 1;
        if cnt == LOCAL_THREADS {
            // I'm last — signal peer, wait for peer, then release
            while cxl_tx_push(&make_msg(1, b.round.load(Ordering::Relaxed))) != 0 {}
            let mut reply = [0u8; 60];
            loop {
                if cxl_rx_pop(&mut reply) == 0 {
                    if read_msg(&reply).0 == 2 { break; }
                }
            }
            b.released.store(true, Ordering::Release);
        } else {
            while !b.released.load(Ordering::Acquire) { core::hint::spin_loop(); }
        }
        // Reset for next round
        core::sync::atomic::fence(Ordering::SeqCst);
        if b.released.swap(false, Ordering::AcqRel) {
            // Only let one thread reset arrived and advance round
            b.arrived.store(0, Ordering::Release);
            b.round.fetch_add(1, Ordering::Relaxed);
        }
    }
    LOCAL_DONE[id].store(true, Ordering::Release);
    exit(0)
}

// ----- Instance 1: Participant -----
fn part_worker(id: usize) -> ! {
    while !START.load(Ordering::Acquire) { yield_(); }
    let b = unsafe { &*(BARR_PTR.load(Ordering::Relaxed) as *const DistBarrier) };

    for _ in 0..ROUNDS {
        let cnt = b.arrived.fetch_add(1, Ordering::AcqRel) + 1;
        if cnt == LOCAL_THREADS {
            // Last local thread: signal peer (mirror direction)
            while cxl_rx_push(&make_msg(2, b.round.load(Ordering::Relaxed))) != 0 {}
            // Wait for peer's release signal
            let mut sig = [0u8; 60];
            loop {
                if cxl_tx_pop(&mut sig) == 0 {
                    if read_msg(&sig).0 == 1 { break; }
                }
            }
            b.released.store(true, Ordering::Release);
        } else {
            while !b.released.load(Ordering::Acquire) { core::hint::spin_loop(); }
        }
        core::sync::atomic::fence(Ordering::SeqCst);
        if b.released.swap(false, Ordering::AcqRel) {
            b.arrived.store(0, Ordering::Release);
            b.round.fetch_add(1, Ordering::Relaxed);
        }
    }
    LOCAL_DONE[id].store(true, Ordering::Release);
    exit(0)
}

// ----- Main -----
#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let me = get_instance_id();
    println!("=== Distributed Barrier (instance {}) ===", me);

    let sz = core::mem::size_of::<DistBarrier>();
    let va = cxl_mmap(sz);
    assert!(va > 0, "cxl_mmap failed");
    let b = unsafe { &*(va as *mut DistBarrier) };
    b.round.store(0, Ordering::Release);
    b.arrived.store(0, Ordering::Release);
    b.released.store(false, Ordering::Release);
    BARR_PTR.store(va as usize, Ordering::Release);
    println!("  barrier at VA {:#x}, {} threads, {} rounds", va, LOCAL_THREADS, ROUNDS);

    // Spawn local workers (use correct role function)
    let mut tids = [0isize; LOCAL_THREADS];
    for id in 0..LOCAL_THREADS {
        let entry = if me == 0 {
            linker_symbol_addr!(coord_worker)
        } else {
            linker_symbol_addr!(part_worker)
        };
        tids[id] = thread_create(entry, id);
        assert!(tids[id] > 0, "create worker {}", id);
    }

    let t0 = get_time();
    START.store(true, Ordering::Release);

    // Instance 1 is reactive: coordinator pushes first, participant responds
    // Instance 0 is proactive: its last-local-thread pushes to peer

    for id in 0..LOCAL_THREADS {
        waittid(tids[id] as usize);
    }
    let ms = get_time() - t0;
    println!("  {} rounds × {} threads in {} ms", ROUNDS, LOCAL_THREADS, ms);

    // Verify barrier struct left clean
    let arrived = b.arrived.load(Ordering::Relaxed);
    let released = b.released.load(Ordering::Relaxed);
    let ok = arrived == 0 && !released;
    println!("  final: arrived={} released={}: {}",
             arrived, released, if ok { "PASS" } else { "FAIL" });

    cxl_munmap(va as usize, sz);
    if ok { 0 } else { 1 }
}
