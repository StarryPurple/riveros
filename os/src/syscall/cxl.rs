use crate::mm::FRAME_ALLOCATOR;
use crate::mm::PAGE_MIGRATOR;
use crate::task::current_user_token;
use crate::mm::translated_refmut;
use core::mem::size_of;
use crate::task::current_process;
use crate::config::PAGE_SIZE;
use crate::mm::{MapArea, MapType, MapPermission};
use crate::mm::VirtAddr;

/// `size`: the size of the memory to map
pub fn sys_cxl_mmap(size: usize) -> isize {
  let page_count = (size + PAGE_SIZE - 1) / PAGE_SIZE;
  let process = current_process();
  let mut inner = process.inner_exclusive_access();
  let start_va = inner.memory_set.find_mmap_base(page_count).unwrap();
  let end_va: VirtAddr = (start_va.0 + page_count * PAGE_SIZE).into();
  let area = MapArea::new(start_va, end_va, MapType::FramedShm, MapPermission::R | MapPermission::W | MapPermission::U);
  inner.memory_set.push(area, None);
  start_va.0 as isize
}

/// Requires to release the whole mapped area. Not like Linux.
pub fn sys_cxl_munmap(ptr: usize, _size: usize) -> isize {
  let process = current_process();
  let mut inner = process.inner_exclusive_access();
  inner.memory_set.remove_area_with_start_vpn(ptr.into());
  0
}

/// Write 60 bytes into the shared ring buffer (lock-free, non-blocking).
pub fn sys_cxl_ring_push(buf: *const u8) -> isize {
    let mut msg = [0u8; crate::cxl::ring::MSG_SIZE];
    let token = current_user_token();
    let src = unsafe { crate::mm::translated_byte_buffer(token, buf, msg.len()) };
    let mut copied = 0usize;
    for seg in src {
        let len = seg.len().min(msg.len() - copied);
        msg[copied..copied + len].copy_from_slice(&seg[..len]);
        copied += len;
        if copied >= msg.len() { break; }
    }
    if unsafe { crate::cxl::ring::push(&msg) } { 0 } else { -1 }
}

/// Read 60 bytes from the shared ring buffer (lock-free, non-blocking).
pub fn sys_cxl_ring_pop(buf: *mut u8) -> isize {
    let result = unsafe { crate::cxl::ring::pop() };
    match result {
        None => -1,
        Some(msg) => {
            let token = current_user_token();
            let dst = unsafe { crate::mm::translated_byte_buffer(token, buf, msg.len()) };
            let mut copied = 0usize;
            for seg in dst {
                let len = seg.len().min(msg.len() - copied);
                seg[..len].copy_from_slice(&msg[copied..copied + len]);
                copied += len;
                if copied >= msg.len() { break; }
            }
            0
        }
    }
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

// ── Syscall wrappers for SHM reference counting + GC ──

pub fn sys_shm_alloc_page() -> isize {
    match crate::cxl::allocator::shm_alloc_page() {
        Some(idx) => idx as isize,
        None => -1,
    }
}

pub fn sys_shm_free_page(idx: usize) -> isize {
    crate::cxl::allocator::shm_free_page(idx);
    0
}

pub fn sys_shm_ref_page(idx: usize) -> isize {
    unsafe {
        crate::cxl::lock::bakery_lock(crate::cxl::allocator::me());
        crate::cxl::refcnt::shm_ref_page(idx);
        crate::cxl::lock::bakery_unlock(crate::cxl::allocator::me());
    }
    0
}

pub fn sys_shm_unref_page(idx: usize) -> isize {
    unsafe {
        crate::cxl::lock::bakery_lock(crate::cxl::allocator::me());
        crate::cxl::refcnt::shm_unref_page(idx);
        crate::cxl::lock::bakery_unlock(crate::cxl::allocator::me());
    }
    0
}

pub fn sys_shm_gc_collect() -> isize {
    unsafe { crate::cxl::gc::shm_gc_collect() as isize }
}