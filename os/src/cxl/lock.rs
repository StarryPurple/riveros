//! Lamport's Bakery mutual-exclusion lock.
//!
//! Uses only plain loads and stores (no atomic RMW or lr/sc)
//! so it works across independently-emulated QEMU instances that
//! share the same ivshmem file.

use super::layout::*;

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
        let ptr_j_choose = (SHM_BASE + OFF_CHOOSING + j) as *const u8;
        while unsafe { ptr_j_choose.read_volatile() } != 0 {
            unsafe { shm_fence(); }
        }
        loop {
            let n = unsafe { shm_read64(OFF_NUMBER + j * 8) };
            if n == 0 { break; }
            if (n, j as u64) > (my_ticket, me as u64) { break; }
            unsafe { shm_fence(); }
        }
    }
}

/// Release the bakery lock held by instance `me`.
pub unsafe fn bakery_unlock(me: usize) {
    unsafe { shm_write64(OFF_NUMBER + me * 8, 0); }
}
