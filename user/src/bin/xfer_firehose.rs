#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_push, cxl_tx_pop, cxl_rx_push, cxl_rx_pop,
               get_time, get_instance_id, msg_seal, msg_verify};

const BURST: usize = 500;

fn make_msg(seq: u32, payload: u64, sender: usize) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0..4].copy_from_slice(&seq.to_le_bytes());
    m[4..12].copy_from_slice(&payload.to_le_bytes());
    msg_seal(&mut m, sender);
    m
}
fn read_msg(m: &[u8; 60]) -> (u32, u64) {
    assert!(msg_verify(m).is_some(), "checksum");
    let seq = u32::from_le_bytes([m[0], m[1], m[2], m[3]]);
    let payload = u64::from_le_bytes([m[4], m[5], m[6], m[7], m[8], m[9], m[10], m[11]]);
    (seq, payload)
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
    println!("=== Firehose Unidirectional Push — Node 0 (Master) ===");
    println!("  burst: {} messages, no per-msg reply", BURST);

    let t0 = get_time();
    for i in 0..BURST {
        let payload = (i as u64).wrapping_mul(0x9E3779B97F4A7C15);
        let msg = make_msg(i as u32, payload, 0);
        while cxl_tx_push(&msg) != 0 {}
    }
    let t_push = get_time() - t0;

    // Send DONE marker
    let mut done = [0u8; 60];
    done[0] = 0xFF;
    msg_seal(&mut done, 0);
    while cxl_tx_push(&done) != 0 {}

    // Wait for worker's summary
    let mut reply = [0u8; 60];
    loop {
        if cxl_rx_pop(&mut reply) == 0 {
            assert!(msg_verify(&reply).is_some(), "checksum");
            if reply[0] == 0xFE { break; }
        }
    }
    let got = u32::from_le_bytes([reply[1], reply[2], reply[3], reply[4]]) as usize;
    let errs = u32::from_le_bytes([reply[5], reply[6], reply[7], reply[8]]) as usize;

    let ok = got == BURST && errs == 0;
    let ms_total = get_time() - t0;
    let rate = if ms_total > 0 { BURST as u64 * 1000 / ms_total as u64 } else { 0 };
    println!("  push: {} ms, total: {} ms ({} msgs/s)", t_push, ms_total, rate);
    println!("  worker: got={} errs={}: {}", got, errs, if ok { "PASS" } else { "FAIL" });
    if ok { 0 } else { 1 }
}

fn run_worker() -> i32 {
    println!("=== Firehose Unidirectional Push — Node 1 (Worker) ===");

    let mut got = 0usize;
    let mut errors = 0usize;
    let mut last_seq: u32 = 0;
    let mut first = true;

    loop {
        let mut m = [0u8; 60];
        if cxl_tx_pop(&mut m) == 0 {
            assert!(msg_verify(&m).is_some(), "checksum");
            if m[0] == 0xFF { break; } // DONE

            let (seq, payload) = read_msg(&m);
            let expected = (seq as u64).wrapping_mul(0x9E3779B97F4A7C15);
            if payload != expected { errors += 1; }

            if !first && seq != last_seq + 1 { errors += 1; }
            first = false;
            last_seq = seq;
            got += 1;
        }
    }

    // Send summary back to master
    let mut sum = [0u8; 60];
    sum[0] = 0xFE;
    sum[1..5].copy_from_slice(&(got as u32).to_le_bytes());
    sum[5..9].copy_from_slice(&(errors as u32).to_le_bytes());
    msg_seal(&mut sum, 1);
    while cxl_rx_push(&sum) != 0 {}

    println!("  worker got {} messages, {} errors", got, errors);
    0
}
