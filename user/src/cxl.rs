use super::*;

#[derive(Debug, Default)]
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

pub fn query_cxl_meminfo(info: &mut CxlMemInfo) -> isize {
    sys_cxl_meminfo(info as *mut _ as *mut u8)
}

pub fn cxl_mmap(slow_count: usize) -> isize {
    sys_cxl_mmap(slow_count)
}