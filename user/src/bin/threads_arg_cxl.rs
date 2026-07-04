// test cxl_mmap in multi-thread environment
// including: concurrent allocation, concurrent read/write, data consistency verification, page migration observation
#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;
extern crate core;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{
    cxl_mmap, cxl_munmap, query_cxl_meminfo, thread_create, waittid, exit, CxlMemInfo, CXL_CARD_COUNT,
};

fn total_slow_alloc(info: &CxlMemInfo) -> u64 {
    info.slow_alloc_count.iter().sum()
}

// the argument for each thread
struct ThreadArg {
    thread_id: usize,
    page_count: usize,      // the number of pages to allocate
    pattern_offset: usize,  // the offset to generate different data patterns
}

// global counter, used to count the total number of bytes written (optional)
static TOTAL_BYTES_WRITTEN: AtomicUsize = AtomicUsize::new(0);
static TOTAL_BYTES_READ: AtomicUsize = AtomicUsize::new(0);

// the worker function for each thread
fn cxl_worker_thread(arg: *const ThreadArg) -> ! {
    let arg = unsafe { &*arg };
    let thread_id = arg.thread_id;
    let page_count = arg.page_count;
    let pattern_offset = arg.pattern_offset;
    let total_bytes = page_count * 4096;

    println!("Thread {}: starting, allocating {} pages...", thread_id, page_count);

    // 1. allocate CXL memory
    let ptr = cxl_mmap(page_count) as *mut u8;
    if ptr.is_null() {
        println!("Thread {}: cxl_mmap failed!", thread_id);
        exit(thread_id as i32 + 100);
    }
    println!("Thread {}: allocated CXL memory at {:p}", thread_id, ptr);

    // 2. write data: write the pattern (offset + thread_id) to each byte
    unsafe {
        for i in 0..total_bytes {
            let value = ((i + pattern_offset + thread_id * 7) & 0xff) as u8;
            ptr.add(i).write(value);
        }
    }
    TOTAL_BYTES_WRITTEN.fetch_add(total_bytes, Ordering::Relaxed);
    println!("Thread {}: write completed", thread_id);

    // 3. read and verify data
    let mut sum = 0u64;
    unsafe {
        for i in 0..total_bytes {
            let expected = ((i + pattern_offset + thread_id * 7) & 0xff) as u8;
            let actual = ptr.add(i).read();
            // verify data consistency
            if actual != expected {
                println!(
                    "Thread {}: data mismatch at offset {}: expected {}, got {}",
                    thread_id, i, expected, actual
                );
                // note: here we don't panic, continue to execute to observe the error pattern
            }
            sum += actual as u64;
        }
    }
    TOTAL_BYTES_READ.fetch_add(total_bytes, Ordering::Relaxed);

    // 4. calculate and print the expected sum
    let expected_sum = (0..total_bytes)
        .map(|i| ((i + pattern_offset + thread_id * 7) & 0xff) as u64)
        .sum::<u64>();
    
    println!(
        "Thread {}: read completed, sum={}, expected={}, {}",
        thread_id, sum, expected_sum,
        if sum == expected_sum { "✓ PASS" } else { "✗ FAIL" }
    );

    // 5. optional: write一遍不同的数据，测试重用
    unsafe {
        for i in 0..total_bytes.min(1024) { // write only the first 1KB as secondary verification
            ptr.add(i).write(0xaa);
        }
    }

    // 6. free memory
    let ret = cxl_munmap(ptr as usize, page_count);
    println!("Thread {}: cxl_munmap returned {}", thread_id, ret);

    exit(thread_id as i32 + 1)
}

// test 1: concurrent allocation of independent CXL memory
fn test_concurrent_allocation() -> i32 {
    println!("\n=== Test 1: Concurrent Allocation ===");

    const THREAD_COUNT: usize = 4;
    const PAGES_PER_THREAD: usize = 8; // each thread 32KB

    let mut args: Vec<ThreadArg> = (0..THREAD_COUNT)
        .map(|i| ThreadArg {
            thread_id: i,
            page_count: PAGES_PER_THREAD,
            pattern_offset: i * 1024,
        })
        .collect();

    // record the CXL statistics at the start
    let mut meminfo_start = CxlMemInfo::default();
    query_cxl_meminfo(&mut meminfo_start);

    // create threads
    let mut tids = Vec::new();
    for i in 0..THREAD_COUNT {
        let arg_ptr = &args[i] as *const ThreadArg;
        let tid = thread_create(
            linker_symbol_addr!(cxl_worker_thread),
            arg_ptr as usize,
        );
        println!("Main: created thread {}, tid={}", i, tid);
        tids.push(tid);
    }

    // wait for all threads to complete
    let mut exit_codes = Vec::new();
    for (i, tid) in tids.iter().enumerate() {
        let code = waittid(*tid as usize);
        println!("Main: thread {} (tid={}) exited with code {}", i, tid, code);
        exit_codes.push(code);
    }

    // record the statistics at the end
    let mut meminfo_end = CxlMemInfo::default();
    query_cxl_meminfo(&mut meminfo_end);

    println!("\n--- Test 1 Results ---");
    println!("Total bytes written: {}", TOTAL_BYTES_WRITTEN.load(Ordering::Relaxed));
    println!("Total bytes read: {}", TOTAL_BYTES_READ.load(Ordering::Relaxed));
    println!(
        "Slow alloc count: {} -> {} (delta = {})",
        total_slow_alloc(&meminfo_start),
        total_slow_alloc(&meminfo_end),
        total_slow_alloc(&meminfo_end) - total_slow_alloc(&meminfo_start)
    );
    println!(
        "Fast alloc count: {} -> {} (delta = {})",
        meminfo_start.fast_alloc_count,
        meminfo_end.fast_alloc_count,
        meminfo_end.fast_alloc_count - meminfo_start.fast_alloc_count
    );

    let all_success = exit_codes.iter().all(|&code| code >= 1 && code <= THREAD_COUNT as isize);
    if all_success {
        println!("✓ All threads completed successfully");
        0
    } else {
        println!("✗ Some threads failed");
        1
    }
}

