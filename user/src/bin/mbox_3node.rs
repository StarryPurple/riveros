#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_mbox_send, cxl_mbox_recv, get_instance_id, msg_seal, msg_verify};

const MAX_NODES: usize = 4;

fn make_greeting(tag: u8, from: usize, to: usize) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0] = tag;
    m[1] = from as u8;
    m[2] = to as u8;
    msg_seal(&mut m, from);
    m
}
fn read_greeting(m: &[u8; 60]) -> (u8, usize, usize) {
    msg_verify(m).expect("checksum");
    (m[0], m[1] as usize, m[2] as usize)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let me = get_instance_id();
    // We support 3 nodes (0,1,2). Node 3 is silent if present.
    let n = if me > 2 { 3 } else { 3 };

    println!("=== Mailbox 3-Node Greeting — Node {} ===", me);

    // Phase 1: every node sends a greeting to every OTHER node
    for target in 0..n {
        if target == me { continue; }
        let msg = make_greeting(1, me, target);
        while cxl_mbox_send(target, &msg) != 0 {}
        println!("  [{}] sent greeting to Node {}", me, target);
    }

    // Phase 2: collect greetings from other nodes
    let mut received = [false; MAX_NODES];
    let mut count = 0usize;
    let expected = n - 1;
    let mut spin = 0usize;

    while count < expected {
        let mut m = [0u8; 60];
        if cxl_mbox_recv(&mut m) == 0 {
            let (_tag, from, _to) = read_greeting(&m);
            if from < MAX_NODES && !received[from] {
                received[from] = true;
                count += 1;
                println!("  [{}] received greeting from Node {}", me, from);
            }
        }
        spin += 1;
        if spin % 5000 == 0 {
            println!("  [{}] waiting... ({}/{})", me, count, expected);
        }
    }

    // Phase 3: verify
    let mut ok = true;
    for node in 0..n {
        if node == me { continue; }
        if !received[node] {
            println!("  [{}] MISSING greeting from Node {}", me, node);
            ok = false;
        }
    }
    println!("  [{}] {} greetings collected: {}", me, count,
             if ok { "PASS" } else { "FAIL" });
    if ok { 0 } else { 1 }
}
