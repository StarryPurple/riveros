#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::{cxl_tx_push, cxl_tx_pop, fork, waitpid, exit};

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== CXL Ring Buffer Test ===\n");

    // Sequential push/pop
    println!("--- Round-trip test ---");
    let msg: [u8; 60] = {
        let mut m = [0u8; 60];
        let text = b"HELLO_RING_12345";
        m[..text.len()].copy_from_slice(text);
        m
    };
    let ret = cxl_tx_push(&msg);
    assert!(ret == 0, "push failed: {}", ret);
    let mut out = [0u8; 60];
    let ret = cxl_tx_pop(&mut out);
    assert!(ret == 0, "pop failed: {}", ret);
    assert_eq!(out, msg, "data mismatch on round-trip");
    println!("round-trip: OK  (pushed '{:?}', got '{:?}')",
        core::str::from_utf8(&msg[..18]).unwrap_or("??"),
        core::str::from_utf8(&out[..18]).unwrap_or("??"));

    // Fork-based busy-poll
    println!("\n--- Fork busy-poll test ---");
    let pid = fork();
    if pid == 0 {
        // child: busy-pop until we receive a quit marker
        let mut buf = [0u8; 60];
        loop {
            while cxl_tx_pop(&mut buf) != 0 {
                // busy-poll
            }
            if buf[0] == 0xFF {
                break; // quit marker
            }
            // verify payload begins with 0xAA
            assert_eq!(buf[0], 0xAA, "child: bad payload start {:x}", buf[0]);
        }
        exit(0);
    }

    // parent: push 50 messages + quit marker
    let mut payload = [0u8; 60];
    for i in 0..50 {
        payload[0] = 0xAA;
        payload[1] = i as u8;
        while cxl_tx_push(&payload) != 0 {
            // spin until space available
        }
    }
    payload[0] = 0xFF; // quit marker
    while cxl_tx_push(&payload) != 0 {}

    let mut code = 0i32;
    waitpid(pid as usize, &mut code);
    assert!(code == 0, "child exited with {}", code);
    println!("fork busy-poll: OK  (50 messages)");

    println!("\n=== cxl_ring_test PASSED ===");
    0
}
