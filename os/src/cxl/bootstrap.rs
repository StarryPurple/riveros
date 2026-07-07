//! ivshmem discovery and fixed-instance-ID init.
//!
//! Each QEMU instance receives a fixed instance ID (0, 1, or 2)
//! via QEMU's `-device loader` at a known RAM address.
//! Instance 0 is responsible for first-boot SHM initialisation;
//! instances 1+ simply register their bakery slot and join.

use super::layout::*;
use super::allocator;
use crate::mm::{FRAME_ALLOCATOR, PhysPageNum};

/// Initialise ivshmem (instance 0) OR register bakery slot (instances 1+).
///
/// `my_id` must be supplied externally (from boot-loader parameter).
/// Returns `(my_id, is_first_boot)`.
pub fn shm_init(my_id: usize) -> (usize, bool) {
    allocator::set_instance_id(my_id);
    let magic = unsafe { shm_read64(OFF_MAGIC) };

    if magic != SHM_MAGIC && my_id == 0 {
        // First-ever boot: instance 0 initialises SHM
        // Mark bootstrap in progress (so late-starting peers wait)
        let lock = (SHM_BASE + OFF_BOOTSTRAP_LOCK) as *mut u32;
        unsafe { lock.write_volatile(1u32); }
        unsafe { shm_fence(); }

        // Write magic + init freelist + metadata arrays
        unsafe {
            shm_write64(OFF_MAGIC, SHM_MAGIC);
            shm_write32(OFF_N_INSTANCES, 0);
            shm_write32(OFF_GC_HEAD, 0);
            super::ring::reset_tx();
            super::ring::reset_rx();
            super::mbox::mbox_reset_all();
            shm_write32(OFF_DATA_START, DATA_START as u32);
            shm_write32(OFF_TOTAL_PAGES, DATA_PAGE_COUNT as u32);
            allocator::shm_init_freelist();
            // Reserve cross-ring pages in freelist (indices 16096-16099)
            allocator::reserve_cross_ring_pages();
        }
        pin_cross_ring_pages();
        // Register instance 0's bakery slot
        unsafe { reset_bakery_id(my_id); }
        println!("[CXL] instance {} — first boot, SHM initialised", my_id);

        // Bootstrap done
        unsafe { lock.write_volatile(0u32); }
        unsafe { shm_fence(); }
        (my_id, true)

    } else if magic != SHM_MAGIC && my_id != 0 {
        // Boot before instance 0 finished: spin-wait for magic
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
        // SHM already exists — full cleanup of previous-session state
        unsafe {
            // 1. Drain pending GC entries (reclaim leaked pages)
            super::gc::gc_drain_on_join();
            // 2. Reset vector clocks to a clean baseline
            super::gc::reset_vc();
            // 3. Reset TX ring (head/tail/flags + slot payload)
            super::ring::reset_tx();
            // 4. Reset RX ring (head/tail/flags + slot payload)
            super::ring::reset_rx();
            // 5. Zero the cross-ring shared-memory area (stale data-plane data)
            super::ring::reset_cross_area();
            // 6. Reset all mailboxes
            super::mbox::mbox_reset_all();
            // 7. Clear all bakery lock slots (crashed peers, stale state)
            reset_all_bakery();
        }
        pin_cross_ring_pages();
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
    unsafe { shm_write64(OFF_NUMBER + me * 8, 0) };
    let p = (SHM_BASE + OFF_CHOOSING + me) as *mut u8;
    unsafe { p.write_volatile(0u8) };
    unsafe { shm_fence() };
}

/// Clear all bakery slots — removes stale lock state from crashed peers.
unsafe fn reset_all_bakery() {
    for i in 0..MAX_INSTANCES {
        unsafe { shm_write64(OFF_NUMBER + i * 8, 0) };
        let p = (SHM_BASE + OFF_CHOOSING + i) as *mut u8;
        unsafe { p.write_volatile(0u8) };
    }
    unsafe { shm_fence() };
}

/// Pin all cross-ring reserved pages so the page migrator never
/// promotes them to DRAM (Host accesses these pages at fixed offsets).
fn pin_cross_ring_pages() {
    let mut alloc = FRAME_ALLOCATOR.exclusive_access();
    for i in 0..allocator::CROSS_RING_PAGE_COUNT {
        let idx = allocator::CROSS_RING_PAGE_START + i;
        let ppn = allocator::shm_page_to_ppn(idx);
        alloc.mark_pinned(ppn);
    }
}
