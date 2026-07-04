#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate core;

use user_lib::{cxl_mmap, cxl_munmap};

const PAGE: usize = 4096;

fn write_pattern(ptr: *mut u8, pages: usize, pat: u8) {
    unsafe { for i in 0..(pages * PAGE) { ptr.add(i).write(pat); } }
}

fn verify_pattern(ptr: *mut u8, pages: usize, pat: u8) {
    unsafe {
        for i in 0..(pages * PAGE) {
            assert_eq!(ptr.add(i).read(), pat, "corruption at offset {}", i);
        }
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== CXL Shared Allocator Test ===\n");

    // 1. two allocations from shared pool
    let a = cxl_mmap(4 * PAGE) as *mut u8;
    let b = cxl_mmap(4 * PAGE) as *mut u8;
    assert!(!a.is_null() && !b.is_null(), "cxl_mmap failed");
    assert_ne!(a, b, "a and b overlapped");
    println!("a={:p}  b={:p}  (different addresses)", a, b);

    write_pattern(a, 4, 0xAB);
    write_pattern(b, 4, 0xCD);
    verify_pattern(a, 4, 0xAB);
    verify_pattern(b, 4, 0xCD);
    println!("two independent blocks ok");

    // 2. free a, allocate again (reuses pages)
    cxl_munmap(a as usize, 4 * PAGE);
    let c = cxl_mmap(4 * PAGE) as *mut u8;
    assert!(!c.is_null(), "realloc after free failed");
    assert_ne!(c, b, "c overlapped with b");
    println!("c={:p}  (recycled after a was freed)", c);

    write_pattern(c, 4, 0xEF);
    verify_pattern(c, 4, 0xEF);
    verify_pattern(b, 4, 0xCD);  // b unchanged
    println!("recycled block ok, b still intact");

    // 3. free everything
    cxl_munmap(b as usize, 4 * PAGE);
    cxl_munmap(c as usize, 4 * PAGE);

    println!("\n=== mmap_cxl passed ===");
    0
}
