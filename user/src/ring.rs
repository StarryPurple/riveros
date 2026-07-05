use super::*;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const RING_F_NEED_WAKE_C: u32 = 1 << 0;
pub const RING_F_NEED_WAKE_P: u32 = 1 << 1;
pub const RING_F_SHUTDOWN: u32 = 1 << 3;

#[repr(C, align(8))]
pub struct RingHeader {
    pub head: AtomicU64,
    pub tail: AtomicU64,
    pub flags: AtomicU32,
    pub capacity: u32,
}

pub struct LockFreeRing {
    base: *mut u8,
}

unsafe impl Send for LockFreeRing {}
unsafe impl Sync for LockFreeRing {}

impl LockFreeRing {
    pub fn new(base: *mut u8) -> Self {
        Self { base }
    }

    pub fn header(&self) -> &RingHeader {
        unsafe { &*(self.base as *const RingHeader) }
    }

    fn data_ptr(&self) -> *mut u8 {
        unsafe { self.base.add(24) }
    }

    fn capacity(&self) -> usize {
        self.header().capacity as usize
    }

    /// Try to push data into the ring. All-or-nothing. Non-blocking.
    pub fn try_push(&self, data: &[u8]) -> Result<(), i32> {
        let len = data.len();
        if len == 0 || len > self.capacity() {
            return Err(-1);
        }

        let head = self.header().head.load(Ordering::Acquire);
        let tail = self.header().tail.load(Ordering::Relaxed);

        if tail.wrapping_sub(head) + len as u64 > self.capacity() as u64 {
            return Err(-2); // full
        }

        let cap = self.capacity();
        let pos = (tail as usize) % cap;
        let dst = self.data_ptr();
        let n = core::cmp::min(len, cap - pos);
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), dst.add(pos), n);
            core::ptr::copy_nonoverlapping(data.as_ptr().add(n), dst, len - n);
        }

        self.header()
            .tail
            .store(tail + len as u64, Ordering::Release);
        Ok(())
    }

    /// Try to pop data from the ring. Non-blocking. Returns number of bytes read.
    pub fn try_pop(&self, buf: &mut [u8]) -> Result<usize, i32> {
        let max_len = buf.len();
        if max_len == 0 {
            return Ok(0);
        }

        let tail = self.header().tail.load(Ordering::Acquire);
        let head = self.header().head.load(Ordering::Relaxed);

        let available = tail.wrapping_sub(head) as usize;
        if available == 0 {
            return Err(-2); // empty
        }
        let read_len = core::cmp::min(available, max_len);

        let cap = self.capacity();
        let pos = (head as usize) % cap;
        let src = self.data_ptr();
        let n = core::cmp::min(read_len, cap - pos);
        unsafe {
            core::ptr::copy_nonoverlapping(src.add(pos), buf.as_mut_ptr(), n);
            core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr().add(n), read_len - n);
        }

        self.header()
            .head
            .store(head + read_len as u64, Ordering::Release);
        Ok(read_len)
    }

    /// Busy-poll send. Zero syscall. Best on multi-core.
    pub fn push_spin(&self, data: &[u8]) {
        loop {
            if let Ok(()) = self.try_push(data) {
                return;
            }
            core::hint::spin_loop();
        }
    }

    /// Busy-poll recv. Zero syscall. Best on multi-core.
    pub fn pop_spin(&self, buf: &mut [u8]) -> usize {
        loop {
            if let Ok(n) = self.try_pop(buf) {
                return n;
            }
            core::hint::spin_loop();
        }
    }

    /// Cooperative send: yield on contention. Good for single-core.
    pub fn push_yield(&self, data: &[u8]) {
        while self.try_push(data).is_err() {
            yield_();
        }
    }

    /// Cooperative recv: yield on contention. Good for single-core.
    pub fn pop_yield(&self, buf: &mut [u8]) -> usize {
        loop {
            if let Ok(n) = self.try_pop(buf) {
                return n;
            }
            yield_();
        }
    }

    /// After a successful operation, wake the peer if it is waiting.
    /// Clears the peer's flag (so the notification is consumed exactly once)
    /// then calls sys_ring_notify.
    fn notify_peer(&self, peer_flag: u32, fd: usize) {
        if self.header().flags.load(Ordering::Acquire) & peer_flag != 0 {
            self.header().flags.fetch_and(!peer_flag, Ordering::Release);
            sys_ring_notify(fd);
        }
    }

    /// Hybrid send: spin then fall back to kernel notification.
    pub fn push_hybrid(&self, data: &[u8], max_spin: usize, fd: usize) {
        loop {
            for _ in 0..max_spin {
                if let Ok(()) = self.try_push(data) {
                    self.notify_peer(RING_F_NEED_WAKE_C, fd);
                    return;
                }
                core::hint::spin_loop();
            }

            // Register our interest
            self.header()
                .flags
                .fetch_or(RING_F_NEED_WAKE_P, Ordering::Release);

            // Final check before blocking (missed-wakeup avoidance)
            if let Ok(()) = self.try_push(data) {
                self.header()
                    .flags
                    .fetch_and(!RING_F_NEED_WAKE_P, Ordering::Release);
                self.notify_peer(RING_F_NEED_WAKE_C, fd);
                return;
            }

            // Block — woken by consumer's notify_peer after it pops
            sys_ring_wait(fd, 0);
            self.header()
                .flags
                .fetch_and(!RING_F_NEED_WAKE_P, Ordering::Release);
        }
    }

    /// Hybrid recv: spin then fall back to kernel notification.
    pub fn pop_hybrid(&self, buf: &mut [u8], max_spin: usize, fd: usize) -> usize {
        loop {
            for _ in 0..max_spin {
                if let Ok(n) = self.try_pop(buf) {
                    self.notify_peer(RING_F_NEED_WAKE_P, fd);
                    return n;
                }
                core::hint::spin_loop();
            }

            self.header()
                .flags
                .fetch_or(RING_F_NEED_WAKE_C, Ordering::Release);

            if let Ok(n) = self.try_pop(buf) {
                self.header()
                    .flags
                    .fetch_and(!RING_F_NEED_WAKE_C, Ordering::Release);
                self.notify_peer(RING_F_NEED_WAKE_P, fd);
                return n;
            }

            sys_ring_wait(fd, 0);
            self.header()
                .flags
                .fetch_and(!RING_F_NEED_WAKE_C, Ordering::Release);
        }
    }

    pub fn is_empty(&self) -> bool {
        let head = self.header().head.load(Ordering::Acquire);
        let tail = self.header().tail.load(Ordering::Acquire);
        head == tail
    }

    pub fn is_full(&self) -> bool {
        self.used() == self.capacity()
    }

    pub fn used(&self) -> usize {
        let head = self.header().head.load(Ordering::Acquire);
        let tail = self.header().tail.load(Ordering::Acquire);
        tail.wrapping_sub(head) as usize
    }
}
