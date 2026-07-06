#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_mbox_send, cxl_mbox_recv, get_instance_id, msg_seal, msg_verify};

const TOTAL_PASSES: u32 = 12;
const N: usize = 3;

fn make_token(tag: u8, pass: u32, sender: usize) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0] = tag;
    m[1..5].copy_from_slice(&pass.to_le_bytes());
    msg_seal(&mut m, sender);
    m
}
fn read_token(m: &[u8; 60]) -> (u8, u32) {
    msg_verify(m).expect("checksum");
    let pass = u32::from_le_bytes([m[1], m[2], m[3], m[4]]);
    (m[0], pass)
}

fn make_term(sender: usize) -> [u8; 60] {
    let mut m = [0u8; 60];
    m[0] = 2;
    msg_seal(&mut m, sender);
    m
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let me = get_instance_id();
    let next = (me + 1) % N;

    println!("=== Token Ring — Node {} === ({} total passes)", me, TOTAL_PASSES);

    if me == 0 {
        let tok = make_token(1, 0, me);
        while cxl_mbox_send(next, &tok) != 0 {}
        println!("  [0] created and sent first token to Node {}", next);
    }

    let mut my_work = 0u32;

    loop {
        let mut m = [0u8; 60];
        if cxl_mbox_recv(&mut m) == 0 {
            let (tag, _) = (m[0], {
                msg_verify(&m).expect("checksum");
                u32::from_le_bytes([m[1], m[2], m[3], m[4]])
            });

            if tag == 2 {
                // Termination signal from another node
                println!("  [{}] received termination signal (work={})", me, my_work);
                break;
            }

            // tag == 1: normal token
            let pass = u32::from_le_bytes([m[1], m[2], m[3], m[4]]);
            my_work += 1;

            if pass >= TOTAL_PASSES {
                // Absorb token — broadcast termination to all OTHER nodes
                let term = make_term(me);
                for target in 0..N {
                    if target == me { continue; }
                    while cxl_mbox_send(target, &term) != 0 {}
                }
                println!("  [{}] absorbed token at pass={}, sent termination (work={})",
                         me, pass, my_work);
                break;
            }

            let new_pass = pass + 1;
            let tok = make_token(1, new_pass, me);
            while cxl_mbox_send(next, &tok) != 0 {}
            if new_pass <= 3 || new_pass % 4 == 0 {
                println!("  [{}] pass={} -> sent to Node {}", me, new_pass, next);
            }
        }
    }

    let expected_min = TOTAL_PASSES / 3;
    let expected_max = expected_min + 1;
    let ok = my_work >= expected_min && my_work <= expected_max;
    println!("  [{}] work_done={} (expected {}-{}): {}",
             me, my_work, expected_min, expected_max, if ok { "PASS" } else { "FAIL" });
    if ok { 0 } else { 1 }
}
