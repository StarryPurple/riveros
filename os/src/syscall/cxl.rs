use crate::mm::FRAME_ALLOCATOR;
use crate::mm::PAGE_MIGRATOR;
use crate::task::current_user_token;
use crate::mm::translated_refmut;
use core::mem::size_of;
use crate::task::current_process;
use crate::config::PAGE_SIZE;
use crate::mm::{MemorySet, MapArea, MapType, MapPermission};
use crate::mm::VirtAddr;

/// `slow_count`: the number of slow memory pages to map
pub fn sys_cxl_mmap(slow_count: usize) -> isize {
  let process = current_process();
  let mut inner = process.inner_exclusive_access();
  let start_va = inner.memory_set.find_mmap_base(slow_count).unwrap();
  let end_va: VirtAddr = (start_va.0 + slow_count * PAGE_SIZE).into();
  let area = MapArea::new(start_va, end_va, MapType::FramedSlow, MapPermission::R | MapPermission::W | MapPermission::U);
  inner.memory_set.push(area, None);
  start_va.0 as isize
}

/// Requires to release the whole mapped area. No like Linux.
pub fn sys_cxl_munmap(ptr: usize, _count: usize) -> isize {
  let process = current_process();
  let mut inner = process.inner_exclusive_access();
  inner.memory_set.remove_area_with_start_vpn(ptr.into());
  0
}

#[repr(C)]
pub struct CxlMemInfo {
    pub version: u32, // reserved for this struct
    pub size: u32,    // size of this struct

    pub promote_count: u64,
    pub demote_count: u64,
    pub fast_alloc_count: u64,
    pub slow_alloc_count: u64,
    pub fast_dealloc_count: u64,
    pub slow_dealloc_count: u64,
}

pub fn sys_cxl_meminfo(buf: *mut CxlMemInfo) -> isize {
    let token = current_user_token();
    let info = translated_refmut(token, buf);

    info.version = 1;
    info.size = size_of::<CxlMemInfo>() as u32;

    let alloc = FRAME_ALLOCATOR.exclusive_access();
    info.fast_alloc_count = alloc.fast_alloc_count;
    info.slow_alloc_count = alloc.slow_alloc_count;
    info.fast_dealloc_count = alloc.fast_dealloc_count;
    info.slow_dealloc_count = alloc.slow_dealloc_count;

    let mig = PAGE_MIGRATOR.exclusive_access();
    info.promote_count = mig.promote_count;
    info.demote_count = mig.demote_count;

    0
}