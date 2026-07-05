#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::{LockFreeRing, sys_ring_create_cross, exit, get_time};

const MSG_SIZE: usize = 64;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Cross-VM Ring Buffer (Guest) ===");

    // Map the ivshmem cross-VM rings
    let mut vaddr: usize = 0;
    let ret = sys_ring_create_cross(&mut vaddr);
    if ret < 0 {
        println!("sys_ring_create_cross failed: {}", ret);
        exit(1);
    }
    println!("Cross rings mapped at vaddr={:#x}", vaddr);

    // Ring 0: Host -> Guest (read by guest)
    // Ring 1: Guest -> Host (written by guest)
    // Each ring is 0x2000 bytes apart
    let ring_h2g = LockFreeRing::new(vaddr as *mut u8);
    let ring_g2h = LockFreeRing::new((vaddr + 0x2000) as *mut u8);

    let capacity = ring_h2g.header().capacity as usize;
    println!("Ring capacity: {} bytes", capacity);

    if capacity == 0 {
        println!("ERROR: rings not initialized!");
        exit(1);
    }

    let mut buf = [0u8; MSG_SIZE];
    let mut count: u64 = 0;
    let mut last_time = get_time();

    println!("Waiting for host messages on ring 0...");
    println!("(Run ./host_bench from the host, in the repos root)");
    println!("");

    loop {
        // Read from ring 0 (Host->Guest)
        match ring_h2g.try_pop(&mut buf) {
            Ok(n) if n == MSG_SIZE => {
                // Echo back via ring 1 (Guest->Host)
                ring_g2h.push_spin(&buf);

                count += 1;
                if count % 1000 == 0 {
                    let now = get_time();
                    let elapsed = if now > last_time { now - last_time } else { 1 };
                    let rate = 1000 * 1000 / elapsed as u64;
                    println!("[{}] msg/s: ~{}", count, rate);
                    last_time = now;
                }
            }
            Ok(_) => {}
            Err(_) => {}
        }

        if count >= 10000 {
            println!("Reached 10000 messages. Stopping.");
            break;
        }
    }

    let total_ms = get_time() - last_time;
    let throughput = (count * MSG_SIZE as u64) * 1000 / total_ms as u64;
    println!("Done. {} msgs, throughput: {} B/s", count, throughput);
    0
}
