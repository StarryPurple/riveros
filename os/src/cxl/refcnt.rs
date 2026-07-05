//! Reference counting + GC pending for distributed shared pages.
//!
//! Each SHM data page has a reference count (at REFCOUNT_OFF) and
//! an owner (at OWNER_OFF).  When refcnt drops to 0 the page enters
//! a GC-pending list.  `shm_gc_collect()` (in gc.rs) uses the
//! distributed vector clock to decide when a pending page can be
//! safely freed.

use super::layout::*;
use super::lock;

/// The owner writes a pending entry (page must have refcnt == 0).
unsafe fn gc_pending_push(idx: usize, owner: u8) {
    let head = shm_read32(OFF_GC_HEAD) as usize;
    let max = GC_PENDING_ENTRIES;
    if head >= max {
        // Pending list full — skip (page leaks, rare in practice)
        return;
    }
    let entry_off = GC_PENDING_OFF + head * GC_ENTRY_SIZE;
    // page_idx (u32)
    shm_write32(entry_off, idx as u32);
    // timestamp (u64) = current vc[me]
    let me = super::allocator::me();
    let my_vc = shm_read64(OFF_GLOBAL_VC + me * 8);
    let ts_ptr = (SHM_BASE + entry_off + 4) as *mut u64;
    ts_ptr.write_volatile(my_vc);
    // flags: owner in low nibble, valid in bit 7
    let flags = owner | 0x80u8;
    let ptr = (SHM_BASE + entry_off + 12) as *mut u8;
    ptr.write_volatile(flags);
    shm_fence();
    // head += 1
    shm_write32(OFF_GC_HEAD, (head + 1) as u32);
    shm_fence();
}

/// Increment a page's reference count.
/// Caller must hold the bakery lock.
pub unsafe fn shm_ref_page(idx: usize) {
    let rc_ptr = (SHM_BASE + REFCOUNT_OFF + idx * 4) as *mut u32;
    let rc = rc_ptr.read_volatile();
    rc_ptr.write_volatile(rc + 1);
    shm_fence();
}

/// Decrement a page's reference count.
/// If it reaches 0 and we are the owner, push onto GC pending.
/// If we are NOT the owner, just decrement and leave.
/// Caller must hold the bakery lock.
pub unsafe fn shm_unref_page(idx: usize) {
    let rc_ptr = (SHM_BASE + REFCOUNT_OFF + idx * 4) as *mut u32;
    let rc = rc_ptr.read_volatile();
    if rc <= 1 {
        // Last reference dropped
        rc_ptr.write_volatile(0);
        shm_fence();
        let owner_ptr = (SHM_BASE + OWNER_OFF + idx) as *mut u8;
        let owner = owner_ptr.read_volatile();
        let me = super::allocator::me();
        if owner == me as u8 || owner == 0 {
            // is the owner -> push to GC pending
            gc_pending_push(idx, owner);
        }
        // If not the owner, the GC (run by owner) will collect later
    } else {
        rc_ptr.write_volatile(rc - 1);
        shm_fence();
    }
}
