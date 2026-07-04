#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::{
    CxlMemInfo, query_cxl_meminfo,
    add_cxl_card, remove_cxl_card, cxl_route,
    cxl_mmap, cxl_munmap,
};

const PAGES_PER_ALLOC: usize = 30;
const ROUTE_TEST_KEYS: u64 = 256;

fn print_meminfo(label: &str, info: &CxlMemInfo) {
    let total_slow: u64 = info.slow_alloc_count.iter().sum();
    println!("[{}]", label);
    println!("  fast_alloc={} fast_dealloc={}", info.fast_alloc_count, info.fast_dealloc_count);
    println!("  slow_alloc_total={} slow_dealloc_total={}", total_slow, info.slow_dealloc_count.iter().sum::<u64>());
    for i in 0..user_lib::CXL_CARD_COUNT {
        if info.slow_alloc_count[i] > 0 || info.slow_dealloc_count[i] > 0 {
            println!("  card[{}]: alloc={} dealloc={}", i, info.slow_alloc_count[i], info.slow_dealloc_count[i]);
        }
    }
}

fn test_route_distribution(label: &str, expected_cards: &[usize]) {
    println!("  [{}] testing route distribution", label);
    let mut counts = [0u64; user_lib::CXL_CARD_COUNT];
    let mut unmatched = 0;
    for key in 0..ROUTE_TEST_KEYS {
        let ret = cxl_route(key);
        if ret < 0 {
            unmatched += 1;
        } else {
            counts[ret as usize] += 1;
        }
    }
    for &cid in expected_cards {
        println!("  [{}] card[{}]: {} / {}", label, cid, counts[cid], ROUTE_TEST_KEYS);
    }
    if unmatched > 0 {
        println!("  [{}] unmatched keys: {}", label, unmatched);
    }
}

/// allocate a CXL memory block, write data, return (ptr, page_count)
fn alloc_and_write(prefix: u8) -> (*mut u8, usize) {
    let size = PAGES_PER_ALLOC * 4096;
    let ptr = cxl_mmap(size) as *mut u8;
    assert!(!ptr.is_null(), "cxl_mmap failed");
    unsafe {
        for i in 0..size {
            ptr.add(i).write(prefix.wrapping_add((i & 0xff) as u8));
        }
    }
    (ptr, PAGES_PER_ALLOC)
}

