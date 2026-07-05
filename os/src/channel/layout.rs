use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const RING_F_NEED_WAKE_C: u32 = 1 << 0;
pub const RING_F_NEED_WAKE_P: u32 = 1 << 1;
pub const RING_F_PEER_READY: u32 = 1 << 2;
pub const RING_F_SHUTDOWN: u32 = 1 << 3;

/// Layout in shared memory (24 bytes header + data[N]):
///   [0..8)  head:  AtomicU64  — written by consumer
///   [8..16) tail:  AtomicU64  — written by producer
///  [16..20) flags: AtomicU32  — written by both
///  [20..24) capacity: u32     — write-once at init
///  [24..)   data: [u8]        — ring buffer payload
#[repr(C, align(8))]
pub struct RingHeader {
    pub head: AtomicU64,
    pub tail: AtomicU64,
    pub flags: AtomicU32,
    pub capacity: u32,
}

impl RingHeader {
    pub const fn header_bytes() -> usize {
        24
    }

    pub fn init(&self, capacity: u32) {
        self.head.store(0, Ordering::Release);
        self.tail.store(0, Ordering::Release);
        self.flags.store(0, Ordering::Release);
        unsafe {
            let cap_ptr = &self.capacity as *const u32 as *mut u32;
            cap_ptr.write_volatile(capacity);
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity as usize
    }

    /// Returns a pointer past the header — the start of the data area.
    pub fn data_ptr(&self) -> *mut u8 {
        (self as *const Self as usize + 24) as *mut u8
    }
}
