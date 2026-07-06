#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_push, cxl_tx_pop, cxl_rx_push, cxl_rx_pop,
               get_time, get_instance_id, msg_seal, msg_verify};

const REQUESTS: usize = 64;

fn make_req(id: u32, op: u8, val: u32, sender: usize) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0] = 1;
    m[1..5].copy_from_slice(&id.to_le_bytes());
    m[5] = op;
    m[6..10].copy_from_slice(&val.to_le_bytes());
    msg_seal(&mut m, sender);
    m
}
fn read_req(m: &[u8; 60]) -> (u32, u8, u32) {
    let id = u32::from_le_bytes([m[1], m[2], m[3], m[4]]);
    (id, m[5], u32::from_le_bytes([m[6], m[7], m[8], m[9]]))
}
fn make_rsp(id: u32, result: u32, sender: usize) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0] = 2;
    m[1..5].copy_from_slice(&id.to_le_bytes());
    m[5..9].copy_from_slice(&result.to_le_bytes());
    msg_seal(&mut m, sender);
    m
}
fn read_rsp(m: &[u8; 60]) -> (u32, u32) {
    let id = u32::from_le_bytes([m[1], m[2], m[3], m[4]]);
    (id, u32::from_le_bytes([m[5], m[6], m[7], m[8]]))
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let me = get_instance_id();
    if me == 0 {
        run_master()
    } else {
        run_worker()
    }
}

fn run_master() -> i32 {
    println!("=== Async RPC — Node 0 (Master) ===");
    println!("  {} parallel requests, no per-req wait", REQUESTS);

    // Send all requests at once
    let t0 = get_time();
    for i in 0..REQUESTS {
        let id = i as u32;
        let op: u8 = ((i % 4 + 1) as u8); // op 1=+,2=-,3=*,4=/
        let val = (id.wrapping_mul(13).wrapping_add(7)) as u32;
        let req = make_req(id, op, val, 0);
        while cxl_tx_push(&req) != 0 {}
    }

    // Send DONE marker
    let mut done = [0u8; 60];
    done[0] = 0xFF;
    msg_seal(&mut done, 0);
    while cxl_tx_push(&done) != 0 {}

    // Collect all responses (order may differ from requests — async)
    let mut results = [0u32; REQUESTS];
    let mut received = [false; REQUESTS];
    let mut count = 0usize;
    while count < REQUESTS {
        let mut m = [0u8; 60];
        if cxl_rx_pop(&mut m) == 0 {
            assert!(msg_verify(&m).is_some(), "checksum");
            if m[0] == 0xFE { continue; } // may get stray DONE echo, ignore
            let (id, result) = read_rsp(&m);
            let idx = id as usize;
            if idx < REQUESTS && !received[idx] {
                results[idx] = result;
                received[idx] = true;
                count += 1;
            }
        }
    }
    let ms_total = get_time() - t0;

    // Verify results
    let mut errors = 0usize;
    for i in 0..REQUESTS {
        let id = i as u32;
        let op: u8 = ((i % 4 + 1) as u8);
        let val = (id.wrapping_mul(13).wrapping_add(7)) as u32;
        let expected = compute(op, val, id);
        if results[i] != expected {
            if errors < 3 { println!("  req[{}]: op={} val={} exp={} got={}", i, op, val, expected, results[i]); }
            errors += 1;
        }
    }
    let ok = errors == 0 && count == REQUESTS;
    println!("  {} ms, {} responses: {}", ms_total, count, if ok { "PASS" } else { "FAIL" });
    if ok { 0 } else { 1 }
}

fn run_worker() -> i32 {
    println!("=== Async RPC — Node 1 (Worker) ===");

    let mut processed = 0usize;
    loop {
        let mut m = [0u8; 60];
        if cxl_tx_pop(&mut m) == 0 {
            assert!(msg_verify(&m).is_some(), "checksum");
            if m[0] == 0xFF { break; }
            let (id, op, val) = read_req(&m);
            let result = compute(op, val, id);
            let rsp = make_rsp(id, result, 1);
            while cxl_rx_push(&rsp) != 0 {}
            processed += 1;
        }
    }
    println!("  worker processed {} requests", processed);
    0
}

fn compute(op: u8, a: u32, id: u32) -> u32 {
    match op {
        1 => a.wrapping_add(id),
        2 => a.wrapping_sub(id),
        3 => a.wrapping_mul(id.wrapping_add(1)),
        4 => if id.wrapping_add(1) != 0 { a / (id.wrapping_add(1)) } else { 0 },
        _ => a,
    }
}
