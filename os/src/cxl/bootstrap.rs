//! ivshmem discovery and instance-ID assignment.
//!
//! On the very first run (magic == 0) the header is written and the
//! freelist is built.  On subsequent runs the calling QEMU instance
//! simply claims the next available instance ID.

use core::arch::asm;
use super::layout::*;

use crate::mm::FRAME_ALLOCATOR;

/// Alias for `asm!("fence iorw, iorw")` — ensures I/O+RW ordering.
#[inline(always)]
unsafe fn fence() {
    asm!("fence iorw, iorw");
}

/// Read a u32 at SHM offset `off`.
#[inline(always)]
unsafe fn shm_read32(off: usize) -> u32 {
    let ptr = (SHM_BASE + off) as *const u32;
    fence(); // load-to-load ordering
    ptr.read_volatile()
}

/// Read a u64 at SHM offset `off`.
#[inline(always)]
unsafe fn shm_read64(off: usize) -> u64 {
    let ptr = (SHM_BASE + off) as *const u64;
    fence();
    ptr.read_volatile()
}

/// Write a u32 at SHM offset `off`.
#[inline(always)]
unsafe fn shm_write32(off: usize, val: u32) {
    let ptr = (SHM_BASE + off) as *mut u32;
    ptr.write_volatile(val);
    fence();
}

/// Write a u64 at SHM offset `off`.
#[inline(always)]
unsafe fn shm_write64(off: usize, val: u64) {
    let ptr = (SHM_BASE + off) as *mut u64;
    ptr.write_volatile(val);
    fence();
}

/// Initialise ivshmem header OR claim an instance ID.
///
/// Returns `(my_instance_id, is_first_boot)`.
pub fn shm_init() -> (usize, bool) {
    let magic = unsafe { shm_read64(OFF_MAGIC) };

    if magic != SHM_MAGIC {
        // ── first boot ──
        unsafe {
            shm_write64(OFF_MAGIC, SHM_MAGIC);
            shm_write32(OFF_N_INSTANCES, 0);
            shm_write32(OFF_DATA_START, DATA_START as u32);
            shm_write32(OFF_TOTAL_PAGES, DATA_PAGE_COUNT as u32);

            // build initial freelist (all data pages free, linked list)
            shm_write32(OFF_FREE_HEAD, 0u32);
            let n = DATA_PAGE_COUNT;
            for i in 0..n {
                let next = if i + 1 < n { (i + 1) as u32 } else { !0u32 };
                let off = FREELIST_OFF + i * 4;
                shm_write32(off, next);
            }

            // zero out ref_counts & owner array for safety
            for i in 0..n {
                shm_write32(REFCOUNT_OFF + i * 4, 0);
                let owner_off = OWNER_OFF + i;
                let owner_ptr = (SHM_BASE + owner_off) as *mut u8;
                owner_ptr.write_volatile(0u8);
            }
            fence();
        }

        let n_inst = unsafe { shm_read32(OFF_N_INSTANCES) };
        let my_id = n_inst as usize;
        unsafe {
            shm_write32(OFF_N_INSTANCES, n_inst + 1);
        }
        println!("[CXL] shm first-boot -- instance {}", my_id);
        (my_id, true)
    } else {
        // ── subsequent boot ──
        let n_inst = unsafe { shm_read32(OFF_N_INSTANCES) };
        let my_id = n_inst as usize;
        unsafe {
            shm_write32(OFF_N_INSTANCES, n_inst + 1);
        }
        println!("[CXL] shm joined — instance {}", my_id);
        (my_id, false)
    }
}
