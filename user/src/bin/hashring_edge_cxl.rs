#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{add_cxl_card, remove_cxl_card, cxl_route};

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== HashRing CXL Edge Cases ===\n");

    // 1. empty ring
    println!("--- 1. empty ring ---");
    let ret = cxl_route(42);
    assert!(ret < 0, "empty ring should return -1, got {}", ret);
    println!("  route(42) = {}  OK", ret);

    // 2. remove non-existent card
    println!("\n--- 2. remove non-existent ---");
    let ret = remove_cxl_card(99);
    assert!(ret < 0, "remove non-existent should fail, got {}", ret);
    println!("  remove(99) = {}  OK", ret);

    // 3. add card successfully
    println!("\n--- 3. insert card 0 ---");
    let ret = add_cxl_card(0);
    assert!(ret >= 0, "add(0) failed: {}", ret);
    println!("  add(0) = {}  OK", ret);

    let ret = cxl_route(42);
    assert!(ret == 0, "route(42) should map to card 0, got {}", ret);
    println!("  route(42) = {}  OK", ret);

    // 4. add same card again (idempotent: should not crash)
    println!("\n--- 4. duplicate insert ---");
    let ret = add_cxl_card(0);
    println!("  add(0) again = {}  (allowed, no crash)", ret);

    // 5. remove → remove again
    println!("\n--- 5. double remove ---");
    let ret = remove_cxl_card(0);
    assert!(ret == 0, "first remove(0) should succeed, got {}", ret);
    println!("  remove(0) = {}  OK", ret);

    let ret = remove_cxl_card(0);
    println!("  remove(0) again = {}  (should fail, card gone)", ret);

    let ret = cxl_route(42);
    assert!(ret < 0, "route after full removal should be -1, got {}", ret);
    println!("  route(42) = {}  OK", ret);

    // 6. insert-remove-insert cycle
    println!("\n--- 6. insert-remove-insert cycle ---");
    for cid in 0..3 {
        let r1 = add_cxl_card(cid);
        assert!(r1 >= 0, "add({}) failed: {}", cid, r1);
        let r2 = remove_cxl_card(cid);
        assert!(r2 == 0, "remove({}) failed: {}", cid, r2);
        let r3 = add_cxl_card(cid);
        assert!(r3 >= 0, "re-add({}) failed: {}", cid, r3);
        println!("  card[{}]: add={} remove={} re-add={}  OK", cid, r1, r2, r3);
    }

    // cleanup
    for cid in 0..3 { remove_cxl_card(cid); }

    // 7. out-of-range card
    println!("\n--- 7. out-of-range card ---");
    let ret = add_cxl_card(9999);
    assert!(ret < 0, "add out-of-range should fail, got {}", ret);
    println!("  add(9999) = {}  OK", ret);

    println!("\n=== hashring_edge_cxl PASSED ===");
    0
}
