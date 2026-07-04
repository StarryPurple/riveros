use super::*;

pub const CXL_CARD_COUNT: usize = 32;

#[derive(Debug, Default)]
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

pub fn query_cxl_meminfo(info: &mut CxlMemInfo) -> isize {
    sys_cxl_meminfo(info as *mut _ as *mut u8)
}

pub fn cxl_mmap(size: usize) -> isize {
    sys_cxl_mmap(size)
}

pub fn cxl_munmap(ptr: usize, size: usize) -> isize {
    sys_cxl_munmap(ptr, size)
}

pub fn add_cxl_card(card_id: usize) -> isize {
    sys_cxl_add_card(card_id)
}

pub fn remove_cxl_card(card_id: usize) -> isize {
    sys_cxl_remove_card(card_id)
}

pub fn cxl_route(key: u64) -> isize {
    sys_cxl_route(key)
}