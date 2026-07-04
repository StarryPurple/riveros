#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::{CxlMemInfo, query_cxl_meminfo, add_cxl_card, remove_cxl_card, cxl_mmap, cxl_munmap, get_time};

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let mut mem = CxlMemInfo::default();
    println!("=== HashRing CXL Promote/Demote Demo ===\n");

    // ── setup: add 2 cards ──
    for cid in 0..2 {
        let ret = add_cxl_card(cid);
        assert!(ret >= 0, "add_cxl_card({}) failed", cid);
    }
    println!("added card 0 and 1");

    query_cxl_meminfo(&mut mem);
    let promote_before = mem.promote_count;
    let demote_before  = mem.demote_count;
    println!("promote_before={}, demote_before={}\n", promote_before, demote_before);

    // ── promote demo: allocate CXL pages, write to make them hot, wait ──
    println!("--- Promote Demo ---");
    const PROMOTE_PAGES: usize = 20;
    let ptr = cxl_mmap(PROMOTE_PAGES * 4096) as *mut u8;
    assert!(!ptr.is_null(), "cxl_mmap failed");
    println!("cxl_mmap {} pages at {:p}", PROMOTE_PAGES, ptr);

    // write a different byte per page so shared-PPN bugs are caught
    for pg in 0..PROMOTE_PAGES {
        let val = ((0x40 + pg) & 0xff) as u8;
        unsafe {
            let base = ptr.add(pg * 4096);
            for i in 0..4096 { base.add(i).write(val); }
        }
    }
    println!("wrote per-page patterns — pages are hot");

    // busy-wait ~3s so migrator scans OUR page table (not someone else's)
    println!("busy-waiting 3s for migrator ...");
    let start = get_time();
    while get_time() - start < 3000 {
        let _ = ptr; // keep reachable
    }

    query_cxl_meminfo(&mut mem);
    println!("promote_count: {} -> {}", promote_before, mem.promote_count);
    println!("demote_count:  {} -> {}", demote_before,  mem.demote_count);
    assert!(mem.promote_count > promote_before,
        "promote did NOT trigger -- migrator not scanning our pages?");
    println!("promote verified!\n");

    // ── verify data survived promote ──
    println!("--- Data Integrity After Promote ---");
    for pg in 0..PROMOTE_PAGES {
        let expected = ((0x40 + pg) & 0xff) as u8;
        unsafe {
            let base = ptr.add(pg * 4096);
            for i in 0..4096 {
                assert_eq!(base.add(i).read(), expected,
                    "corruption at page {} offset {}", pg, i);
            }
        }
    }
    println!("data intact after promote\n");

    // ── cleanup ──
    cxl_munmap(ptr as usize, PROMOTE_PAGES * 4096);
    for cid in 0..2 {
        remove_cxl_card(cid);
    }
    println!("cleanup done");

    query_cxl_meminfo(&mut mem);
    println!("final promote={} demote={}", mem.promote_count, mem.demote_count);
    println!("\n=== hashring_sleep_cxl test PASSED ===");
    0
}
