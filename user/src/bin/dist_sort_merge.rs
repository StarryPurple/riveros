#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_push, cxl_rx_pop, cxl_tx_pop, cxl_rx_push,
               get_time, get_instance_id, msg_seal, msg_verify};

const TAG_BATCH:  u8 = 1;
const TAG_DATA:   u8 = 2;
const TAG_RESULT: u8 = 3;
const TAG_ACK:    u8 = 4;
const TAG_DONE:   u8 = 9;

const PER_MSG: usize = 7; // each msg carries up to 7 u64 values
const BATCHES: usize = 8;
const BATCH_SIZE: usize = 256; // numbers per batch
const TOTAL: usize = BATCHES * BATCH_SIZE; // 1024 numbers total
const GEN_SEED: u64 = 0x5EED_5EED;

fn make_msg(tag: u8, vals: &[u64], sender: usize) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0] = tag;
    let n = vals.len().min(7);
    for i in 0..n {
        m[1 + i*8..9 + i*8].copy_from_slice(&vals[i].to_le_bytes());
    }
    msg_seal(&mut m, sender);
    m
}
fn read_msg(m: &[u8; 60]) -> (u8, [u64; 7]) {
    assert!(msg_verify(m).is_some(), "checksum");
    let mut vals = [0u64; 7];
    for i in 0..7 {
        let mut b = [0u8; 8];
        b.copy_from_slice(&m[1 + i*8..9 + i*8]);
        vals[i] = u64::from_le_bytes(b);
    }
    (m[0], vals)
}

// Deterministic pseudo-random sequence (LCG)
fn next_rand(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    *seed
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
    println!("=== Distributed Sort-Merge Pipeline — Node 0 (Master) ===");
    println!("  batches={} x {} = {} numbers", BATCHES, BATCH_SIZE, TOTAL);

    let mut seed = GEN_SEED;
    let mut data = [0u64; BATCH_SIZE];
    let t0 = get_time();

    for b in 0..BATCHES {
        for i in 0..BATCH_SIZE {
            data[i] = next_rand(&mut seed);
        }

        // Send batch header with count
        let hdr = make_msg(TAG_BATCH, &[BATCH_SIZE as u64, 0,0,0,0,0,0], 0);
        while cxl_tx_push(&hdr) != 0 {}

        // Send data in chunks of PER_MSG
        let mut sent = 0;
        while sent < BATCH_SIZE {
            let chunk_len = (BATCH_SIZE - sent).min(PER_MSG);
            let mut chunk = [0u64; 7];
            for i in 0..chunk_len { chunk[i] = data[sent + i]; }
            while cxl_tx_push(&make_msg(TAG_DATA, &chunk, 0)) != 0 {}
            sent += chunk_len;
        }

        // Receive sorted batch from worker
        let mut got = 0usize;
        let mut sorted = [0u64; BATCH_SIZE];
        while got < BATCH_SIZE {
            let mut msg = [0u8; 60];
            if cxl_rx_pop(&mut msg) == 0 {
                let (tag, vals) = read_msg(&msg);
                if tag == TAG_RESULT {
                    let n = (BATCH_SIZE - got).min(PER_MSG);
                    for i in 0..n { sorted[got + i] = vals[i]; }
                    got += n;
                }
            }
        }
        // Verify sorted order
        let mut ok = true;
        for i in 1..BATCH_SIZE {
            if sorted[i] < sorted[i-1] { ok = false; break; }
        }
        if !ok {
            println!("  Master: batch {} NOT sorted!", b);
            return 1;
        }
        // Verify all numbers present (compare sets by sorting original too)
        let mut orig = data;
        sort(&mut orig);
        for i in 0..BATCH_SIZE {
            if orig[i] != sorted[i] {
                println!("  Master: batch {} data corrupted at {} (exp {}, got {})",
                         b, i, orig[i], sorted[i]);
                return 2;
            }
        }
        if b < 3 { println!("  Master: batch {} verified OK", b); }

        while cxl_tx_push(&make_msg(TAG_ACK, &[], 0)) != 0 {}
    }

    // Signal done
    while cxl_tx_push(&make_msg(TAG_DONE, &[], 0)) != 0 {}
    let ms = get_time() - t0;
    let total_ops = (TOTAL * 2) as u64; // send + receive per number
    println!("  Master: all {} batches OK in {} ms ({} ops/s)",
             BATCHES, ms,
             if ms > 0 { total_ops * 1000 / ms as u64 } else { 0 });
    0
}

fn run_worker() -> i32 {
    println!("=== Distributed Sort-Merge Pipeline — Node 1 (Worker) ===");

    let mut processed = 0usize;
    loop {
        // Wait for batch header or DONE
        let mut m = [0u8; 60];
        let batch_size;
        loop {
            if cxl_tx_pop(&mut m) == 0 {
                let (tag, vals) = read_msg(&m);
                if tag == TAG_DONE {
                    println!("  Worker: received DONE (processed {} batches)", processed);
                    return 0;
                }
                if tag == TAG_BATCH {
                    batch_size = vals[0] as usize;
                    break;
                }
            }
        }

        // Receive data
        let mut buf = [0u64; BATCH_SIZE];
        let mut got = 0;
        while got < batch_size {
            if cxl_tx_pop(&mut m) == 0 {
                let (tag, vals) = read_msg(&m);
                if tag == TAG_DATA {
                    let n = (batch_size - got).min(PER_MSG);
                    for i in 0..n { buf[got + i] = vals[i]; }
                    got += n;
                }
            }
        }

        // Sort
        sort_range(&mut buf, batch_size);

        // Send sorted data back
        let mut sent = 0;
        while sent < batch_size {
            let chunk_len = (batch_size - sent).min(PER_MSG);
            let mut chunk = [0u64; 7];
            for i in 0..chunk_len { chunk[i] = buf[sent + i]; }
            while cxl_rx_push(&make_msg(TAG_RESULT, &chunk, 1)) != 0 {}
            sent += chunk_len;
        }

        // Wait for master ack
        loop {
            let mut ack = [0u8; 60];
            if cxl_tx_pop(&mut ack) == 0 && read_msg(&ack).0 == TAG_ACK { break; }
        }
        processed += 1;
        if processed <= 3 { println!("  Worker: batch {} sorted and returned", processed - 1); }
    }
}

// Bubble sort for small arrays (fine for BATCH_SIZE=64)
fn sort_range(a: &mut [u64], n: usize) {
    for i in 0..n {
        for j in i+1..n {
            if a[j] < a[i] { a.swap(i, j); }
        }
    }
}
fn sort(a: &mut [u64]) { sort_range(a, a.len()); }
