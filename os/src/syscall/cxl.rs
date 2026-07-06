use crate::mm::FRAME_ALLOCATOR;
use crate::mm::PAGE_MIGRATOR;
use crate::mm::{
    frame_alloc_fast, frame_alloc_slow, frame_alloc_slow_route, frame_dealloc, copy_page, replace_ppn_global,
    translated_refmut, PhysAddr, PhysPageNum, VirtAddr, VirtPageNum,
    MapArea, MapType, MapPermission,
};
use crate::task::current_user_token;
use crate::task::current_process;
use crate::config::{PAGE_SIZE, CXL_CARD_COUNT, CXL_MEMORY_RANGES};
use crate::cxl::{CxlCardId, CXL_CARD_MANAGER};
use core::mem::size_of;
use core::mem::forget;
use alloc::vec::Vec;

/// `size`: the size of the memory to map (continuous region, single route)
pub fn sys_cxl_mmap(size: usize) -> isize {
  let page_count = (size + PAGE_SIZE - 1) / PAGE_SIZE;
  let process = current_process();
  let mut inner = process.inner_exclusive_access();
  let start_va = match inner.memory_set.find_mmap_base(page_count) {
      Some(va) => va,
      None => return -1,
  };
  let end_va: VirtAddr = (start_va.0 + page_count * PAGE_SIZE).into();
  let card_id = match CXL_CARD_MANAGER.exclusive_access()
      .hash_ring.route(start_va.0 as u64)
      .copied()
  {
      Some(c) => c,
      None => return -2, // no CXL card available
  };
  inner.memory_set.push_cxl_fallback(start_va, end_va, card_id, MapPermission::R | MapPermission::W | MapPermission::U);
  start_va.0 as isize
}

/// Requires to release the whole mapped area. Not like Linux.
pub fn sys_cxl_munmap(ptr: usize, _size: usize) -> isize {
  let process = current_process();
  let mut inner = process.inner_exclusive_access();
  inner.memory_set.remove_area_with_start_vpn(ptr.into());
  0
}

#[repr(C)]
pub struct CxlMemInfo {
    pub version: u32,
    pub size: u32,

    pub promote_count: u64,
    pub demote_count: u64,
    pub fast_alloc_count: u64,
    pub slow_alloc_count: [u64; CXL_CARD_COUNT],
    pub fast_dealloc_count: u64,
    pub slow_dealloc_count: [u64; CXL_CARD_COUNT],
}

/// register card to hash ring + frame allocator + rehash data migration
pub fn sys_cxl_add_card(card_id: usize) -> isize {
    if card_id >= CXL_CARD_COUNT {
        return -1;
    }
    let (start, end) = CXL_MEMORY_RANGES[card_id];
    let card = CxlCardId(card_id);

    FRAME_ALLOCATOR.exclusive_access().add_slow(
        card,
        PhysAddr::from(start).ceil(),
        PhysAddr::from(end).floor(),
    );

    CXL_CARD_MANAGER.exclusive_access().add_card(card);

    // rehash: iterate all existing CXL pages, route them to new ring, and migrate data
    let pages_to_migrate: Vec<(PhysPageNum, VirtPageNum, usize)> = {
        let mgr = CXL_CARD_MANAGER.exclusive_access();
        mgr.ppn2card
            .iter()
            .filter(|(_, old_card)| **old_card != card)
            .filter_map(|(ppn, _)| {
                mgr.ppn2vpn.get(ppn).and_then(|(vpn, pid)| {
                    let new_card_for_key = *mgr.hash_ring.route(vpn.0 as u64)?;
                    if new_card_for_key == card {
                        Some((*ppn, *vpn, *pid))
                    } else {
                        None
                    }
                })
            })
            .collect()
    };

    for (old_ppn, vpn, pid) in pages_to_migrate {
        let new_frame = match frame_alloc_slow(card) {
            Some(f) => f,
            None => break, // new card full, stop rehash migration
        };
        let new_ppn = new_frame.ppn;
        copy_page(old_ppn, new_ppn);
        replace_ppn_global(old_ppn, new_ppn);
        CXL_CARD_MANAGER.exclusive_access().track_page_vpn(new_ppn, vpn, pid);
        frame_dealloc(old_ppn);
        forget(new_frame);
    }

    card_id as isize
}

/// eject card: re-route pages to other cards + remove from ring + remove from frame allocator
pub fn sys_cxl_remove_card(card_id: usize) -> isize {
    let card = CxlCardId(card_id);

    // 1. get all pages of this card
    let pages: Vec<(PhysPageNum, VirtPageNum, usize)> = {
        let mgr = CXL_CARD_MANAGER.exclusive_access();
        if !mgr.has_card(card) {
            return -1;
        }
        mgr.get_card_pages(card)
    };

    // 2. remove card's vnodes from ring FIRST (so re-route won't return this card)
    CXL_CARD_MANAGER.exclusive_access().hash_ring.remove_card(card);
    CXL_CARD_MANAGER.exclusive_access().cards.remove(&card);

    // 3. re-route each page to another CXL card (or promote to DRAM)
    for (ppn, vpn, pid) in pages {
        if let Some(new_frame) = frame_alloc_slow_route(vpn) {
            // re-route to another CXL card
            let new_ppn = new_frame.ppn;
            copy_page(ppn, new_ppn);
            replace_ppn_global(ppn, new_ppn);
            CXL_CARD_MANAGER.exclusive_access().track_page_vpn(new_ppn, vpn, pid);
            frame_dealloc(ppn);
            forget(new_frame);
        } else {
            // all other cards full, promote to DRAM
            let new_frame = frame_alloc_fast().unwrap();
            let new_ppn = new_frame.ppn;
            copy_page(ppn, new_ppn);
            replace_ppn_global(ppn, new_ppn);
            frame_dealloc(ppn);
            forget(new_frame);
        }
    }

    // 4. remove card from frame allocator
    FRAME_ALLOCATOR.exclusive_access().try_eject_slow(card);

    0
}

pub fn sys_cxl_route(key: u64) -> isize {
    let result = CXL_CARD_MANAGER
        .exclusive_access()
        .hash_ring
        .route(key)
        .copied();
    match result {
        Some(card_id) => card_id.0 as isize,
        None => -1,
    }
}

pub fn sys_cxl_meminfo(buf: *mut CxlMemInfo) -> isize {
    let token = current_user_token();
    let info = translated_refmut(token, buf);

    info.version = 2;
    info.size = size_of::<CxlMemInfo>() as u32;

    let alloc = FRAME_ALLOCATOR.exclusive_access();
    info.fast_alloc_count = alloc.fast_alloc_count;
    info.slow_alloc_count = alloc.slow_alloc_count.clone();
    info.fast_dealloc_count = alloc.fast_dealloc_count;
    info.slow_dealloc_count = alloc.slow_dealloc_count.clone();

    let mig = PAGE_MIGRATOR.exclusive_access();
    info.promote_count = mig.promote_count;
    info.demote_count = mig.demote_count;

    0
}