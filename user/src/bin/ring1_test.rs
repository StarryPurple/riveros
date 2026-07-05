#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{cxl_tx_push, cxl_tx_pop, cxl_rx_push, cxl_rx_pop,
               thread_create, waittid, exit};

fn client(_arg: usize) -> ! {
    // Wait for msg on Ring 0, send ack on Ring 1
    let mut buf = [0u8; 60];
    loop {
        if cxl_tx_pop(&mut buf) == 0 && buf[0] != 0 {
            buf[0] = 0xFF; // ack marker
            while cxl_rx_push(&buf) != 0 {}
            break;
        }
    }
    exit(0)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Ring1 test (single QEMU) ===");

    // Send via Ring 0
    let msg = [0x42u8; 60];
    while cxl_tx_push(&msg) != 0 {}
    println!("sent via Ring 0");

    // Start client thread (reads Ring 0, replies on Ring 1)
    let ct = thread_create(linker_symbol_addr!(client), 0);

    // Read ack via Ring 1
    let mut reply = [0u8; 60];
    loop {
        if cxl_rx_pop(&mut reply) == 0 {
            if reply[0] == 0xFF {
                println!("received ack via Ring 1");
                break;
            }
        }
    }

    waittid(ct as usize);
    println!("=== Ring1 test passed ===");
    0
}
