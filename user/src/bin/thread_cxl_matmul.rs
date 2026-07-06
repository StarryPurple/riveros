#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use user_lib::{cxl_mmap, cxl_munmap, thread_create, waittid, exit, yield_, get_time};

const N: usize = 64;
const WORKERS: usize = 4;

static A_PTR: AtomicU64 = AtomicU64::new(0);
static B_PTR: AtomicU64 = AtomicU64::new(0);
static C_PTR: AtomicU64 = AtomicU64::new(0);
static GOLDEN_PTR: AtomicU64 = AtomicU64::new(0);
static START: AtomicBool = AtomicBool::new(false);
static WORKER_DONE: [AtomicBool; WORKERS] = [
    AtomicBool::new(false), AtomicBool::new(false),
    AtomicBool::new(false), AtomicBool::new(false),
];

fn val(i: usize, j: usize) -> i64 {
    (i.wrapping_mul(7).wrapping_add(j.wrapping_mul(13)) % 100) as i64
}

fn worker(id: usize) -> ! {
    while !START.load(Ordering::Acquire) { yield_(); }
    let a = A_PTR.load(Ordering::Relaxed) as *const i64;
    let b = B_PTR.load(Ordering::Relaxed) as *const i64;
    let c = C_PTR.load(Ordering::Relaxed) as *mut i64;
    let rows = N / WORKERS;
    let r0 = id * rows;
    let r1 = if id == WORKERS - 1 { N } else { r0 + rows };

    unsafe {
        for i in r0..r1 {
            for k in 0..N {
                let aik = *a.add(i * N + k);
                if aik == 0 { continue; }
                for j in 0..N {
                    *c.add(i * N + j) += aik * *b.add(k * N + j);
                }
            }
        }
    }
    WORKER_DONE[id].store(true, Ordering::Release);
    exit(0)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== CXL Blocked Matrix Multiply ({}x{}) ===", N, N);

    let mat_sz = N * N * core::mem::size_of::<i64>();
    let total_sz = mat_sz * 4;
    let va = cxl_mmap(total_sz);
    assert!(va > 0, "cxl_mmap failed");
    let a = va as *mut i64;
    let b = unsafe { a.add(N * N) };
    let c = unsafe { b.add(N * N) };
    let golden = unsafe { c.add(N * N) };
    println!("  cxl_mmap {}B", total_sz);

    unsafe {
        for i in 0..N { for j in 0..N {
            *a.add(i * N + j) = val(i, j);
            *b.add(i * N + j) = val(j, i.wrapping_add(N));
            *c.add(i * N + j) = 0;
            *golden.add(i * N + j) = 0;
        }}
    }
    // Single-threaded golden reference
    unsafe {
        for i in 0..N {
            for k in 0..N {
                let aik = *a.add(i * N + k);
                for j in 0..N {
                    *golden.add(i * N + j) += aik * *b.add(k * N + j);
                }
            }
        }
    }
    println!("  golden reference computed");

    A_PTR.store(a as u64, Ordering::Release);
    B_PTR.store(b as u64, Ordering::Release);
    C_PTR.store(c as u64, Ordering::Release);
    GOLDEN_PTR.store(golden as u64, Ordering::Release);

    let mut tids = [0isize; WORKERS];
    for id in 0..WORKERS {
        tids[id] = thread_create(linker_symbol_addr!(worker), id);
        assert!(tids[id] > 0, "create worker {}", id);
    }

    let t0 = get_time();
    START.store(true, Ordering::Release);
    for id in 0..WORKERS {
        waittid(tids[id] as usize);
    }
    let ms = get_time() - t0;
    println!("  {} workers, {} ms", WORKERS, ms);

    let mut errs = 0usize;
    unsafe {
        for i in 0..N { for j in 0..N {
            let got = *c.add(i * N + j);
            let exp = *golden.add(i * N + j);
            if got != exp {
                if errs < 3 { println!("  [{},{}] got {} exp {}", i, j, got, exp); }
                errs += 1;
            }
        }}
    }
    let ok = errs == 0;
    println!("  mismatches={}: {}", errs, if ok { "PASS" } else { "FAIL" });

    cxl_munmap(va as usize, total_sz);
    if ok { 0 } else { 1 }
}
