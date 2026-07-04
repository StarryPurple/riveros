#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;
extern crate core;

use user_lib::{CxlMemInfo, query_cxl_meminfo, cxl_mmap, cxl_munmap, CXL_CARD_COUNT};

fn total_slow_alloc(info: &CxlMemInfo) -> u64 {
    info.slow_alloc_count.iter().sum()
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    const N: usize = 20;
    let mut cxl_meminfo = CxlMemInfo::default();
    query_cxl_meminfo(&mut cxl_meminfo);
    let start_slow_alloc_count = total_slow_alloc(&cxl_meminfo);
    let ptr = cxl_mmap(N * 4096) as *mut u8;
    unsafe {
      for i in 0..(N * 4096) {
        ptr.add(i).write((i & 0xff) as u8);
      }
    }
    let mut sum = 0;
    unsafe {
      for i in 0..(N * 4096) {
        sum += ptr.add(i).read() as usize;
      }
    }
    let answer = (255 * 256 / 2) * (N * 4096 / 256) as usize;
    println!("checksum = {}, should be {}, {}", sum, answer, if sum == answer { "correct" } else { "incorrect" });
    assert_eq!(sum, answer);
    query_cxl_meminfo(&mut cxl_meminfo);
    let end_slow_alloc_count = total_slow_alloc(&cxl_meminfo);
    let delta = end_slow_alloc_count - start_slow_alloc_count;
    println!("cxl info after mmap: {:?}", cxl_meminfo);
    println!("cxl slow alloc delta = {}, shall be nearly always equal to N = {}", delta, N);
    // might be more than N due to page migration, but very very unlikely (they're frequently accessed)
    assert!(delta >= N as u64);
    cxl_munmap(ptr as usize, N * 4096);
    query_cxl_meminfo(&mut cxl_meminfo);
    println!("cxl info after munmap: {:?}", cxl_meminfo);
    0
}