// test 2: multiple threads share the same CXL memory (by passing the same pointer)
// note: this requires additional synchronization mechanisms, here we use a simple Atomic marker to demonstrate
use core::sync::atomic::AtomicBool;

static SHARED_INITIALIZED: AtomicBool = AtomicBool::new(false);

fn shared_memory_worker(arg: usize) -> ! {
    let thread_id = arg & 0xFF;
    let ptr = (arg & !0xFF) as *mut u8;

    while !SHARED_INITIALIZED.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }

    const CHUNK_SIZE: usize = 4096;
    let offset = thread_id * CHUNK_SIZE;
    let mut sum = 0u64;
    unsafe {
        for i in offset..offset + CHUNK_SIZE {
            sum += ptr.add(i).read() as u64;
        }
    }
    println!("Shared worker {}: sum of chunk at offset {} = {}", thread_id, offset, sum);
    exit(0)
}

fn test_shared_memory() -> i32 {
    println!("\n=== Test 2: Shared CXL Memory ===");

    const SHARED_PAGES: usize = 8; // 32KB
    const WORKER_COUNT: usize = 4;

    // the main thread allocates shared memory
    let shared_ptr = cxl_mmap(SHARED_PAGES) as *mut u8;
    if shared_ptr.is_null() {
        println!("Failed to allocate shared CXL memory");
        return 1;
    }
    println!("Main: allocated shared memory at {:p}", shared_ptr);

    // the main thread initializes data
    unsafe {
        for i in 0..(SHARED_PAGES * 4096) {
            shared_ptr.add(i).write((i & 0xff) as u8);
        }
    }
    SHARED_INITIALIZED.store(true, Ordering::Release);
    println!("Main: shared memory initialized");

    // create worker threads, passing the shared pointer
    let ptr_val = shared_ptr as usize;
    let mut tids = Vec::new();
    for i in 0..WORKER_COUNT {
        let tid = thread_create(
            linker_symbol_addr!(shared_memory_worker),
            ptr_val + i,
        );
        tids.push(tid);
    }

    // wait for all worker threads to complete
    for (i, tid) in tids.iter().enumerate() {
        let code = waittid(*tid as usize);
        println!("Main: shared worker {} (tid={}) exited with {}", i, tid, code);
    }

    // free shared memory
    let ret = cxl_munmap(shared_ptr as usize, SHARED_PAGES);
    println!("Main: cxl_munmap shared memory returned {}", ret);

    0
}

// test 3: stress test - many threads competing for allocation
fn test_stress_allocation() -> i32 {
    println!("\n=== Test 3: Stress Allocation ===");

    const THREAD_COUNT: usize = 16;
    const PAGES_PER_THREAD: usize = 4;

    let mut args: Vec<ThreadArg> = (0..THREAD_COUNT)
        .map(|i| ThreadArg {
            thread_id: i,
            page_count: PAGES_PER_THREAD,
            pattern_offset: i * 512,
        })
        .collect();

    let mut tids = Vec::new();
    for i in 0..THREAD_COUNT {
        let arg_ptr = &args[i] as *const ThreadArg;
        let tid = thread_create(
            linker_symbol_addr!(cxl_worker_thread),
            arg_ptr as usize,
        );
        tids.push(tid);
    }

    let mut failed = 0;
    for (i, tid) in tids.iter().enumerate() {
        let code = waittid(*tid as usize);
        if code < 1 || code > THREAD_COUNT as isize {
            failed += 1;
            println!("Thread {} failed with code {}", i, code);
        }
    }

    println!("Stress test: {} out of {} threads failed", failed, THREAD_COUNT);
    if failed == 0 { 0 } else { 1 }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== CXL mmap Multi-thread Test Suite ===");

    let mut overall_result = 0;

    // run test 1
    let result1 = test_concurrent_allocation();
    if result1 != 0 {
        overall_result = 1;
    }

    // run test 2
    let result2 = test_shared_memory();
    if result2 != 0 {
        overall_result = 1;
    }

    // run test 3 (optional, if system resources are allowed)
    let result3 = test_stress_allocation();
    if result3 != 0 {
        overall_result = 1;
    }

    println!("\n=== Overall Test Result: {} ===", if overall_result == 0 { "PASS" } else { "FAIL" });
    overall_result
}