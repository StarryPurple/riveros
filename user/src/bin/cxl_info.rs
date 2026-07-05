#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{CxlMemInfo, query_cxl_meminfo};

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let mut info = CxlMemInfo::default();
    query_cxl_meminfo(&mut info);
    println!("=== CXL Memory Info ===");
    println!("Version:          {}", info.version);
    println!("Promotions:       {}", info.promote_count);
    println!("Demotions:        {}", info.demote_count);
    println!("Fast Allocs:      {}", info.fast_alloc_count);
    println!("Slow Allocs:      {}", info.slow_alloc_count);
    println!("Fast Deallocs:    {}", info.fast_dealloc_count);
    println!("Slow Deallocs:    {}", info.slow_dealloc_count);
    0
}
