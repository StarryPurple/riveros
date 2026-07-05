//! ivshmem discovery and fixed-instance-ID init.
//!
//! Each QEMU instance receives a fixed instance ID (0, 1, or 2)
//! via QEMU's `-device loader` at a known RAM address.
//! Instance 0 is responsible for first-boot SHM initialisation;
//! instances 1+ simply register their bakery slot and join.

use super::layout::*;
use super::allocator;

/// Initialise ivshmem (instance 0) OR register bakery slot (instances 1+).
///
/// `my_id` must be supplied externally (from boot-loader parameter).
/// Returns `(my_id, is_first_boot)`.
pub fn shm_init(my_id: usize) -> (usize, bool) {
    allocator::set_instance_id(my_id);
    let magic = unsafe { shm_read64(OFF_MAGIC) };

    if magic != SHM_MAGIC && my_id == 0 {
        // ── First-ever boot: instance 0 initialises SHM ──
        // Mark bootstrap in progress (so late-starting peers wait)
        let lock = (SHM_BASE + OFF_BOOTSTRAP_LOCK) as *mut u32;
        unsafe { lock.write_volatile(1u32); }
        unsafe { shm_fence(); }

        // Write magic + init freelist + metadata arrays
        unsafe {
            shm_write64(OFF_MAGIC, SHM_MAGIC);
            shm_write32(OFF_N_INSTANCES, 0);
            shm_write32(OFF_GC_HEAD, 0);
            shm_write32(OFF_DATA_START, DATA_START as u32);
            shm_write32(OFF_TOTAL_PAGES, DATA_PAGE_COUNT as u32);
            allocator::shm_init_freelist();
            // Reserve cross-ring pages in freelist (indices 16096-16099)
            allocator::reserve_cross_ring_pages();
        }
        // Register instance 0's bakery slot
        unsafe { reset_bakery_id(my_id); }
        println!("[CXL] instance {} — first boot, SHM initialised", my_id);

        // Bootstrap done
        unsafe { lock.write_volatile(0u32); }
        unsafe { shm_fence(); }
        (my_id, true)

    } else if magic != SHM_MAGIC && my_id != 0 {
        // ── Boot before instance 0 finished: spin-wait for magic ──
        crate::print!("[CXL] instance {} waiting for SHM init...", my_id);
        let mut wait = 0u32;
        while unsafe { shm_read64(OFF_MAGIC) } != SHM_MAGIC {
            wait += 1;
            if wait > 50_000_000 {
                crate::println!(" timeout! Is instance 0 running?");
                unsafe { reset_bakery_id(my_id); }
                return (my_id, false);
            }
            core::hint::spin_loop();
        }
        // Also wait for bootstrap lock to be released
        let lock = (SHM_BASE + OFF_BOOTSTRAP_LOCK) as *const u32;
        while unsafe { lock.read_volatile() } != 0 {
            core::hint::spin_loop();
        }
        unsafe { reset_bakery_id(my_id); }
        println!(" done");

        // Update instance count
        let n_inst = unsafe { shm_read32(OFF_N_INSTANCES) };
        if my_id as u32 >= n_inst {
            unsafe { shm_write32(OFF_N_INSTANCES, (my_id + 1) as u32); }
        }
        (my_id, false)

    } else {
        // ── SHM already exists, join ──
        // Clear stale GC head from previous session
        unsafe { shm_write32(OFF_GC_HEAD, 0); }
        unsafe { reset_bakery_id(my_id); }
        let n_inst = unsafe { shm_read32(OFF_N_INSTANCES) };
        if my_id as u32 >= n_inst {
            unsafe { shm_write32(OFF_N_INSTANCES, (my_id + 1) as u32); }
        }
        println!("[CXL] instance {} — joined SHM", my_id);
        (my_id, false)
    }
}

/// Reset this instance's bakery state (choosing=0, number=0).
unsafe fn reset_bakery_id(me: usize) {
    if me >= MAX_INSTANCES { panic!("instance ID {} out of range", me); }
    shm_write64(OFF_NUMBER + me * 8, 0);
    let p = (SHM_BASE + OFF_CHOOSING + me) as *mut u8;
    p.write_volatile(0u8);
    shm_fence();
}
