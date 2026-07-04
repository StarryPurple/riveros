#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::{
    CxlMemInfo, query_cxl_meminfo,
    add_cxl_card, remove_cxl_card, cxl_mmap, cxl_munmap,
};

const ITERATIONS: usize = 10;
const CARDS: usize = 4;
const PAGES: usize = 20;

fn pattern(iter: usize, i: usize) -> u8 {
    (0x20u8).wrapping_add((i as u8) << 4).wrapping_add(iter as u8)
}

fn alloc_and_write(iter: usize, i: usize) -> (*mut u8, usize) {
    let size = PAGES * 4096;
    let ptr = cxl_mmap(size) as *mut u8;
    assert!(!ptr.is_null(), "cxl_mmap failed");
    let val = pattern(iter, i);
    unsafe {
        for off in 0..size {
            ptr.add(off).write(val);
        }
    }
    (ptr, PAGES)
}

fn verify_data(ptr: *mut u8, pages: usize, iter: usize, i: usize) {
    let val = pattern(iter, i);
    unsafe {
        for off in 0..(pages * 4096) {
            let actual = ptr.add(off).read();
            if actual != val {
                panic!("corruption at offset {}: expected {:#x} got {:#x}", off, val, actual);
            }
        }
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let mut mem = CxlMemInfo::default();
    println!("=== HashRing CXL Stress Test ===");
    println!("  {} iterations of add/alloc/verify/remove\n", ITERATIONS);

    for iter in 0..ITERATIONS {
        println!("[{}/{}]", iter + 1, ITERATIONS);

        // add
        for cid in 0..CARDS {
            let r = add_cxl_card(cid);
            assert!(r >= 0, "add({}) failed at iter {}", cid, iter);
        }

        // alloc + verify
        let mut bufs: [(*mut u8, usize); 2] = [(core::ptr::null_mut(), 0); 2];
        for i in 0..2 {
            bufs[i] = alloc_and_write(iter, i);
        }
        for (i, &(p, n)) in bufs.iter().enumerate() {
            verify_data(p, n, iter, i);
        }

        // remove
        for cid in 0..CARDS {
            remove_cxl_card(cid);
        }

        // data still accessible (promoted to DRAM)
        for (i, &(p, n)) in bufs.iter().enumerate() {
            verify_data(p, n, iter, i);
        }

        // cleanup
        for &(p, n) in bufs.iter() {
            cxl_munmap(p as usize, n * 4096);
        }

        // periodic stat check
        if (iter + 1) % 5 == 0 || iter == 0 {
            query_cxl_meminfo(&mut mem);
            let total_alloc: u64 = mem.slow_alloc_count.iter().sum();
            let total_dealloc: u64 = mem.slow_dealloc_count.iter().sum();
            println!("  alloc={} dealloc={}  {}",
                total_alloc, total_dealloc,
                if total_alloc == total_dealloc { "balanced" } else { "UNBALANCED!" });
        } else {
            println!("  done");
        }
    }

    // final check: all cards alloc == dealloc
    query_cxl_meminfo(&mut mem);
    let total_alloc: u64 = mem.slow_alloc_count.iter().sum();
    let total_dealloc: u64 = mem.slow_dealloc_count.iter().sum();
    if total_alloc == total_dealloc {
        println!("\n  all cards balanced: OK  (no leak)");
    } else {
        println!("\n  LEAK DETECTED: alloc={} dealloc={}", total_alloc, total_dealloc);
        return -1;
    }

    println!("\n=== hashring_stress_cxl PASSED ===");
    0
}
