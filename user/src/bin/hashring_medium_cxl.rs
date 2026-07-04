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

const CARD_COUNT: usize = 8;
const PAGES_PER_ALLOC: usize = 40;
const ROUTE_TEST_KEYS: u64 = 1024;

fn print_meminfo(label: &str, info: &CxlMemInfo) {
    let total_alloc: u64 = info.slow_alloc_count.iter().sum();
    let total_dealloc: u64 = info.slow_dealloc_count.iter().sum();
    println!("[{}]", label);
    println!("  fast_alloc={} fast_dealloc={}", info.fast_alloc_count, info.fast_dealloc_count);
    println!("  slow_alloc_total={} slow_dealloc_total={}", total_alloc, total_dealloc);
    for i in 0..CARD_COUNT {
        if info.slow_alloc_count[i] > 0 || info.slow_dealloc_count[i] > 0 {
            println!("  card[{}]: alloc={} dealloc={}", i, info.slow_alloc_count[i], info.slow_dealloc_count[i]);
        }
    }
}

fn test_route_distribution(label: &str, expected_cards: &[usize]) {
    println!("  [{}] route distribution:", label);
    let mut counts = [0u64; user_lib::CXL_CARD_COUNT];
    let mut unmatched = 0;
    for i in 0..ROUTE_TEST_KEYS {
        let key = i * 64 + 37;
        let ret = cxl_route(key);
        if ret < 0 {
            unmatched += 1;
        } else {
            counts[ret as usize] += 1;
        }
    }
    for &cid in expected_cards {
        println!("    card[{}]: {} / {}", cid, counts[cid], ROUTE_TEST_KEYS);
    }
    if unmatched > 0 {
        println!("    unmatched: {}", unmatched);
    }
}

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

fn verify_data(ptr: *mut u8, pages: usize, prefix: u8) {
    let size = pages * 4096;
    unsafe {
        for i in 0..size {
            let expected = prefix.wrapping_add((i & 0xff) as u8);
            let actual = ptr.add(i).read();
            if actual != expected {
                println!("data mismatch at offset {}: expected {:#x}, got {:#x}", i, expected, actual);
                assert!(false, "data corruption detected!");
            }
        }
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let mut mem = CxlMemInfo::default();
    println!("=== HashRing CXL Medium-Scale Test ===\n");

    query_cxl_meminfo(&mut mem);
    print_meminfo("initial", &mem);

    // ── Phase 1: add 8 cards ──
    println!("\n--- Phase 1: add {} cards ---", CARD_COUNT);
    for cid in 0..CARD_COUNT {
        let ret = add_cxl_card(cid);
        assert!(ret >= 0, "add_cxl_card({}) failed: {}", cid, ret);
    }
    println!("  all {} cards added", CARD_COUNT);

    test_route_distribution("after add 8 cards", &[0, 1, 2, 3, 4, 5, 6, 7]);

    // ── Phase 2: allocate data across cards ──
    println!("\n--- Phase 2: multi-block allocation ---");
    let mut bufs: [(*mut u8, usize); 8] = [(core::ptr::null_mut(), 0); 8];
    for i in 0..8 {
        bufs[i] = alloc_and_write((i * 10 + 10) as u8);
        println!("  cxl_mmap #{}: ptr={:p}, pages={}", i + 1, bufs[i].0, bufs[i].1);
    }

    query_cxl_meminfo(&mut mem);
    print_meminfo("after 8 x cxl_mmap", &mem);

    let total_pages = (PAGES_PER_ALLOC * 8) as u64;
    let avg = total_pages / CARD_COUNT as u64;
    println!("  total pages allocated: {}, avg per card: ~{}", total_pages, avg);

    // verify all data
    for (i, &(ptr, n)) in bufs.iter().enumerate() {
        verify_data(ptr, n, (i * 10 + 10) as u8);
    }
    println!("  data integrity: all 4 blocks OK");

    // ── Phase 3: remove a card that has data, verify re-route ──
    println!("\n--- Phase 3: remove a busy card ---");
    query_cxl_meminfo(&mut mem);

    // pick the card with the most allocations (guaranteed to have data)
    let mut victim_cid = 0usize;
    let mut max_alloc = 0u64;
    for cid in 0..CARD_COUNT {
        if mem.slow_alloc_count[cid] > max_alloc {
            max_alloc = mem.slow_alloc_count[cid];
            victim_cid = cid;
        }
    }
    assert!(max_alloc > 0, "no card has data — test setup problem");
    println!("  removing card {} (alloc={})", victim_cid, max_alloc);

    let before_alloc = mem.slow_alloc_count;
    let ret = remove_cxl_card(victim_cid);
    assert!(ret == 0, "remove_cxl_card({}) failed: {}", victim_cid, ret);

    query_cxl_meminfo(&mut mem);
    println!("  card[{}] after removal: alloc={} dealloc={}",
        victim_cid, mem.slow_alloc_count[victim_cid], mem.slow_dealloc_count[victim_cid]);
    assert!(mem.slow_dealloc_count[victim_cid] >= before_alloc[victim_cid],
        "card[{}] pages not fully deallocated: {} < {}",
        victim_cid, mem.slow_dealloc_count[victim_cid], before_alloc[victim_cid]);

    // data survives (re-routed or promoted)
    for (i, &(ptr, n)) in bufs.iter().enumerate() {
        verify_data(ptr, n, (i * 10 + 10) as u8);
    }
    println!("  data integrity after remove card {}: all OK", victim_cid);

    // new allocation must not go to the removed card
    let (ptr5, n5) = alloc_and_write(0x50);
    println!("  cxl_mmap #5: ptr={:p}, pages={}", ptr5, n5);
    query_cxl_meminfo(&mut mem);
    let card_grew = mem.slow_alloc_count[victim_cid] > before_alloc[victim_cid];
    assert!(!card_grew, "removed card {} received new allocations", victim_cid);
    println!("  verify: card {} received no new allocs", victim_cid);

    // route must not return the removed card
    for i in 0..ROUTE_TEST_KEYS {
        let key = i * 64 + 37;
        let ret = cxl_route(key) as usize;
        assert_ne!(ret, victim_cid,
            "key {} still routes to removed card {}", key, victim_cid);
    }
    println!("  verify: no keys route to card {}", victim_cid);

    // ── Phase 4: remove all remaining cards ──
    println!("\n--- Phase 4: remove all cards ---");
    for cid in 0..CARD_COUNT {
        if cid == victim_cid { continue; }
        let ret = remove_cxl_card(cid);
        assert!(ret == 0, "remove_cxl_card({}) failed: {}", cid, ret);
    }
    println!("  all cards removed");

    // all data intact after full removal
    for (i, &(ptr, n)) in bufs.iter().enumerate() {
        verify_data(ptr, n, (i * 10 + 10) as u8);
    }
    assert!(!ptr5.is_null());
    verify_data(ptr5, n5, 0x50);
    println!("  data integrity after full removal: all OK");

    // empty ring check
    let ret = cxl_route(42);
    assert!(ret < 0, "route should return -1 on empty ring, got {}", ret);
    println!("  empty ring: {}", ret);

    // ── Phase 5: final stats ──
    println!("\n--- Phase 5: final stats ---");
    query_cxl_meminfo(&mut mem);
    print_meminfo("final", &mem);

    // cleanup
    for &(ptr, n) in bufs.iter() {
        if !ptr.is_null() {
            cxl_munmap(ptr as usize, n * 4096);
        }
    }
    cxl_munmap(ptr5 as usize, n5 * 4096);

    println!("\n=== hashring_medium_cxl test PASSED ===");
    0
}
