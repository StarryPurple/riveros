#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_mbox_recv, cxl_mbox_send, get_instance_id, msg_seal, msg_verify, sleep};

const TASKS_PER_WORKER: usize = 32;
const WORKERS: usize = 2;
const TOTAL_TASKS: usize = WORKERS * TASKS_PER_WORKER;

fn make_task(req_id: u32, op: u8, a: u32, b: u32, sender: usize) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0] = 1;
    m[1..5].copy_from_slice(&req_id.to_le_bytes());
    m[5] = op;
    m[6..10].copy_from_slice(&a.to_le_bytes());
    m[10..14].copy_from_slice(&b.to_le_bytes());
    msg_seal(&mut m, sender);
    m
}
fn read_task(m: &[u8; 60]) -> (u32, u8, u32, u32) {
    msg_verify(m).expect("checksum");
    let id = u32::from_le_bytes([m[1], m[2], m[3], m[4]]);
    let op = m[5];
    let a  = u32::from_le_bytes([m[6], m[7], m[8], m[9]]);
    let b  = u32::from_le_bytes([m[10], m[11], m[12], m[13]]);
    (id, op, a, b)
}

fn make_reply(req_id: u32, result: u32, sender: usize) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0] = 2;
    m[1..5].copy_from_slice(&req_id.to_le_bytes());
    m[5..9].copy_from_slice(&result.to_le_bytes());
    msg_seal(&mut m, sender);
    m
}
fn read_reply(m: &[u8; 60]) -> (u32, u32) {
    msg_verify(m).expect("checksum");
    let id = u32::from_le_bytes([m[1], m[2], m[3], m[4]]);
    let r  = u32::from_le_bytes([m[5], m[6], m[7], m[8]]);
    (id, r)
}

fn compute(op: u8, a: u32, b: u32) -> u32 {
    match op {
        1 => a.wrapping_add(b),
        2 => a.wrapping_mul(b),
        3 => a ^ (b.wrapping_mul(0x9E3779B9)),
        _ => a,
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let me = get_instance_id();
    if me == 0 {
        run_master()
    } else if me <= WORKERS {
        run_worker(me)
    } else {
        println!("  Node {} is idle", me);
        0
    }
}

fn run_master() -> i32 {
    println!("=== Distributed Calculator — Node 0 (Master) ===");
    println!("  {} workers × {} tasks = {} total", WORKERS, TASKS_PER_WORKER, TOTAL_TASKS);

    // Pre-compute expected results
    let mut expected = [0u32; TOTAL_TASKS];

    // Dispatch tasks to workers
    for w in 0..WORKERS {
        for i in 0..TASKS_PER_WORKER {
            let id = (w * TASKS_PER_WORKER + i) as u32;
            let op = ((id % 3 + 1) as u8);
            let a = id.wrapping_mul(13).wrapping_add(7);
            let b = id.wrapping_mul(31).wrapping_add(19);
            expected[id as usize] = compute(op, a, b);

            let task = make_task(id, op, a, b, 0);   // master is node 0
            while cxl_mbox_send(w + 1, &task) != 0 {} // worker IDs are 1,2
        }
        println!("  [master] sent {} tasks to Worker {}", TASKS_PER_WORKER, w + 1);
    }

    // Collect results
    let mut results = [0u32; TOTAL_TASKS];
    let mut received = [false; TOTAL_TASKS];
    let mut count = 0usize;
    let mut spin = 0usize;

    println!("  [master] waiting for worker replies...");
    while count < TOTAL_TASKS {
        let mut m = [0u8; 60];
        if cxl_mbox_recv(&mut m) == 0 {
            if m[0] != 2 { continue; }
            let (id, result) = read_reply(&m);
            let idx = id as usize;
            if idx < TOTAL_TASKS && !received[idx] {
                results[idx] = result;
                received[idx] = true;
                count += 1;
            }
        }
        spin += 1;
        if spin % 10000 == 0 {
            println!("  [master] received {}/{}, still waiting...", count, TOTAL_TASKS);
        }
    }
    println!("  [master] collected {} results", count);

    // Verify
    let mut errors = 0usize;
    for i in 0..TOTAL_TASKS {
        if results[i] != expected[i] {
            if errors < 3 { println!("  task[{}]: exp={} got={}", i, expected[i], results[i]); }
            errors += 1;
        }
    }
    let ok = errors == 0;
    println!("  [master] verification: {}", if ok { "PASS" } else { "FAIL" });
    if ok { 0 } else { 1 }
}

fn run_worker(me: usize) -> i32 {
    println!("=== Distributed Calculator — Node {} (Worker) ===", me);

    let mut processed = 0usize;
    while processed < TASKS_PER_WORKER {
        let mut m = [0u8; 60];
        if cxl_mbox_recv(&mut m) == 0 {
            sleep(100);
            if m[0] != 1 { continue; }
            let (id, op, a, b) = read_task(&m);
            let result = compute(op, a, b);
            let reply = make_reply(id, result, me);
            while cxl_mbox_send(0, &reply) != 0 {}
            processed += 1;
            println!("  [worker {}] processed {} tasks", me, processed);
        }
    }
    // implicit: expected == TASKS_PER_WORKER
    0
}
