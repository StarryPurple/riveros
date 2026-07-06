#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{add_cxl_card, remove_cxl_card, cxl_route, CXL_CARD_COUNT};

const CARDS: usize = 6;
const KEYS: usize = 10000;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== HashRing Uniform Distribution Test ===");
    println!("  {} cards, {} keys per test", CARDS, KEYS);

    // Clean up any leftover cards from previous tests
    for cid in 0..CXL_CARD_COUNT { let _ = remove_cxl_card(cid); }

    for cid in 0..CARDS {
        assert!(add_cxl_card(cid) >= 0, "add card {}", cid);
    }

    let mut counts = [0u64; CXL_CARD_COUNT];
    for key in 0..KEYS {
        let r = cxl_route(key as u64);
        assert!(r >= 0, "route failed for key {}", key);
        counts[r as usize] += 1;
    }

    let expected = KEYS as f64 / CARDS as f64;
    println!("  expected per card: {:.1} keys", expected);

    let mut chi2 = 0.0f64;
    let mut max_dev = 0.0f64;
    let mut ok = true;

    for cid in 0..CARDS {
        let got = counts[cid] as f64;
        let dev = (got - expected).abs() / expected * 100.0;
        chi2 += (got - expected) * (got - expected) / expected;
        println!("  card[{}]: {} keys, deviation {:.1}%", cid, counts[cid], dev);
        if dev > max_dev { max_dev = dev; }
        if dev > 20.0 { ok = false; }
    }

    // chi-squared critical value for df=5 at p=0.01 is ~15.09
    println!("  chi^2 = {:.2} (critical p=0.01: ~15.1)", chi2);
    let chi_ok = chi2 < 16.0;
    if !chi_ok { ok = false; }

    for cid in 0..CARDS { remove_cxl_card(cid); }

    println!("  max deviation: {:.1}%, chi^2 OK: {}", max_dev, chi_ok);
    println!("  {}: {}", if ok { "PASS" } else { "FAIL" },
             if ok { "distribution is uniform" } else { "too skewed" });
    if ok { 0 } else { 1 }
}
