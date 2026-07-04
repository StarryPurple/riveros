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

pub fn cxl_mmap(size: usize) -> isize {
    sys_cxl_mmap(size)
}

pub fn cxl_munmap(ptr: usize, size: usize) -> isize {
    sys_cxl_munmap(ptr, size)
}

pub fn cxl_ring_push(data: &[u8; 60]) -> isize {
    sys_cxl_ring_push(data as *const u8)
}

pub fn cxl_ring_pop(data: &mut [u8; 60]) -> isize {
    sys_cxl_ring_pop(data as *mut u8)
}