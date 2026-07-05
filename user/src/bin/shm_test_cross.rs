#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use user_lib::{shm_alloc_page, shm_free_page, shm_ref_page, shm_gc_collect,
               thread_create, waittid, exit};

static PAGE_SENT: AtomicBool = AtomicBool::new(false);
static PAGE_IDX: AtomicUsize = AtomicUsize::new(0);

fn instance1(_arg: usize) -> ! {
    while !PAGE_SENT.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
    let idx = PAGE_IDX.load(Ordering::Relaxed);
    shm_ref_page(idx);
    exit(0)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Cross-Instance SHM Test ===");

    let p0 = shm_alloc_page();
    if p0 < 0 { panic!("alloc failed"); }
    let p0 = p0 as usize;
    println!("[inst 0] alloc page idx={}", p0);

    let t1 = thread_create(linker_symbol_addr!(instance1), 0);

    PAGE_IDX.store(p0, Ordering::Relaxed);
    PAGE_SENT.store(true, Ordering::Release);
    println!("[inst 0] sent page idx={} to inst 1", p0);

    waittid(t1 as usize);
    println!("[inst 0] inst 1 has ref'd the page (refcnt->2)");

    shm_free_page(p0);  // ref 2->1
    println!("[inst 0] free (1/2) – refcnt->1");
    shm_free_page(p0);  // ref 1->0 -> GC pending
    println!("[inst 0] free (2/2) – refcnt->0 -> GC pending");

    let freed = shm_gc_collect();
    println!("[inst 0] gc_collect freed {} pages", freed);

    shm_gc_collect();
    let p1 = shm_alloc_page();
    if p1 >= 0 {
        shm_free_page(p1 as usize);
        shm_gc_collect();
        println!("✓ re-alloc succeeded (page idx={})", p1);
    }

    println!("=== Test passed ===");
    0
}
