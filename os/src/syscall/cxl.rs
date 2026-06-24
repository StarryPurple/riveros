use crate::mm::FRAME_ALLOCATOR;
use crate::mm::PAGE_MIGRATOR;
use crate::task::current_user_token;
use crate::mm::translated_refmut;
use core::mem::size_of;

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