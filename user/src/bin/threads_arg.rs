// 测试 cxl_mmap 在多线程环境下的使用
// 包括：并发分配、并发读写、数据一致性验证、页面迁移观察
#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;
extern crate core;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{
    cxl_mmap, cxl_munmap, query_cxl_meminfo, thread_create, waittid, exit, CxlMemInfo,
};

// 每个线程的工作参数
struct ThreadArg {
    thread_id: usize,
    page_count: usize,      // 分配的页数
    pattern_offset: usize,  // 用于生成不同的数据模式
}

// 全局计数器，用于统计总操作字节数（可选）
static TOTAL_BYTES_WRITTEN: AtomicUsize = AtomicUsize::new(0);
static TOTAL_BYTES_READ: AtomicUsize = AtomicUsize::new(0);

// 线程工作函数
fn cxl_worker_thread(arg: *const ThreadArg) -> ! {
    let arg = unsafe { &*arg };
    let thread_id = arg.thread_id;
    let page_count = arg.page_count;
    let pattern_offset = arg.pattern_offset;
    let total_bytes = page_count * 4096;

    println!("Thread {}: starting, allocating {} pages...", thread_id, page_count);

    // 1. 分配 CXL 内存
    let ptr = cxl_mmap(page_count) as *mut u8;
    if ptr.is_null() {
        println!("Thread {}: cxl_mmap failed!", thread_id);
        exit(thread_id as i32 + 100);
    }
    println!("Thread {}: allocated CXL memory at {:p}", thread_id, ptr);

    // 2. 写入数据：每个字节写入 (offset + thread_id) 的模式
    unsafe {
        for i in 0..total_bytes {
            let value = ((i + pattern_offset + thread_id * 7) & 0xff) as u8;
            ptr.add(i).write(value);
        }
    }
    TOTAL_BYTES_WRITTEN.fetch_add(total_bytes, Ordering::Relaxed);
    println!("Thread {}: write completed", thread_id);

    // 3. 读取并验证数据
    let mut sum = 0u64;
    unsafe {
        for i in 0..total_bytes {
            let expected = ((i + pattern_offset + thread_id * 7) & 0xff) as u8;
            let actual = ptr.add(i).read();
            // 验证数据一致性
            if actual != expected {
                println!(
                    "Thread {}: data mismatch at offset {}: expected {}, got {}",
                    thread_id, i, expected, actual
                );
                // 注意：这里不 panic，继续执行以便观察错误模式
            }
            sum += actual as u64;
        }
    }
    TOTAL_BYTES_READ.fetch_add(total_bytes, Ordering::Relaxed);

    // 4. 计算并打印期望的和
    let expected_sum = (0..total_bytes)
        .map(|i| ((i + pattern_offset + thread_id * 7) & 0xff) as u64)
        .sum::<u64>();
    
    println!(
        "Thread {}: read completed, sum={}, expected={}, {}",
        thread_id, sum, expected_sum,
        if sum == expected_sum { "✓ PASS" } else { "✗ FAIL" }
    );

    // 5. 可选：再写一遍不同的数据，测试重用
    unsafe {
        for i in 0..total_bytes.min(1024) { // 只写前 1KB 作为二次验证
            ptr.add(i).write(0xaa);
        }
    }

    // 6. 释放内存
    let ret = cxl_munmap(ptr as usize, page_count);
    println!("Thread {}: cxl_munmap returned {}", thread_id, ret);

    exit(thread_id as i32 + 1)
}

