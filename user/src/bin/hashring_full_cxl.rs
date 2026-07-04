#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::{
    CxlMemInfo, query_cxl_meminfo,
    add_cxl_card, remove_cxl_card, cxl_route,
    cxl_mmap, cxl_munmap,
    fork, waitpid, exit,
};

const FULL_CARDS: usize = 6; // use 6 cards for this test (keeps mgmt overhead low)
const PAGES_PER_ALLOC: usize = 30;
const ROUTE_KEYS: u64 = 4096;

fn print_meminfo(label: &str, info: &CxlMemInfo) {
    let total_alloc: u64 = info.slow_alloc_count.iter().sum();
    let total_dealloc: u64 = info.slow_dealloc_count.iter().sum();
    println!("[{}]  total_alloc={} total_dealloc={} promote={} demote={}",
        label, total_alloc, total_dealloc, info.promote_count, info.demote_count);
    for i in 0..FULL_CARDS {
        if info.slow_alloc_count[i] > 0 || info.slow_dealloc_count[i] > 0 {
            print!(" card[{}]: {}a/{}d", i, info.slow_alloc_count[i], info.slow_dealloc_count[i]);
        }
    }
}

fn test_route(label: &str, cards: &[u64]) {
    let mut counts = [0u64; 32];
    for i in 0..ROUTE_KEYS {
        let key = i * 64 + 37;
        let ret = cxl_route(key);
        if ret >= 0 { counts[ret as usize] += 1; }
    }
    print!("  [{}] keys={}", label, ROUTE_KEYS);
    for &c in cards {
        print!("  card[{}]={}", c, counts[c as usize]);
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

fn verify(ptr: *mut u8, pages: usize, prefix: u8) {
    unsafe {
        for i in 0..(pages * 4096) {
            let expected = prefix.wrapping_add((i & 0xff) as u8);
            let actual = ptr.add(i).read();
            assert_eq!(actual, expected, "corruption at offset {}", i);
        }
    }
}

fn alloc_block(prefix: u8) -> (*mut u8, usize) {
    alloc_and_write(prefix)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let mut mem = CxlMemInfo::default();
    println!("=== HashRing CXL Full Test ===\n");

    query_cxl_meminfo(&mut mem);
    print_meminfo("initial", &mem);

    // ─── Phase 1: add FULL_CARDS ───
    println!("\n--- Phase 1: add {} cards ---", FULL_CARDS);
    for cid in 0..FULL_CARDS {
        let ret = add_cxl_card(cid);
        assert!(ret >= 0, "add({}) failed: {}", cid, ret);
    }
    println!("  all {} cards added", FULL_CARDS);

    let exp: [u64; FULL_CARDS] = core::array::from_fn(|i| i as u64);
    test_route("distribution", &exp);

    // ─── Phase 2: allocate data ───
    println!("\n--- Phase 2: allocate ---");
    let mut bufs1: [(*mut u8, usize); 4] = [(core::ptr::null_mut(), 0); 4];
    for i in 0..4 {
        bufs1[i] = alloc_block(0x10 + i as u8);
        println!("  mmap#{}: {:p} {}p", i+1, bufs1[i].0, bufs1[i].1);
    }

    query_cxl_meminfo(&mut mem);
    print_meminfo("after alloc", &mem);

    for (i, &(p, n)) in bufs1.iter().enumerate() {
        verify(p, n, 0x10 + i as u8);
    }
    println!("  data integrity: OK");

    // ─── Phase 3: remove a card that has data ───
    println!("\n--- Phase 3: remove busy card ---");
    let victim = {
        // find card with most allocs (guaranteed data)
        let mut best = 0usize;
        let mut max_a = 0u64;
        for cid in 0..FULL_CARDS {
            if mem.slow_alloc_count[cid] > max_a {
                max_a = mem.slow_alloc_count[cid];
                best = cid;
            }
        }
        best
    };
    println!("  removing card {} (alloc={})", victim, mem.slow_alloc_count[victim]);

    let alloc_before = mem.slow_alloc_count;
    let ret = remove_cxl_card(victim);
    assert!(ret == 0, "remove({}) failed: {}", victim, ret);

    query_cxl_meminfo(&mut mem);
    assert!(mem.slow_dealloc_count[victim] >= alloc_before[victim],
        "card {} not fully deallocated: {} < {}",
        victim, mem.slow_dealloc_count[victim], alloc_before[victim]);
    println!("  dealloc={} >= alloc_before={}  OK", mem.slow_dealloc_count[victim], alloc_before[victim]);

    for (i, &(p, n)) in bufs1.iter().enumerate() {
        verify(p, n, 0x10 + i as u8);
    }
    println!("  data integrity: OK");

    // route must not return removed card
    for i in 0..ROUTE_KEYS {
        let key = i * 64 + 37;
        if cxl_route(key) as usize == victim {
            panic!("key {} still routes to removed card {}", key, victim);
        }
    }
    println!("  route bypasses card {}: OK", victim);

    // ─── Phase 4: re-add the card (triggers rehash) ───
    println!("\n--- Phase 4: re-add card {} ---", victim);
    let ret = add_cxl_card(victim);
    assert!(ret >= 0, "re-add({}) failed: {}", victim, ret);
    println!("  re-added card {}", victim);

    query_cxl_meminfo(&mut mem);
    print_meminfo("after re-add", &mem);

    if mem.slow_alloc_count[victim] > 0 {
        println!("  rehash migrated {} pages to card {}", mem.slow_alloc_count[victim], victim);
    } else {
        println!("  (no pages migrated -- card was empty at removal time)");
    }

    for (i, &(p, n)) in bufs1.iter().enumerate() {
        verify(p, n, 0x10 + i as u8);
    }
    println!("  data integrity: OK");

    // ─── Phase 5: cross-process (fork) ───
    println!("\n--- Phase 5: cross-process ---");
    let pid = fork();
    if pid == 0 {
        // child: allocate CXL pages and write data, then exit
        let extra = alloc_block(0x50);
        verify(extra.0, extra.1, 0x50);
        cxl_munmap(extra.0 as usize, extra.1 * 4096);
        exit(0);
    }
    let mut code = 0i32;
    waitpid(pid as usize, &mut code);
    println!("  child exited with code {}", code);

    // Child's allocations and deallocations are reflected in stats
    query_cxl_meminfo(&mut mem);
    print_meminfo("after cross-process", &mem);

    // ─── Phase 6: remove ALL cards ───
    println!("\n--- Phase 6: remove all cards ---");
    for cid in 0..FULL_CARDS {
        let r = remove_cxl_card(cid);
        if r != 0 { println!("  remove({}) returned {} (already removed?)", cid, r); }
    }
    println!("  all removed");

    // Data must still be accessible (promoted to DRAM)
    for (i, &(p, n)) in bufs1.iter().enumerate() {
        verify(p, n, 0x10 + i as u8);
    }
    println!("  data integrity after full removal: OK");

    let ret = cxl_route(42);
    assert!(ret < 0, "empty ring should return -1, got {}", ret);
    println!("  empty ring: {}  OK", ret);

    query_cxl_meminfo(&mut mem);
    print_meminfo("final", &mem);

    // Assert alloc == dealloc for all cards
    for cid in 0..FULL_CARDS {
        assert_eq!(mem.slow_alloc_count[cid], mem.slow_dealloc_count[cid],
            "card {} alloc({}) != dealloc({}) -- leak!",
            cid, mem.slow_alloc_count[cid], mem.slow_dealloc_count[cid]);
    }
    println!("  alloc==dealloc for all cards: OK  (no leak)");

    // cleanup
    for &(p, n) in bufs1.iter() { cxl_munmap(p as usize, n * 4096); }

    println!("\n=== hashring_full_cxl PASSED ===");
    0
}
