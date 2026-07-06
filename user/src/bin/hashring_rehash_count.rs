#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{add_cxl_card, remove_cxl_card, cxl_mmap, cxl_munmap,
               CxlMemInfo, query_cxl_meminfo};

const CARDS: usize = 6;
const ALLOCS: usize = 12;
const PAGES: usize = 10;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== HashRing Rehash Migration Count ===");
    println!("  {} cards, {} allocs × {} pages", CARDS, ALLOCS, PAGES);

    // Clean up any leftover cards from previous tests
    for cid in 0..user_lib::CXL_CARD_COUNT { let _ = remove_cxl_card(cid); }

    // Register all cards
    for cid in 0..CARDS {
        assert!(add_cxl_card(cid) >= 0, "add card {}", cid);
    }

    // Allocate data blocks (distributed across cards)
    let mut ptrs = [(0usize, 0usize); ALLOCS];
    for i in 0..ALLOCS {
        let va = cxl_mmap(PAGES * 4096);
        assert!(va > 0, "mmap failed at alloc {}", i);
        let p = va as *mut u8;
        let prefix = (i as u8).wrapping_mul(0x30);
        unsafe {
            for off in 0..(PAGES * 4096) {
                p.add(off).write(prefix);
            }
        }
        ptrs[i] = (va as usize, PAGES);
    }
    println!("  allocated {} blocks", ALLOCS);

    // Record pre-removal stats
    let mut pre = CxlMemInfo::default();
    query_cxl_meminfo(&mut pre);
    let pre_total: u64 = pre.slow_alloc_count.iter().sum();
    let pre_card1: u64 = pre.slow_alloc_count[1];
    println!("  before removal: total={}, card[1]={}", pre_total, pre_card1);

    // Remove card 1
    assert!(remove_cxl_card(1) == 0, "remove card 1");

    let mut post = CxlMemInfo::default();
    query_cxl_meminfo(&mut post);

    // Card 1 should have deallocated ALL its pages
    let card1_dealloc = post.slow_dealloc_count[1];
    println!("  card[1] dealloc'd: {} pages", card1_dealloc);
    assert!(card1_dealloc >= pre_card1,
        "card[1] dealloc {} < alloc {}, pages lost",
        card1_dealloc, pre_card1);

    // Pages from card 1 should have migrated to remaining 5 cards
    // Expected: card 1 had ~pre_total/6 pages. These migrate to cards 0,2-5.
    // Each remaining card should see ~pre_card1/5 pages added.
    let expected_per_remaining = pre_card1 as f64 / (CARDS - 1) as f64;
    println!("  expected migration per remaining card: ~{:.1} pages", expected_per_remaining);

    let mut rehash_ok = true;
    for cid in 0..CARDS {
        if cid == 1 { continue; }
        let post_alloc = post.slow_alloc_count[cid];
        let pre_alloc = pre.slow_alloc_count[cid];
        let new_pages = post_alloc.saturating_sub(pre_alloc);
        // The remaining cards may have received pages from card 1's data
        // Their dealloc counts won't change since data was just re-parented
        println!("  card[{}]: pre_alloc={}, post_alloc={}, new={}",
                 cid, pre_alloc, post_alloc, new_pages);
        // Accept a wide tolerance — exact numbers depend on hash distribution
        if new_pages > expected_per_remaining as u64 * 3 {
            rehash_ok = false;
        }
    }

    // Verify all data still intact
    let mut data_ok = true;
    for i in 0..ALLOCS {
        let p = ptrs[i].0 as *mut u8;
        let n = ptrs[i].1;
        let prefix = (i as u8).wrapping_mul(0x30);
        unsafe {
            for off in 0..(n * 4096) {
                if p.add(off).read() != prefix {
                    println!("  block {}: corruption at offset {}", i, off);
                    data_ok = false;
                    break;
                }
            }
            if !data_ok { break; }
        }
    }
    println!("  data integrity: {}", if data_ok { "OK" } else { "FAIL" });

    // Re-add card 1 — should trigger rehash migrating some pages back
    assert!(add_cxl_card(1) >= 0, "re-add card 1");
    let mut readd = CxlMemInfo::default();
    query_cxl_meminfo(&mut readd);
    let card1_realloc = readd.slow_alloc_count[1];
    println!("  after re-add: card[1] alloc={} (rehash migration)", card1_realloc);
    // After re-add, some fraction of pages should be on card 1 again
    let migration_happened = card1_realloc > 0 || pre_card1 == 0;
    println!("  migration to re-added card: {}", if migration_happened { "detected" } else { "none (card was empty?)" });

    // Verify data AGAIN after re-add
    for i in 0..ALLOCS {
        let p = ptrs[i].0 as *mut u8;
        let n = ptrs[i].1;
        let prefix = (i as u8).wrapping_mul(0x30);
        unsafe {
            for off in 0..(n * 4096) {
                if p.add(off).read() != prefix {
                    println!("  block {} (post re-add): corruption at offset {}", i, off);
                    data_ok = false;
                    break;
                }
            }
            if !data_ok { break; }
        }
    }
    println!("  data integrity after re-add: {}", if data_ok { "OK" } else { "FAIL" });

    // Cleanup
    for cid in 0..CARDS { remove_cxl_card(cid); }
    for &(va, n) in &ptrs { cxl_munmap(va, n * 4096); }

    let ok = data_ok && rehash_ok;
    println!("  {}", if ok { "PASS" } else { "FAIL" });
    if ok { 0 } else { 1 }
}
