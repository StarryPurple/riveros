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

pub fn cxl_tx_push(data: &[u8; 60]) -> isize {
    sys_cxl_tx_push(data as *const u8)
}

pub fn cxl_tx_pop(data: &mut [u8; 60]) -> isize {
    sys_cxl_tx_pop(data as *mut u8)
}
pub fn cxl_rx_push(data: &[u8; 60]) -> isize {
    sys_cxl_rx_push(data as *const u8)
}
pub fn cxl_rx_pop(data: &mut [u8; 60]) -> isize {
    sys_cxl_rx_pop(data as *mut u8)
}

pub fn shm_alloc_page() -> isize {
    sys_shm_alloc_page()
}
pub fn shm_free_page(idx: usize) -> isize {
    sys_shm_free_page(idx)
}
pub fn shm_ref_page(idx: usize) -> isize {
    sys_shm_ref_page(idx)
}
pub fn shm_unref_page(idx: usize) -> isize {
    sys_shm_unref_page(idx)
}
pub fn shm_gc_collect() -> isize {
    sys_shm_gc_collect()
}

pub fn get_instance_id() -> usize {
    sys_get_instance_id() as usize
}

/// Attach sender+checksum to a message buffer (60 bytes).
/// Byte 57 = sender, bytes 58..59 = u16le additive checksum of bytes 0..57.
pub fn msg_seal(m: &mut [u8; 60], sender: usize) {
    m[57] = sender as u8;
    let sum: u16 = m[0..58].iter().fold(0u16, |s, &b| s.wrapping_add(b as u16));
    m[58..60].copy_from_slice(&sum.to_le_bytes());
}

/// Verify checksum. Returns Some(sender) on success, None on corruption.
pub fn msg_verify(m: &[u8; 60]) -> Option<usize> {
    let expected = u16::from_le_bytes([m[58], m[59]]);
    let computed: u16 = m[0..58].iter().fold(0u16, |s, &b| s.wrapping_add(b as u16));
    if computed != expected { return None; }
    Some(m[57] as usize)
}

/// Extract tag byte from a message.
pub fn msg_tag(m: &[u8; 60]) -> u8 { m[0] }

/// Send 60 bytes to target node's mailbox. Returns 0 on success, -1 if full.
pub fn cxl_mbox_send(to: usize, data: &[u8; 60]) -> isize {
    sys_cxl_mbox_send(to, data as *const u8)
}

/// Receive 60 bytes from own mailbox (non-blocking). Returns 0 on success, -1 if empty.
pub fn cxl_mbox_recv(data: &mut [u8; 60]) -> isize {
    sys_cxl_mbox_recv(data as *mut u8)
}