/// verify the data written before is still correct
fn verify_data(ptr: *mut u8, pages: usize, prefix: u8) {
    let size = pages * 4096;
    let mut ok = true;
    unsafe {
        for i in 0..size {
            let expected = prefix.wrapping_add((i & 0xff) as u8);
            let actual = ptr.add(i).read();
            if actual != expected {
                if ok {
                    println!("data mismatch at offset {}: expected {:#x}, got {:#x}", i, expected, actual);
                }
                ok = false;
            }
        }
    }
    assert!(ok, "data corruption detected!");
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let mut mem = CxlMemInfo::default();
    println!("=== HashRing CXL End-to-End Test ===");

    // Phase 0: initial state
    query_cxl_meminfo(&mut mem);
    print_meminfo("initial", &mem);

    // Phase 1: add 3 cards, test routing + allocation
    println!("\n--- Phase 1: add 3 cards ---");
    for cid in 0..3 {
        let ret = add_cxl_card(cid);
        assert!(ret >= 0, "add_cxl_card({}) failed: {}", cid, ret);
        println!("  add_cxl_card({}) = {}", cid, ret);
    }

    test_route_distribution("after add 3 cards", &[0, 1, 2]);

    // Allocate 2 blocks (60 pages total), should distribute across cards
    let (buf1, n1) = alloc_and_write(0x10);
    println!("  cxl_mmap #1: ptr={:p}, pages={}", buf1, n1);
    let (buf2, n2) = alloc_and_write(0x20);
    println!("  cxl_mmap #2: ptr={:p}, pages={}", buf2, n2);

    query_cxl_meminfo(&mut mem);
    print_meminfo("after 2 x cxl_mmap", &mem);
    // Each card should have ~20 pages allocated (60 / 3 = 20)
    for cid in 0..3 {
        let avg = (n1 + n2) as u64 / 3;
        let tol = (n1 + n2) as u64 / 2;
        assert!(
            mem.slow_alloc_count[cid] <= avg + tol,
            "card[{}] has {} slow allocs, expected ~{}",
            cid, mem.slow_alloc_count[cid], avg
        );
    }

    // Verify both buffers are intact
    verify_data(buf1, n1, 0x10);
    verify_data(buf2, n2, 0x20);
    println!("  data integrity check #1 passed");

    // Phase 2: remove card 1, verify data survives
    println!("\n--- Phase 2: remove card 1 ---");
    query_cxl_meminfo(&mut mem);
    let alloc_before = mem.slow_alloc_count;
    let _dealloc_before = mem.slow_dealloc_count;
    let ret = remove_cxl_card(1);
    assert!(ret == 0, "remove_cxl_card(1) failed: {}", ret);
    println!("  remove_cxl_card(1) = {}", ret);

    query_cxl_meminfo(&mut mem);
    print_meminfo("after remove card 1", &mem);
    // card[1] should have dealloc_count increased (pages promoted to DRAM)
    assert!(
        mem.slow_dealloc_count[1] >= alloc_before[1],
        "card[1] dealloc {} < alloc {}, pages not fully deallocated",
        mem.slow_dealloc_count[1], alloc_before[1]
    );
    // Data must still be accessible (promoted to DRAM)
    verify_data(buf1, n1, 0x10);
    verify_data(buf2, n2, 0x20);
    println!("  data integrity after remove-card-1: passed");

    // Route must not return card 1
    test_route_distribution("after remove card 1", &[0, 2]);
    for key in 0..ROUTE_TEST_KEYS {
        assert_ne!(cxl_route(key), 1, "key {} still routes to removed card 1", key);
    }
    println!("  verify: no keys to card 1 -- passed");

    // New allocation must not go to card 1
    let (buf3, n3) = alloc_and_write(0x30);
    println!("  cxl_mmap #3: ptr={:p}, pages={}", buf3, n3);
    query_cxl_meminfo(&mut mem);
    let card1_grew = mem.slow_alloc_count[1] > alloc_before[1];
    assert!(!card1_grew, "card 1 received new allocations after removal");
    println!("  verify: card 1 no new allocs -- passed");
    verify_data(buf3, n3, 0x30);

    // Phase 3: add card 1 back
    println!("\n--- Phase 3: add card 1 back ---");
    let ret = add_cxl_card(1);
    assert!(ret >= 0, "add_cxl_card(1) failed: {}", ret);
    println!("  add_cxl_card(1) = {}", ret);

    test_route_distribution("after re-add card 1", &[0, 1, 2]);

    query_cxl_meminfo(&mut mem);
    print_meminfo("after re-add card 1", &mem);

    // Existing data must be intact
    verify_data(buf1, n1, 0x10);
    verify_data(buf2, n2, 0x20);
    verify_data(buf3, n3, 0x30);
    println!("  data integrity after re-add: passed");

    // New allocations use all 3 cards
    let (buf4, n4) = alloc_and_write(0x40);
    println!("  cxl_mmap #4: ptr={:p}, pages={}", buf4, n4);
    verify_data(buf4, n4, 0x40);

    // Phase 4: remove all cards
    println!("\n--- Phase 4: remove all cards ---");
    for cid in 0..3 {
        let ret = remove_cxl_card(cid);
        assert!(ret == 0, "remove_cxl_card({}) failed: {}", cid, ret);
        println!("  remove_cxl_card({}) = {}", cid, ret);
    }

    // All data must still be accessible (pages promoted to DRAM)
    verify_data(buf1, n1, 0x10);
    verify_data(buf2, n2, 0x20);
    verify_data(buf3, n3, 0x30);
    verify_data(buf4, n4, 0x40);
    println!("  data integrity after full removal: passed");

    // Empty ring
    let ret = cxl_route(42);
    assert!(ret < 0, "route should return -1 on empty ring, got {}", ret);
    println!("  verify: empty ring returns {} -- passed", ret);

    // Phase 5: final meminfo
    println!("\n--- Phase 5: final stats ---");
    query_cxl_meminfo(&mut mem);
    print_meminfo("final", &mem);

    // Cleanup: munmap all buffers
    cxl_munmap(buf1 as usize, n1 * 4096);
    cxl_munmap(buf2 as usize, n2 * 4096);
    cxl_munmap(buf3 as usize, n3 * 4096);
    cxl_munmap(buf4 as usize, n4 * 4096);

    println!("\n=== hashring_cxl test PASSED ===");
    0
}
