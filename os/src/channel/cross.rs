//! Cross-VM ring buffer in ivshmem shared memory.
//!
//! Two fixed SPSC ring buffers at known offsets in the 64 MB ivshmem BAR.
//! Both Host (via mmap of the backing file) and Guest (via PCI BAR)
//! know the exact physical offsets, so no registration protocol is needed.
//!
//! Layout inside ivshmem BAR:
//!   0x3F0_0000  Ring 0 (Host → Guest):  8 KB  (header + ~8 KB data)
//!   0x3F0_2000  Ring 1 (Guest → Host):  8 KB

use crate::config::{IVSHMEM_BAR_BASE, PAGE_SIZE};
use crate::mm::{MapArea, MapPermission, MapType, VirtAddr};
use crate::sync::UPIntrFreeCell;
use crate::task::current_process;

use super::layout::RingHeader;

pub const CROSS_BASE: usize = 0x3F0_0000;
pub const RING_SIZE: usize = 0x2000; // 8 KB per ring
pub const RING_CAPACITY: usize = RING_SIZE - 24; // ~8152 bytes data

pub fn cross_create(vaddr_out: *mut usize) -> isize {
    let total = 2 * RING_SIZE; // 16 KB for both rings
    let page_count = (total + PAGE_SIZE - 1) / PAGE_SIZE;
    let pa_base = IVSHMEM_BAR_BASE + CROSS_BASE;

    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let start_va = inner
        .memory_set
        .find_mmap_base(page_count)
        .expect("cross_create: no VA space");
    let end_va: VirtAddr = (start_va.0 + page_count * PAGE_SIZE).into();

    let vpn_base = start_va.floor();
    let ppn_base = pa_base >> 12;
    let pn_offset = ppn_base as isize - vpn_base.0 as isize;

    let area = MapArea::new(
        start_va,
        end_va,
        MapType::Linear(pn_offset),
        MapPermission::R | MapPermission::W | MapPermission::U,
    );
    inner.memory_set.push(area, None);
    drop(inner);

    // Init ring 0 (Host → Guest): producer=Host, consumer=Guest
    let r0_pa = pa_base;
    let r0 = unsafe { &*(r0_pa as *const RingHeader) };
    r0.init(RING_CAPACITY as u32);

    // Init ring 1 (Guest → Host): producer=Guest, consumer=Host
    let r1_pa = pa_base + RING_SIZE;
    let r1 = unsafe { &*(r1_pa as *const RingHeader) };
    r1.init(RING_CAPACITY as u32);

    let token = crate::task::current_user_token();
    let dst = unsafe { crate::mm::translated_refmut(token, vaddr_out) };
    *dst = start_va.0;

    0
}
