#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{add_cxl_card, remove_cxl_card, cxl_mmap, cxl_munmap,
               thread_create, waittid, exit, yield_,
               CXL_CARD_COUNT};

const ROUNDS: usize = 20;
const WORKERS: usize = 4;
const SMALL_PAGES: usize = 4;

static CARD_IDS: [usize; WORKERS] = [0, 1, 2, 3];
static WORKER_DONE: [core::sync::atomic::AtomicBool; WORKERS] = [
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
];
static START: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

use core::sync::atomic::Ordering;

fn card_shuffler(id: usize) -> ! {
    while !START.load(Ordering::Acquire) {}
    let card = CARD_IDS[id];

    for _round in 0..ROUNDS {
        // Add card
        while add_cxl_card(card) < 0 {}
        // Remove card
        while remove_cxl_card(card) != 0 {}
    }
    WORKER_DONE[id].store(true, Ordering::Release);
    exit(0)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== HashRing Concurrent Add/Remove Stress ===");
    println!("  {} workers x {} rounds, cards {:?}", WORKERS, ROUNDS, &CARD_IDS[..WORKERS]);

    // Spawn workers
    let mut tids = [0isize; WORKERS];
    for id in 0..WORKERS {
        tids[id] = thread_create(linker_symbol_addr!(card_shuffler), id);
        assert!(tids[id] > 0, "thread_create {}", id);
    }

    START.store(true, Ordering::Release);

    // Main thread: try to alloc + write/verify while cards shuffle
    let mut alloc_count = 0usize;
    let mut crash = false;
    loop {
        // Small alloc — may fail if no cards available, retry
        let ptr = cxl_mmap(SMALL_PAGES * 4096);
        if ptr > 0 {
            let p = ptr as *mut u8;
            let val = (alloc_count as u8).wrapping_mul(17);
            unsafe {
                for i in 0..(SMALL_PAGES * 4096) {
                    p.add(i).write(val);
                }
                for i in 0..(SMALL_PAGES * 4096) {
                    if p.add(i).read() != val {
                        println!("  data corruption at offset {} (alloc #{})", i, alloc_count);
                        crash = true;
                        break;
                    }
                }
            }
            cxl_munmap(p as usize, SMALL_PAGES * 4096);
            alloc_count += 1;
        } else {
            yield_(); // give workers a chance to add cards
        }

        let all_done = (0..WORKERS).all(|i| WORKER_DONE[i].load(Ordering::Acquire));
        if all_done { break; }
    }

    for id in 0..WORKERS {
        waittid(tids[id] as usize);
    }
    println!("  main thread: {} successful allocs", alloc_count);

    // Cleanup: ensure no cards left registered
    for cid in 0..WORKERS {
        let _ = remove_cxl_card(cid);
    }

    let ok = !crash && alloc_count > 0;
    println!("  {}: no deadlock, no corruption, {} allocs",
             if ok { "PASS" } else { "FAIL" }, alloc_count);
    if ok { 0 } else { 1 }
}
