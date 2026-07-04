//! Freelist-based page allocator for ivshmem data region.
//!
//! Allocation / free guarded by Lamport Bakery lock.
//! Page indices are zero-based within the data region.

use super::layout::*;
use super::lock;
use crate::mm::PhysPageNum;

const FREE_END: u32 = !0u32; // sentinel — end of freelist

static mut MY_ID: usize = 0;

pub fn set_instance_id(id: usize) {
    unsafe { MY_ID = id; }
}

#[inline]
fn me() -> usize {
    unsafe { MY_ID }
}

unsafe fn read_freelist(idx: u32) -> u32 {
    unsafe { shm_read32(FREELIST_OFF + idx as usize * 4) }
}

unsafe fn write_freelist(idx: u32, next: u32) {
    unsafe { shm_write32(FREELIST_OFF + idx as usize * 4, next); }
}

/// Rewrite freelist as a linked chain of all data pages (first boot).
pub unsafe fn shm_init_freelist() {
    unsafe {
        shm_write32(OFF_FREE_HEAD, 0u32);
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
        lock::bakery_unlock(me());
        Some(head as usize)
    }
}

/// Return a previously-allocated page to the shared region.
pub fn shm_free_page(idx: usize) {
    unsafe {
        lock::bakery_lock(me());
        let old_head = shm_read32(OFF_FREE_HEAD);
        write_freelist(idx as u32, old_head);
        shm_write32(OFF_FREE_HEAD, idx as u32);
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
