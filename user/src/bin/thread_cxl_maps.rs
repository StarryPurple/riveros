#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_mmap, cxl_munmap, thread_create, waittid, exit, yield_};

const THREADS: usize = 4;
const PAGES_PER_THREAD: usize = 8;
const PAGE: usize = 4096;

fn worker(id: usize) -> ! {
    let mut regions: [usize; PAGES_PER_THREAD] = [0; PAGES_PER_THREAD];

    for i in 0..PAGES_PER_THREAD {
        let sz = (id * 100 + i * 50 + PAGE) % (4 * PAGE) + PAGE;
        let va = cxl_mmap(sz);
        assert!(va > 0, "thread {} map {} failed", id, i);
        // Write a thread-specific pattern
        let ptr = va as *mut u8;
        for j in 0..sz {
            unsafe { *ptr.add(j) = ((id * 31 + i * 17 + j) & 0xFF) as u8; }
        }
        // Read back and verify
        for j in 0..sz {
            let v = unsafe { *ptr.add(j) };
            let exp = ((id * 31 + i * 17 + j) & 0xFF) as u8;
            assert!(v == exp, "thread {} region {} byte {}: got {:02x} exp {:02x}", id, i, j, v, exp);
        }
        regions[i] = va as usize;
    }

    yield_();

    // Free in reverse order
    for i in (0..PAGES_PER_THREAD).rev() {
        let sz = (id * 100 + i * 50 + PAGE) % (4 * PAGE) + PAGE;
        cxl_munmap(regions[i], sz);
    }

    exit(0)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Concurrent CXL mmap Stress ({} threads × {} maps) ===",
             THREADS, PAGES_PER_THREAD);

    let mut tids = [0isize; THREADS];
    for id in 0..THREADS {
        tids[id] = thread_create(linker_symbol_addr!(worker), id);
        assert!(tids[id] > 0, "create thread {}", id);
    }

    for id in 0..THREADS {
        waittid(tids[id] as usize);
    }
    println!("  all {} threads completed successfully", THREADS);

    // Final verification: should be able to alloc after all are done
    let va = cxl_mmap(PAGE);
    let ok = va > 0;
    if ok {
        let ptr = va as *mut u8;
        unsafe { for j in 0..PAGE { *ptr.add(j) = 0xAB; }}
        cxl_munmap(va as usize, PAGE);
    }
    println!("  post-stress alloc: {}", if ok { "PASS" } else { "FAIL" });
    if ok { 0 } else { 1 }
}
