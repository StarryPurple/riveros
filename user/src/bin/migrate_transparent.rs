#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{CxlMemInfo, query_cxl_meminfo, add_cxl_card, remove_cxl_card,
               cxl_mmap, cxl_munmap, get_time};

const PAGES: usize = 16;
const WAIT_MS: usize = 3500;

fn write_pattern(ptr: *mut u8, pages: usize, seed: u8) {
    for pg in 0..pages {
        let val = seed.wrapping_add(pg as u8);
        unsafe {
            let base = ptr.add(pg * 4096);
            for i in 0..4096 { base.add(i).write(val); }
        }
    }
}
fn check_pattern(ptr: *mut u8, pages: usize, seed: u8) -> bool {
    for pg in 0..pages {
        let val = seed.wrapping_add(pg as u8);
        unsafe {
            let base = ptr.add(pg * 4096);
            for i in 0..4096 {
                if base.add(i).read() != val {
                    println!("  page {} offset {}: expected {:02x}", pg, i, val);
                    return false;
                }
            }
        }
    }
    true
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Migrate: Transparent Page Promotion ===");
    println!("  {} pages, waiting {}ms for migrator", PAGES, WAIT_MS);

    // Clean slate
    for cid in 0..user_lib::CXL_CARD_COUNT { let _ = remove_cxl_card(cid); }
    for cid in 0..2 { assert!(add_cxl_card(cid) >= 0, "add card {}", cid);}

    let mut mem = CxlMemInfo::default();
    query_cxl_meminfo(&mut mem);
    let prom_before = mem.promote_count;
    println!("  promote_before={}", prom_before);

    // Allocate CXL pages and write patterns
    let ptr = cxl_mmap(PAGES * 4096) as *mut u8;
    assert!(ptr as usize > 0, "cxl_mmap failed");
    write_pattern(ptr, PAGES, 0xA0);
    println!("  allocated+written {} CXL pages", PAGES);

    // Busy-access: repeatedly touch pages to set accessed/dirty bits
    println!("  busy-accessing pages for {}ms...", WAIT_MS);
    let start = get_time();
    while get_time() - start < WAIT_MS as isize {
        // Touch every page to keep accessed bit set
        for pg in 0..PAGES {
            unsafe {
                let _ = ptr.add(pg * 4096).read_volatile();
                ptr.add(pg * 4096).write_volatile(0xCC);
                ptr.add(pg * 4096).write_volatile(0xA0u8.wrapping_add(pg as u8));
            }
        }
    }

    query_cxl_meminfo(&mut mem);
    println!("  promote_count: {} -> {}", prom_before, mem.promote_count);
    let promoted = mem.promote_count > prom_before;

    // Verify data survived migration
    let data_ok = check_pattern(ptr, PAGES, 0xA0);
    println!("  data integrity: {}", if data_ok { "OK" } else { "CORRUPTED" });

    cxl_munmap(ptr as usize, PAGES * 4096);
    for cid in 0..2 { remove_cxl_card(cid); }

    let ok = promoted && data_ok;
    println!("  promotion: {}, integrity: {} → {}",
             if promoted { "detected" } else { "not detected (migrator may need longer wait)" },
             if data_ok { "PASS" } else { "FAIL" },
             if ok { "PASS" } else { "REVIEW" });
    if ok { 0 } else { 1 }
}
