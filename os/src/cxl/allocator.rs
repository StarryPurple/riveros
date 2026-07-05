//! Freelist-based page allocator for ivshmem data region.
//!
//! Allocation / free guarded by Lamport Bakery lock.
//! Page indices are zero-based within the data region.

use super::layout::*;
use super::lock;
use crate::mm::PhysPageNum;

const FREE_END: u32 = !0u32; // sentinel — end of freelist

// Cross-VM ring + Ring1 entries reside at SHM data pages starting here.
// Must match channel/cross.rs CROSS_BASE and cxl/layout.rs OFF_RX_ENTRIES.
pub const CROSS_RING_PAGE_START: usize = 0x3EE0; // 16096
pub const CROSS_RING_PAGE_COUNT: usize = 5;       // 20KB: cross rings(16KB) + ring1 entries(4KB)

static mut MY_ID: usize = 0;

pub fn set_instance_id(id: usize) {
    unsafe { MY_ID = id; }
}

#[inline]
pub fn me() -> usize {
    unsafe { MY_ID }
}

unsafe fn read_freelist(idx: u32) -> u32 {
    unsafe { shm_read32(FREELIST_OFF + idx as usize * 4) }
}

unsafe fn write_freelist(idx: u32, next: u32) {
    unsafe { shm_write32(FREELIST_OFF + idx as usize * 4, next); }
}

/// Mark the pages used by the cross-VM ring as permanently allocated
/// so the freelist never hands them out.
pub unsafe fn reserve_cross_ring_pages() {
    // Remove the 4 cross-ring pages from the freelist by advancing
    // the freelist head past them.
    let mut prev = FREE_END;
    let mut cur = unsafe { shm_read32(OFF_FREE_HEAD) };
    for _ in 0..CROSS_RING_PAGE_START {
        if cur == FREE_END { break; }
        prev = cur;
        cur = unsafe { read_freelist(cur) };
    }
    // Now cur = first page to remove.  Skip all reserved pages.
    for _ in 0..CROSS_RING_PAGE_COUNT {
        if cur == FREE_END { break; }
        cur = unsafe { read_freelist(cur) };
    }
    // Link prev -> cur, effectively cutting out the reserved block.
    if prev == FREE_END {
        unsafe { shm_write32(OFF_FREE_HEAD, cur) };
    } else {
        unsafe { write_freelist(prev, cur) };
    }
    unsafe { shm_fence() };
}

/// Rewrite freelist as a linked chain of all data pages (first boot).
pub unsafe fn shm_init_freelist() {
    unsafe {
        shm_write32(OFF_FREE_HEAD, 0u32);
        shm_write32(OFF_GC_HEAD, 0u32);    // ← zero GC pending head
        let n = DATA_PAGE_COUNT;
        for i in 0..n {
            let next = if i + 1 < n { (i + 1) as u32 } else { FREE_END };
            write_freelist(i as u32, next);
        }
        for i in 0..n {
            shm_write32(REFCOUNT_OFF + i * 4, 0);
            let owner_ptr = (SHM_BASE + OWNER_OFF + i) as *mut u8;
            owner_ptr.write_volatile(0u8);
        }
        shm_fence();
    }
}

/// Allocate one page from the shared region.
/// Sets ref_count=1 and owner=me.
pub fn shm_alloc_page() -> Option<usize> {
    unsafe {
        lock::bakery_lock(me());
        let head = shm_read32(OFF_FREE_HEAD);
        if head == FREE_END {
            lock::bakery_unlock(me());
            return None;
        }
        let next = read_freelist(head);
        shm_write32(OFF_FREE_HEAD, next);
        // Init ref_count=1, owner=me
        shm_write32(REFCOUNT_OFF + head as usize * 4, 1);
        let owner_ptr = (SHM_BASE + OWNER_OFF + head as usize) as *mut u8;
        owner_ptr.write_volatile(me() as u8);
        shm_fence();
        lock::bakery_unlock(me());
        Some(head as usize)
    }
}

/// Decrement a page's reference count.
/// When refcnt reaches 0 the page enters the GC-pending list.
/// Call `shm_gc_collect()` later to actually free eligible pages.
pub fn shm_free_page(idx: usize) {
    unsafe {
        lock::bakery_lock(me());
        super::refcnt::shm_unref_page(idx);
        // Try to GC immediately — in single-instance this frees right away;
        // in multi-instance the page stays pending until all peers advance.
        super::gc::shm_gc_collect();
        lock::bakery_unlock(me());
    }
}

/// Convert a data-page index into a [`PhysPageNum`].
#[inline]
pub fn shm_page_to_ppn(idx: usize) -> PhysPageNum {
    PhysPageNum(SHM_DATA_PPN_BASE + idx)
}

/// Convert a [`PhysPageNum`] back to a data-page index, or `None`.
#[inline]
pub fn ppn_to_shm_idx(ppn: PhysPageNum) -> Option<usize> {
    let base = SHM_DATA_PPN_BASE;
    if ppn.0 >= base && ppn.0 < base + DATA_PAGE_COUNT {
        Some(ppn.0 - base)
    } else {
        None
    }
}

/// Returns `true` if `ppn` lies in the SHM data region.
#[inline]
pub fn is_shm_page(ppn: PhysPageNum) -> bool {
    ppn.0 >= SHM_DATA_PPN_BASE && ppn.0 < SHM_DATA_PPN_BASE + DATA_PAGE_COUNT
}
