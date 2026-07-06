#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use user_lib::{cxl_mmap, cxl_munmap, thread_create, waittid, exit, yield_};

const DATA_SIZE: usize = 4096 * 4; // 4 pages of i64 = 2048 elements
const WORKERS: usize = 4;

static READY: AtomicBool = AtomicBool::new(false);
static DONE: [AtomicBool; WORKERS] = [
    AtomicBool::new(false), AtomicBool::new(false),
    AtomicBool::new(false), AtomicBool::new(false),
];
static PARTIAL_SUMS: [AtomicI64; WORKERS] = [
    AtomicI64::new(0), AtomicI64::new(0),
    AtomicI64::new(0), AtomicI64::new(0),
];
static DATA_BASE: AtomicI64 = AtomicI64::new(0); // holds the VA of cxl array
static EXPECTED_SUM: AtomicI64 = AtomicI64::new(0);

fn worker(id: usize) -> ! {
    while !READY.load(Ordering::Acquire) {
        yield_();
    }
    let base = DATA_BASE.load(Ordering::Relaxed) as *const i64;
    let n = DATA_SIZE / core::mem::size_of::<i64>();
    let chunk = n / WORKERS;
    let start = id * chunk;
    let end = if id == WORKERS - 1 { n } else { start + chunk };

    let mut sum: i64 = 0;
    for i in start..end {
        sum += unsafe { *base.add(i) };
    }
    PARTIAL_SUMS[id].store(sum, Ordering::Release);
    DONE[id].store(true, Ordering::Release);
    exit(0)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== CXL Parallel Sum (Map-Reduce) ===");

    let va = cxl_mmap(DATA_SIZE);
    assert!(va > 0, "cxl_mmap failed");
    let base = va as *mut i64;
    println!("  cxl_mmap({}B) -> VA {:#x}", DATA_SIZE, va);

    // Fill array with values 0..N-1
    let n = DATA_SIZE / core::mem::size_of::<i64>();
    unsafe {
        for i in 0..n {
            *base.add(i) = i as i64;
        }
    }

    // Compute expected sum (golden reference)
    let expected: i64 = (0..n as i64).sum();
    EXPECTED_SUM.store(expected, Ordering::Relaxed);
    println!("  filled {} elements, expected sum = {}", n, expected);

    DATA_BASE.store(va as i64, Ordering::Release);

    // Create workers
    let mut tids = [0isize; WORKERS];
    for id in 0..WORKERS {
        tids[id] = thread_create(linker_symbol_addr!(worker), id);
        assert!(tids[id] > 0, "thread_create failed for worker {}", id);
    }
    println!("  {} workers created", WORKERS);

    // Release workers
    READY.store(true, Ordering::Release);

    // Wait for all workers
    for id in 0..WORKERS {
        waittid(tids[id] as usize);
    }
    println!("  all workers done");

    // Collect results
    let total: i64 = (0..WORKERS).map(|id| PARTIAL_SUMS[id].load(Ordering::Acquire)).sum();
    let ok = total == expected;
    println!("  total sum = {} (expected {}): {}", total, expected,
             if ok { "PASS" } else { "FAIL" });

    // Verify no overlap: each partial sum should be in expected range
    let mut partial_ok = true;
    let chunk = n / WORKERS;
    for id in 0..WORKERS {
        let s = id as i64 * chunk as i64;
        let e = if id == WORKERS - 1 { n as i64 - 1 } else { s + chunk as i64 - 1 };
        let expected_partial: i64 = (s..=e).sum();
        let got = PARTIAL_SUMS[id].load(Ordering::Acquire);
        if got != expected_partial {
            partial_ok = false;
            println!("  worker {} partial expected {} got {}", id, expected_partial, got);
        }
    }
    if partial_ok {
        println!("  all partial sums correct: PASS");
    }

    cxl_munmap(va as usize, DATA_SIZE);
    println!("  unmapped");

    if ok && partial_ok { 0 } else { 1 }
}
