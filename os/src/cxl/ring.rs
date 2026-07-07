//! Lock-free SPSC ring buffer on ivshmem shared memory.
//!
//! 64 slots × 64 B each (4 KB total).  Producer writes at `head`,
//! consumer reads at `tail`.  No mutex — single producer & consumer
//! ensure correctness with ordered loads/stores + fences.

use super::layout::*;

pub const WINDOW: u32 = RING_CAPACITY as u32;
pub const MASK: u32 = WINDOW - 1;
pub const MSG_SIZE: usize = 60;

/// Reset TX ring to empty (safe to call when no concurrent access).
pub unsafe fn reset_tx() {
    unsafe { shm_write32(OFF_TX_HEAD, 0); }
    unsafe { shm_write32(OFF_TX_TAIL, 0); }
    for slot in 0..RING_CAPACITY {
        let entry = OFF_TX_ENTRIES + slot * 64;
        let flag = (SHM_BASE + entry) as *mut u32;
        unsafe { flag.write_volatile(0u32); }
        // Also clear the 60-byte payload so stale messages aren't misinterpreted
        for b in 0..MSG_SIZE {
            let p = (SHM_BASE + entry + 4 + b) as *mut u8;
            unsafe { p.write_volatile(0u8); }
        }
    }
}

/// Write 60 bytes into the ring.
///
/// Returns `true` on success, `false` if the ring is full.
pub unsafe fn tx_push(data: &[u8; MSG_SIZE]) -> bool {
    unsafe {
        let head = shm_read32(OFF_TX_HEAD);
        let tail = shm_read32(OFF_TX_TAIL);

        if head.wrapping_sub(tail) >= WINDOW {
            // ring appears full — check if it's stale (all flags zero)
            let mut stale = true;
            for slot in 0..RING_CAPACITY {
                let entry = OFF_TX_ENTRIES + slot * 64;
                let flag = (SHM_BASE + entry) as *const u32;
                if flag.read_volatile() != 0 {
                    stale = false;
                    break;
                }
            }
            if stale {
                // previous session left head/tail misaligned; recover
                shm_write32(OFF_TX_HEAD, 0);
                shm_write32(OFF_TX_TAIL, 0);
                crate::println_cxl!("ring recovered from stale state");
            } else {
                crate::println_cxl!("ring full: head={} tail={}", head, tail);
                return false;
            }
        }
        let slot = (head & MASK) as usize;
        let entry = OFF_TX_ENTRIES + slot * 64; // 4 B flags + 60 B data

        // write data first
        for (i, &b) in data.iter().enumerate() {
            let p = (SHM_BASE + entry + 4 + i) as *mut u8;
            p.write_volatile(b);
        }
        shm_fence();

        // flag = 1  (published)
        let flag = (SHM_BASE + entry) as *mut u32;
        flag.write_volatile(1u32);
        shm_fence();

        shm_write32(OFF_TX_HEAD, head.wrapping_add(1));
        true
    }
}

/// Read 60 bytes from the ring (non-blocking).
///
/// Returns `None` when the ring is empty.
pub unsafe fn tx_pop() -> Option<[u8; MSG_SIZE]> {
    unsafe {
        let tail = shm_read32(OFF_TX_TAIL);
        let head = shm_read32(OFF_TX_HEAD);
        if head == tail {
            return None;
        }
        let slot = (tail & MASK) as usize;
        let entry = OFF_TX_ENTRIES + slot * 64;

        // wait until the producer has set flag = 1
        let flag = (SHM_BASE + entry) as *const u32;
        while flag.read_volatile() != 1 {
            shm_fence();
        }
        shm_fence();

        let mut data = [0u8; MSG_SIZE];
        for i in 0..MSG_SIZE {
            let p = (SHM_BASE + entry + 4 + i) as *const u8;
            data[i] = p.read_volatile();
        }

        // clear flag
        let flag_mut = (SHM_BASE + entry) as *mut u32;
        flag_mut.write_volatile(0u32);
        shm_fence();

        shm_write32(OFF_TX_TAIL, tail.wrapping_add(1));
        Some(data)
    }
}

