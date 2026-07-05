//! Lamport's Bakery mutual-exclusion lock.
//!
//! Uses only plain loads and stores (no atomic RMW or lr/sc)
//! so it works across independently-emulated QEMU instances that
//! share the same ivshmem file.
//!
//! Dead-instance detection: if a peer spins for SPIN_LIMIT iterations
//! while holding a non-zero ticket, the peer is assumed crashed and
//! its bakery state is reset.

use super::layout::*;

pub const SPIN_LIMIT: u64 = 1_000_000;

/// Acquire the bakery lock for instance `me`.
pub unsafe fn bakery_lock(me: usize) {
    // 1. announce intention
    let off_choose = OFF_CHOOSING + me;
    let ptr_choose = (SHM_BASE + off_choose) as *mut u8;
    unsafe { ptr_choose.write_volatile(1u8); }
    unsafe { shm_fence(); }

    // 2. take the next ticket number
    let mut max = 0u64;
    for i in 0..MAX_INSTANCES {
        let n = unsafe { shm_read64(OFF_NUMBER + i * 8) };
        if n > max { max = n; }
    }
    let my_ticket = max + 1;
    unsafe { shm_write64(OFF_NUMBER + me * 8, my_ticket); }

    // 3. done choosing
    unsafe { ptr_choose.write_volatile(0u8); }
    unsafe { shm_fence(); }

    // 4. wait for our turn
    for j in 0..MAX_INSTANCES {
        if j == me { continue; }

        // 4a. wait until peer finishes choosing
        let ptr_j_choose = (SHM_BASE + OFF_CHOOSING + j) as *const u8;
        while unsafe { ptr_j_choose.read_volatile() } != 0 {
            unsafe { shm_fence(); }
        }
        // Acquire fence: ensure choosing[j]==0 is visible before reading number[j]
        unsafe { shm_fence(); }

        // 4b. wait for our ticket to come up
        let mut spins: u64 = 0;
        loop {
            let n = unsafe { shm_read64(OFF_NUMBER + j * 8) };
            if n == 0 { break; }
            if (n, j as u64) > (my_ticket, me as u64) { break; }
            spins += 1;
            if spins >= SPIN_LIMIT {
                // Peer j appears dead — clear its bakery state and proceed
                crate::println_cxl!("[CXL] bakery: instance {} dead, resetting", j);
                unsafe { shm_write64(OFF_NUMBER + j * 8, 0); }
                unsafe {
                    let p = (SHM_BASE + OFF_CHOOSING + j) as *mut u8;
                    p.write_volatile(0u8);
                }
                unsafe { shm_fence(); }
                break;
            }
            unsafe { shm_fence(); }
        }
    }
}

/// Release the bakery lock held by instance `me`.
pub unsafe fn bakery_unlock(me: usize) {
    unsafe { shm_write64(OFF_NUMBER + me * 8, 0); }
}
