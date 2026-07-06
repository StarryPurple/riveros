//! Garbage collection with vector-clock validation.
//!
//! Multi-instance safety: a pending entry is only freed once all
//! active instances have advanced their vector clocks past the
//! entry's timestamp — meaning all peers have "seen" the unref.

use super::layout::*;

/// Drain ALL pending GC entries and return them to the freelist.
/// Used during SHM re-join to clean up stale entries from a
/// previous session.  Bypasses the vector-clock check because
/// the previous session is dead and all entries are eligible.
pub unsafe fn gc_drain_on_join() -> usize {
    let head = unsafe { shm_read32(OFF_GC_HEAD) as usize };
    let mut freed = 0usize;

    for i in 0..head.min(GC_PENDING_ENTRIES) {
        let entry_off = GC_PENDING_OFF + i * GC_ENTRY_SIZE;
        let flags_ptr = (SHM_BASE + entry_off + 12) as *mut u8;
        let flags = flags_ptr.read_volatile();
        if (flags & 0x80) == 0 { continue; }

        let page_idx = unsafe { shm_read32(entry_off) as usize };

        unsafe {
            let owner_ptr = (SHM_BASE + OWNER_OFF + page_idx) as *mut u8;
            owner_ptr.write_volatile(0u8);
            shm_write32(REFCOUNT_OFF + page_idx * 4, 0);

            let old_head = shm_read32(OFF_FREE_HEAD);
            let fl_ptr = (SHM_BASE + FREELIST_OFF + page_idx * 4) as *mut u32;
            fl_ptr.write_volatile(old_head);
            shm_fence();
            shm_write32(OFF_FREE_HEAD, page_idx as u32);
            shm_fence();
        }

        unsafe { flags_ptr.write_volatile(0u8) };
        freed += 1;
    }

    unsafe {
        shm_write32(OFF_GC_HEAD, 0);
        shm_fence();
    }

    freed
}

/// Reset all vector clocks to zero. Called during SHM re-join
/// so the new session starts with a clean VC baseline.
pub unsafe fn reset_vc() {
    for i in 0..MAX_INSTANCES {
        unsafe { shm_write64(OFF_GLOBAL_VC + i * 8, 0); }
    }
    unsafe { shm_fence(); }
}

/// Walk the GC-pending list and return freed pages.
pub unsafe fn shm_gc_collect() -> usize {
    let me = super::allocator::me();

    // 1. Advance local vector clock
    let cur_vc = unsafe { shm_read64(OFF_GLOBAL_VC + me * 8) };
    unsafe { shm_write64(OFF_GLOBAL_VC + me * 8, cur_vc + 1) };
    unsafe { shm_fence() };

    // 2. Read vector clocks of all active instances
    let n_inst = unsafe { shm_read32(OFF_N_INSTANCES) as usize };
    let mut vcs = [0u64; MAX_INSTANCES];
    for i in 0..n_inst.min(MAX_INSTANCES) {
        vcs[i] = unsafe { shm_read64(OFF_GLOBAL_VC + i * 8) };
    }

    // 3. Scan GC pending list
    let head = unsafe { shm_read32(OFF_GC_HEAD) as usize };
    let mut freed = 0usize;
    let mut new_head = 0usize;

    for i in 0..head.min(GC_PENDING_ENTRIES) {
        let entry_off = GC_PENDING_OFF + i * GC_ENTRY_SIZE;
        unsafe { shm_fence() };

        let flags_ptr = (SHM_BASE + entry_off + 12) as *mut u8;
        let flags = flags_ptr.read_volatile();
        if (flags & 0x80) == 0 { continue; }

        let page_idx = unsafe { shm_read32(entry_off) as usize };
        let timestamp = {
            let ts_ptr = (SHM_BASE + entry_off + 4) as *const u64;
            unsafe { ts_ptr.read_volatile() }
        };

        // Vector-clock check: free only after all instances' clocks
        // have advanced past the entry's timestamp.
        // In single-instance mode (n_inst <= 1) skip the check —
        // the local VC was already advanced at the top of this function.
        let all_past = n_inst <= 1 || (0..n_inst).all(|j| vcs[j] >= timestamp);

        if all_past {
            // Safe to free
            let owner_ptr = (SHM_BASE + OWNER_OFF + page_idx) as *mut u8;
            unsafe { owner_ptr.write_volatile(0u8) };
            unsafe { shm_write32(REFCOUNT_OFF + page_idx * 4, 0) };

            let old_head = unsafe { shm_read32(OFF_FREE_HEAD) };
            let fl_ptr = (SHM_BASE + FREELIST_OFF + page_idx * 4) as *mut u32;
            unsafe { fl_ptr.write_volatile(old_head) };
            unsafe { shm_fence() };
            unsafe { shm_write32(OFF_FREE_HEAD, page_idx as u32) };
            unsafe { shm_fence() };

            freed += 1;
            unsafe { flags_ptr.write_volatile(0u8) };
        } else {
            // Keep entry — compact forward
            if new_head != i {
                let src = SHM_BASE + entry_off;
                let dst = SHM_BASE + GC_PENDING_OFF + new_head * GC_ENTRY_SIZE;
                unsafe { core::ptr::copy_nonoverlapping(
                    src as *const u8, dst as *mut u8, GC_ENTRY_SIZE) };
            }
            new_head += 1;
        }
    }
    unsafe { shm_write32(OFF_GC_HEAD, new_head as u32) };
    unsafe { shm_fence() };

    freed
}
