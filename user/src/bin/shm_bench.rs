#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::vec::Vec;
use user_lib::{shm_alloc_page, shm_free_page, shm_gc_collect, get_time};

const ITERS: usize = 500;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== SHM Alloc/Free Benchmark ===");
    let mut pages = Vec::new();

    // ── Alloc ──
    let start = get_time();
    for _ in 0..ITERS {
        match shm_alloc_page() {
            idx if idx >= 0 => pages.push(idx as usize),
            _ => { println!("OOM"); break; }
        }
    }
    let t_alloc = get_time() - start;
    let rate_alloc = if t_alloc > 0 { ITERS as u64 * 1000 / t_alloc as u64 } else { 0 };

    // ── Free ──
    let start = get_time();
    for &idx in &pages {
        shm_free_page(idx);
    }
    shm_gc_collect();
    let t_free = get_time() - start;
    let rate_free = if t_free > 0 { ITERS as u64 * 1000 / t_free as u64 } else { 0 };

    // ── Results ──
    let total_ms = t_alloc + t_free;
    let total_ops = ITERS as u64 * 2;
    let ops_per_s = if total_ms > 0 { total_ops * 1000 / total_ms as u64 } else { 0 };
    let us_per_op = if total_ops > 0 && total_ms > 0 {
        (total_ms as u64 * 1000) / total_ops
    } else { 0 };

    println!("Iterations: {}", ITERS);
    println!("──────────────────────────────────────");
    println!("Alloc: {} ms  ({} pages/s)", t_alloc, rate_alloc);
    println!("Free:  {} ms  ({} pages/s)", t_free, rate_free);
    println!("Total: {} ms  ({} ops/s, {} us/op)", total_ms, ops_per_s, us_per_op);
    0
}