// Ring RX (Client->Server direction)

/// Reset RX ring to empty (safe when no concurrent access).
pub unsafe fn reset_rx() {
    unsafe {
        shm_write32(OFF_RX_HEAD, 0);
        shm_write32(OFF_RX_TAIL, 0);
        for slot in 0..RING_CAPACITY {
            let entry = OFF_RX_ENTRIES + slot * 64;
            let flag = (SHM_BASE + entry) as *mut u32;
            flag.write_volatile(0u32);
            for b in 0..MSG_SIZE {
                let p = (SHM_BASE + entry + 4 + b) as *mut u8;
                p.write_volatile(0u8);
            }
        }
        shm_fence();
    }
}

/// Zero out the entire cross-ring shared-memory area (used during SHM re-join
/// to clear stale data-plane data from a previous session).
pub unsafe fn reset_cross_area() {
    let base = SHM_BASE + CROSS_RING_OFFSET;
    for i in 0..CROSS_RING_TOTAL {
        let p = (base + i) as *mut u8;
        unsafe { p.write_volatile(0u8); }
    }
    unsafe { shm_fence(); }
}

/// Sanity: reset head/tail if stale session left garbage.
unsafe fn sanitize_rx() {
    let h = shm_read32(OFF_RX_HEAD);
    let t = shm_read32(OFF_RX_TAIL);
    if h >= 1024 || t >= 1024 || h < t {
        reset_rx();
    }
}

pub unsafe fn rx_push(data: &[u8; MSG_SIZE]) -> bool {
    unsafe {
        sanitize_rx();
        let head = shm_read32(OFF_RX_HEAD);
        let tail = shm_read32(OFF_RX_TAIL);
        if head.wrapping_sub(tail) >= WINDOW { return false; }
        let slot = (head & MASK) as usize;
        let entry = OFF_RX_ENTRIES + slot * 64;
        for (i, &b) in data.iter().enumerate() {
            let p = (SHM_BASE + entry + 4 + i) as *mut u8;
            p.write_volatile(b);
        }
        shm_fence();
        let flag = (SHM_BASE + entry) as *mut u32;
        flag.write_volatile(1u32);
        shm_fence();
        shm_write32(OFF_RX_HEAD, head.wrapping_add(1));
        true
    }
}

/// Block until data is available, then read it.
/// Never returns `None` — instead waits with `wfi`.
/// `wfi` causes TCG to exit the current translation block,
/// avoiding the TB-caching issue with cross-QEMU reads.
pub unsafe fn rx_pop() -> Option<[u8; MSG_SIZE]> {
    unsafe {
        sanitize_rx();
        // Phase 1: wait for data (head != tail)
        let mut tail;
        let entry;
        loop {
            tail = shm_read32(OFF_RX_TAIL);
            let head = shm_read32(OFF_RX_HEAD);
            if head != tail {
                entry = OFF_RX_ENTRIES + ((tail & MASK) as usize) * 64;
                break;
            }
            // wfi: exit TB, preventing TCG from caching the tight loop.
            core::arch::asm!("wfi", options(nostack));
            shm_fence();
        }

        // Phase 2: wait for slot flag
        let flag = (SHM_BASE + entry) as *const u32;
        while flag.read_volatile() != 1 {
            core::arch::asm!("wfi", options(nostack));
            shm_fence();
        }
        shm_fence();

        let mut data = [0u8; MSG_SIZE];
        for i in 0..MSG_SIZE {
            let p = (SHM_BASE + entry + 4 + i) as *const u8;
            data[i] = p.read_volatile();
        }
        let flag_mut = (SHM_BASE + entry) as *mut u32;
        flag_mut.write_volatile(0u32);
        shm_fence();
        shm_write32(OFF_RX_TAIL, tail.wrapping_add(1));
        Some(data)
    }
}