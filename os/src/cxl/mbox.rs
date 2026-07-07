//! Per-node mailbox — MPSC ring buffer on ivshmem shared memory.
//!
//! Each node has a dedicated mailbox (64 slots × 64 B = 4 KB).
//! Multiple producers (other nodes) write to a mailbox while holding
//! the bakery lock.  The owning node reads lock-free.
//!
//! Slot layout (same as ring.rs): 4 B flag + 60 B payload = 64 B.

use super::layout::*;
use super::lock;

const WINDOW: u32 = MBOX_CAPACITY as u32;
const MASK:   u32 = WINDOW - 1;

/// Write 60 bytes to target node's mailbox.
/// Caller is blocked if mailbox is full. Returns false on full.
pub unsafe fn mbox_send(me: usize, target: usize, data: &[u8; crate::cxl::ring::MSG_SIZE]) -> bool {
    if target >= MAX_INSTANCES { return false; }
    unsafe {
        lock::bakery_lock(me);
        let head_off = OFF_MBOX_HEAD + target * 4;
        let tail_off = OFF_MBOX_TAIL + target * 4;
        let head = shm_read32(head_off);
        let tail = shm_read32(tail_off);

        if head.wrapping_sub(tail) >= WINDOW {
            lock::bakery_unlock(me);
            return false;
        }
        let slot = (head & MASK) as usize;
        let entry_base = mbox_entries_off(target) + slot * MBOX_SLOT_SIZE;

        // Write payload first
        for (i, &b) in data.iter().enumerate() {
            let p = (SHM_BASE + entry_base + 4 + i) as *mut u8;
            p.write_volatile(b);
        }
        shm_fence();
        // Publish
        let flag = (SHM_BASE + entry_base) as *mut u32;
        flag.write_volatile(1u32);
        shm_fence();
        shm_write32(head_off, head.wrapping_add(1));
        lock::bakery_unlock(me);
    }
    true
}

/// Read 60 bytes from own mailbox (non-blocking).
/// Returns None when mailbox is empty.
pub unsafe fn mbox_recv(me: usize) -> Option<[u8; crate::cxl::ring::MSG_SIZE]> {
    let head_off = OFF_MBOX_HEAD + me * 4;
    let tail_off = OFF_MBOX_TAIL + me * 4;
    let tail = unsafe { shm_read32(tail_off) };
    let head = unsafe { shm_read32(head_off) };
    if head == tail { return None; }

    let slot = (tail & MASK) as usize;
    let entry_base = mbox_entries_off(me) + slot * MBOX_SLOT_SIZE;

    // Wait for producer flag
    let flag = (SHM_BASE + entry_base) as *const u32;
    unsafe {
        while flag.read_volatile() != 1 {
            core::arch::asm!("wfi", options(nostack));
            shm_fence();
        }
        shm_fence();
    }

    let mut data = [0u8; crate::cxl::ring::MSG_SIZE];
    unsafe {
        for i in 0..crate::cxl::ring::MSG_SIZE {
            let p = (SHM_BASE + entry_base + 4 + i) as *const u8;
            data[i] = p.read_volatile();
        }
        let flag_mut = (SHM_BASE + entry_base) as *mut u32;
        flag_mut.write_volatile(0u32);
        shm_fence();
        shm_write32(tail_off, tail.wrapping_add(1));
    }
    Some(data)
}

/// Reset all mailboxes to empty — used during SHM init/re-join.
pub unsafe fn mbox_reset_all() {
    for node in 0..MAX_INSTANCES {
        let h_off = OFF_MBOX_HEAD + node * 4;
        let t_off = OFF_MBOX_TAIL + node * 4;
        unsafe {
            shm_write32(h_off, 0);
            shm_write32(t_off, 0);
        }
        for slot in 0..MBOX_CAPACITY {
            let entry_base = mbox_entries_off(node) + slot * MBOX_SLOT_SIZE;
            unsafe {
                let flag = (SHM_BASE + entry_base) as *mut u32;
                flag.write_volatile(0u32);
                for b in 0..crate::cxl::ring::MSG_SIZE {
                    let p = (SHM_BASE + entry_base + 4 + b) as *mut u8;
                    p.write_volatile(0u8);
                }
            }
        }
    }
    unsafe { shm_fence(); }
}
