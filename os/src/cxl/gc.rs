//! Garbage collection for shared-memory pages.
//!
//! In single-instance mode the `OFF_GC_HEAD` is advanced on every
//! unref-to-zero, and `shm_gc_collect()` simply walks the pending
//! list and frees every valid entry back to the SHM freelist.
//!
//! For multi-instance (multiple QEMU) the vector-clock slots at
//! OFF_GLOBAL_VC are available (but not yet enforced) so that a
//! page is only freed once all peer instances have "moved past"
//! the GC timestamp at which it was queued.

use super::layout::*;

/// Walk the GC-pending list and return every eligible page to the
/// SHM freelist.  Returns the number of pages freed.
pub unsafe fn shm_gc_collect() -> usize {
    let me = super::allocator::me();

    // Advance local vector clock (for future multi-instance use)
    let cur_vc = shm_read64(OFF_GLOBAL_VC + me * 8);
    shm_write64(OFF_GLOBAL_VC + me * 8, cur_vc + 1);
    shm_fence();

    let head = shm_read32(OFF_GC_HEAD) as usize;
    if head == 0 {
        return 0;
    }

    let mut freed = 0usize;
    let max = GC_PENDING_ENTRIES;
    let mut new_head = 0usize;

    for i in 0..head.min(max) {
        let entry_off = GC_PENDING_OFF + i * GC_ENTRY_SIZE;

        // Fence: ensure head update is visible before reading entry data
        shm_fence();

        // Read flags byte
        let flags_ptr = (SHM_BASE + entry_off + 12) as *mut u8;
        let flags = flags_ptr.read_volatile();
        let valid = (flags & 0x80) != 0;
        if !valid {
            continue;
        }

        // For now: free unconditionally (single-instance mode).
        // Later: check every active instance's VC >= timestamp before freeing.
        let page_idx = shm_read32(entry_off) as usize;
        // Clear owner and refcnt
        let owner_ptr = (SHM_BASE + OWNER_OFF + page_idx) as *mut u8;
        owner_ptr.write_volatile(0u8);
        shm_write32(REFCOUNT_OFF + page_idx * 4, 0);
        // Return to freelist
        let old_head = shm_read32(OFF_FREE_HEAD);
        let fl_ptr = (SHM_BASE + FREELIST_OFF + page_idx * 4) as *mut u32;
        fl_ptr.write_volatile(old_head);
        shm_fence();
        shm_write32(OFF_FREE_HEAD, page_idx as u32);
        shm_fence();
        freed += 1;
        flags_ptr.write_volatile(0u8); // invalidate
    }

    shm_write32(OFF_GC_HEAD, new_head as u32);
    shm_fence();
    freed
}
