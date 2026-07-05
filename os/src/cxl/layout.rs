//! ivshmem shared-memory layout constants.
//!
//! Physical layout of the 64 MB ivshmem BAR:
//!
//!   Page 0         Critical header (magic, bootstrap, lock metadata)
//!   Pages 1..31    Arrays too large for page 0 (freelist, ref_cnt, owner, GC)
//!   Pages 32..     Data pages available for allocation

use crate::config::IVSHMEM_BAR_BASE;

// ── Globals ──

pub const SHM_BASE:     usize = IVSHMEM_BAR_BASE;
pub const SHM_SIZE:     usize = 0x0400_0000;               // 64 MB
pub const PAGE_SIZE:    usize = 0x1000;
pub const PAGE_COUNT:   usize = SHM_SIZE / PAGE_SIZE;      // 16384

// ── Region split ──

pub const HEADER_PAGES:     usize = 32;                    // 128 KB reserved
pub const DATA_START:       usize = HEADER_PAGES;          // first allocatable page
pub const DATA_PAGE_COUNT:  usize = PAGE_COUNT - HEADER_PAGES;

// ── Page 0 offsets (all u32/u64 aligned) ──

pub const OFF_MAGIC:            usize = 0x000;   // u64
pub const OFF_N_INSTANCES:      usize = 0x008;   // u32
pub const OFF_DATA_START:       usize = 0x00C;   // u32
pub const OFF_TOTAL_PAGES:      usize = 0x010;   // u32
pub const OFF_BOOTSTRAP_LOCK:   usize = 0x014;   // u32 (1 = init in progress, 0 = done)

// Bakery lock
pub const OFF_CHOOSING:         usize = 0x040;   // [u8; MAX_INST]
pub const OFF_NUMBER:           usize = 0x044;   // [u64; MAX_INST]
pub const MAX_INSTANCES:        usize = 4;

// Freelist head
pub const OFF_FREE_HEAD:        usize = 0x080;   // u32

// Vector clock
pub const OFF_GLOBAL_VC:        usize = 0x090;   // [u64; MAX_INST]

// GC pending head (index into GC_PENDING_OFF array)
pub const OFF_GC_HEAD:          usize = 0x0B0;   // u32

// GC pending entry size in the GC_PENDING_OFF region
pub const GC_ENTRY_SIZE:        usize = 16;
pub const GC_PENDING_ENTRIES:   usize = 4096;

// Ring 0 (SPSC, 64 slots × 64 B = 4 KB) — Server->Client
pub const OFF_TX_HEAD:        usize = 0x100;   // u32
pub const OFF_TX_TAIL:        usize = 0x104;   // u32
pub const OFF_TX_ENTRIES:     usize = 0x200;   // [RingEntry; 64]
pub const RING_CAPACITY:        usize = 64;

// Ring 1 (SPSC, same layout) — Client->Server
pub const OFF_RX_HEAD:       usize = 0x0C0;   // u32 (was unused in page 0 header)
pub const OFF_RX_TAIL:       usize = 0x0C4;   // u32
pub const OFF_RX_ENTRIES:    usize = 0x3F03000; // inside cross ring data area (safe, 4KB)

// Mailbox (single-slot message passing)
pub const OFF_MBOX_SENDER:      usize = 0x500;   // u32
pub const OFF_MBOX_PAGE:        usize = 0x504;   // u32
pub const OFF_MBOX_READY:       usize = 0x508;   // u32

// ── Metadata array pages (Pages 1..31) ──
//
// Page 1..7    frelelist:     [u32; DATA_PAGE_COUNT]   (64 KB)
// Page 8..14   ref_count:     [u32; DATA_PAGE_COUNT]   (64 KB)
// Page 15      owner:         [u8;  DATA_PAGE_COUNT]   (16 KB)
// Page 16..31  GC pending:    [PendingEntry; N]         (64 KB)

pub const FREELIST_PAGE:        usize = 1;
pub const REFCOUNT_PAGE:        usize = 8;
pub const OWNER_PAGE:           usize = 15;
pub const GC_PENDING_PAGE:      usize = 16;

pub const FREELIST_OFF:     usize = FREELIST_PAGE * PAGE_SIZE;
pub const REFCOUNT_OFF:     usize = REFCOUNT_PAGE * PAGE_SIZE;
pub const OWNER_OFF:        usize = OWNER_PAGE  * PAGE_SIZE;
pub const GC_PENDING_OFF:   usize = GC_PENDING_PAGE * PAGE_SIZE;

// Magic value
pub const SHM_MAGIC: u64 = 0x5348_4D45_4D55_4E53; // "SHMEMUNS" in hex

/// First physical page number of the SHM data region.
pub const SHM_DATA_PPN_BASE: usize = (SHM_BASE / PAGE_SIZE) + HEADER_PAGES;

// ── Shared-memory access primitives ──

use core::arch::asm;

#[inline(always)]
pub unsafe fn shm_fence() {
    unsafe { asm!("fence iorw, iorw"); }
}

#[inline(always)]
pub unsafe fn shm_read32(off: usize) -> u32 {
    let ptr = (SHM_BASE + off) as *const u32;
    unsafe { shm_fence(); }
    unsafe { ptr.read_volatile() }
}

#[inline(always)]
pub unsafe fn shm_read64(off: usize) -> u64 {
    let ptr = (SHM_BASE + off) as *const u64;
    unsafe { shm_fence(); }
    unsafe { ptr.read_volatile() }
}

#[inline(always)]
pub unsafe fn shm_write32(off: usize, val: u32) {
    let ptr = (SHM_BASE + off) as *mut u32;
    unsafe { ptr.write_volatile(val); }
    unsafe { shm_fence(); }
}

#[inline(always)]
pub unsafe fn shm_write64(off: usize, val: u64) {
    let ptr = (SHM_BASE + off) as *mut u64;
    unsafe { ptr.write_volatile(val); }
    unsafe { shm_fence(); }
}