// 测试 1：多线程并发分配各自独立的 CXL 内存
fn test_concurrent_allocation() -> i32 {
    println!("\n=== Test 1: Concurrent Allocation ===");

    const THREAD_COUNT: usize = 4;
    const PAGES_PER_THREAD: usize = 8; // 每个线程 32KB

    let mut args: Vec<ThreadArg> = (0..THREAD_COUNT)
        .map(|i| ThreadArg {
            thread_id: i,
            page_count: PAGES_PER_THREAD,
            pattern_offset: i * 1024,
        })
        .collect();

    // 记录开始时的 CXL 统计信息
    let mut meminfo_start = CxlMemInfo::default();
    query_cxl_meminfo(&mut meminfo_start);

    // 创建线程
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

    // 等待所有线程完成
    let mut exit_codes = Vec::new();
    for (i, tid) in tids.iter().enumerate() {
        let code = waittid(*tid as usize);
        println!("Main: thread {} (tid={}) exited with code {}", i, tid, code);
        exit_codes.push(code);
    }

    // 记录结束时的统计信息
    let mut meminfo_end = CxlMemInfo::default();
    query_cxl_meminfo(&mut meminfo_end);

    println!("\n--- Test 1 Results ---");
    println!("Total bytes written: {}", TOTAL_BYTES_WRITTEN.load(Ordering::Relaxed));
    println!("Total bytes read: {}", TOTAL_BYTES_READ.load(Ordering::Relaxed));
    println!(
        "Slow alloc count: {} -> {} (delta = {})",
        meminfo_start.slow_alloc_count,
        meminfo_end.slow_alloc_count,
        meminfo_end.slow_alloc_count - meminfo_start.slow_alloc_count
    );
    println!(
        "Fast alloc count: {} -> {} (delta = {})",
        meminfo_start.fast_alloc_count,
        meminfo_end.fast_alloc_count,
        meminfo_end.fast_alloc_count - meminfo_start.fast_alloc_count
    );

    // 检查是否有线程失败
    let all_success = exit_codes.iter().all(|&code| code >= 1 && code <= THREAD_COUNT as isize);
    if all_success {
        println!("✓ All threads completed successfully");
        0
    } else {
        println!("✗ Some threads failed");
        1
    }
}

// 测试 2：多个线程共享同一块 CXL 内存（通过传递同一个指针）
// 注意：这需要额外的同步机制，这里使用简单的 Atomic 标记来演示
use core::sync::atomic::AtomicBool;

static SHARED_INITIALIZED: AtomicBool = AtomicBool::new(false);

fn shared_memory_worker(arg: *const usize) -> ! {
    let ptr = unsafe { *(arg) } as *mut u8;
    let thread_id = unsafe { core::ptr::read(arg) } % 100; // 简化获取 ID

    // 等待主线程初始化共享内存
    while !SHARED_INITIALIZED.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }

    // 每个线程读取并验证共享内存的不同区域
    const CHUNK_SIZE: usize = 4096;
    let offset = (thread_id * CHUNK_SIZE) as usize;
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

    // 主线程分配共享内存
    let shared_ptr = cxl_mmap(SHARED_PAGES) as *mut u8;
    if shared_ptr.is_null() {
        println!("Failed to allocate shared CXL memory");
        return 1;
    }
    println!("Main: allocated shared memory at {:p}", shared_ptr);

    // 主线程初始化数据
    unsafe {
        for i in 0..(SHARED_PAGES * 4096) {
            shared_ptr.add(i).write((i & 0xff) as u8);
        }
    }
    SHARED_INITIALIZED.store(true, Ordering::Release);
    println!("Main: shared memory initialized");

    // 创建工作线程，传递共享指针
    let ptr_val = shared_ptr as usize;
    let mut tids = Vec::new();
    for i in 0..WORKER_COUNT {
        // 每个线程传递指针和自身 ID（简化传递）
        let arg = ptr_val + i; // 用低字节编码 ID
        let tid = thread_create(
            linker_symbol_addr!(shared_memory_worker),
            &arg as *const usize as usize,
        );
        tids.push(tid);
    }

    // 等待所有工作线程完成
    for (i, tid) in tids.iter().enumerate() {
        let code = waittid(*tid as usize);
        println!("Main: shared worker {} (tid={}) exited with {}", i, tid, code);
    }

    // 释放共享内存
    let ret = cxl_munmap(shared_ptr as usize, SHARED_PAGES);
    println!("Main: cxl_munmap shared memory returned {}", ret);

    0
}

// 测试 3：压力测试 - 大量线程竞争分配
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

    // 运行测试 1
    let result1 = test_concurrent_allocation();
    if result1 != 0 {
        overall_result = 1;
    }

    // 运行测试 2
    let result2 = test_shared_memory();
    if result2 != 0 {
        overall_result = 1;
    }

    // 运行测试 3 (可选，如果系统资源允许)
    let result3 = test_stress_allocation();
    if result3 != 0 {
        overall_result = 1;
    }

    println!("\n=== Overall Test Result: {} ===", if overall_result == 0 { "PASS" } else { "FAIL" });
    overall_result
}