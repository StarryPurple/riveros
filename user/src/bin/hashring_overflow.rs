#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{add_cxl_card, remove_cxl_card, cxl_mmap, cxl_munmap,
               CxlMemInfo, query_cxl_meminfo};

const PAGE: usize = 4096;
const CARD_MB: usize = 2;
const CARD_PAGES: usize = CARD_MB * 1024 * 1024 / PAGE; // 512 pages

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== HashRing Overflow / Capacity Boundary ===");
    println!("  1 card, {} MB = {} pages capacity", CARD_MB, CARD_PAGES);

    // Clean up any leftover cards from previous tests
    for cid in 0..user_lib::CXL_CARD_COUNT { let _ = remove_cxl_card(cid); }

    // Register only card 0
    assert!(add_cxl_card(0) >= 0, "add card 0");

    // Allocate pages one at a time until exhaustion
    let mut ptrs = [0usize; CARD_PAGES + 10];
    let mut count = 0usize;

    for i in 0..(CARD_PAGES + 10) {
        let va = cxl_mmap(PAGE);
        if va <= 0 {
            println!("  OOM at alloc #{} (card exhausted)", i);
            break;
        }
        // Write a pattern value
        let p = va as *mut u8;
        unsafe { p.write((i as u8).wrapping_mul(0x55)); }
        ptrs[count] = va as usize;
        count += 1;
    }

    let mut mem = CxlMemInfo::default();
    query_cxl_meminfo(&mut mem);

    println!("  allocated {} pages (card[0] alloc={}, capacity={})",
             count, mem.slow_alloc_count[0], CARD_PAGES);
    let within_reasonable = count >= CARD_PAGES * 90 / 100 && count <= CARD_PAGES * 110 / 100;
    println!("  capacity check ({}..{}): {}",
             CARD_PAGES * 90 / 100, CARD_PAGES * 110 / 100,
             if within_reasonable { "OK" } else { "suspicious" });

    // Verify all allocated pages still hold their patterns
    let mut ok = true;
    for i in 0..count {
        let p = ptrs[i] as *mut u8;
        let expected = (i as u8).wrapping_mul(0x55);
        let val = unsafe { p.read() };
        if val != expected {
            println!("  page {}: got {:02x} expected {:02x}", i, val, expected);
            ok = false;
            break;
        }
    }
    println!("  data integrity: {}", if ok { "OK" } else { "FAIL" });

    // Free half the pages
    let free_count = count / 2;
    for i in 0..free_count {
        cxl_munmap(ptrs[i], PAGE);
    }
    println!("  freed {} pages", free_count);

    // Re-allocate to verify freed pages are recycled
    let va = cxl_mmap(PAGE);
    let recycled = va > 0;
    if recycled {
        cxl_munmap(va as usize, PAGE);
    }
    println!("  re-alloc after free: {}", if recycled { "OK" } else { "FAIL (no space?)" });

    // Free rest
    for i in free_count..count {
        cxl_munmap(ptrs[i], PAGE);
    }

    let ret = remove_cxl_card(0);
    let final_ok = within_reasonable && ok && recycled && ret == 0;
    println!("  remove card 0: {}", if ret == 0 { "OK" } else { "FAIL" });
    println!("  {}", if final_ok { "PASS" } else { "FAIL" });
    if final_ok { 0 } else { 1 }
}
