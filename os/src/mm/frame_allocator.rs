use super::{PhysAddr, PhysPageNum};
use crate::config::MEMORY_END;
use crate::sync::UPIntrFreeCell;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::fmt::{self, Debug, Formatter};
use core::ops::Range;
use lazy_static::*;
use crate::drivers::bus::pci::{pci_scan, is_ivshmem, config_ivshmem_bar};

pub struct FrameTracker {
    pub ppn: PhysPageNum,
}

impl FrameTracker {
    pub fn new(ppn: PhysPageNum) -> Self {
        // page cleaning
        let bytes_array = ppn.get_bytes_array();
        for i in bytes_array {
            *i = 0;
        }
        Self { ppn }
    }
}

impl Debug for FrameTracker {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("FrameTracker:PPN={:#x}", self.ppn.0))
    }
}

impl Drop for FrameTracker {
    fn drop(&mut self) {
        frame_dealloc(self.ppn);
    }
}

pub struct StackFrameAllocator {
    start: usize,
    current: usize,
    end: usize,
    recycled: Vec<usize>,
}

impl StackFrameAllocator {
    pub fn init(&mut self, l: PhysPageNum, r: PhysPageNum) {
        self.start = l.0;
        self.current = l.0;
        self.end = r.0;
        // println!("last {} Physical Frames.", self.end - self.start);
    }
    pub fn stack_range(&self) -> Range<PhysPageNum> {
        (self.start.into())..(self.end.into())
    }
    pub fn new() -> Self {
        Self {
            start: 0,
            current: 0,
            end: 0,
            recycled: Vec::new(),
        }
    }
    pub fn alloc(&mut self) -> Option<PhysPageNum> {
        if let Some(ppn) = self.recycled.pop() {
            Some(ppn.into())
        } else if self.current == self.end {
            None
        } else {
            self.current += 1;
            Some((self.current - 1).into())
        }
    }
    #[allow(unused)]
    pub fn alloc_more(&mut self, pages: usize) -> Option<Vec<PhysPageNum>> {
        if self.current + pages >= self.end {
            None
        } else {
            self.current += pages;
            let arr: Vec<usize> = (1..pages + 1).collect();
            let v = arr.iter().map(|x| (self.current - x).into()).collect();
            Some(v)
        }
    }
    pub fn dealloc(&mut self, ppn: PhysPageNum) {
        let ppn = ppn.0;
        // validity check
        if ppn >= self.current || self.recycled.iter().any(|&v| v == ppn) {
            let ra: usize;
            let frame_ra: usize;
            unsafe {
                core::arch::asm!("mv {}, ra", out(reg) ra);
                core::arch::asm!("ld {}, 8(sp)", out(reg) frame_ra);
            }
            panic!("Frame ppn={:#x} not alloc! cur={:#x} recycled={} ra={:#x} f_ra={:#x}",
                ppn, self.current, self.recycled.iter().any(|&v| v == ppn), ra, frame_ra);
        }
        // recycle
        self.recycled.push(ppn);
    }
}

#[derive(Debug, PartialEq)]
pub enum MemoryTier {
    Fast,
    Slow,
}

pub struct TieredFrameAllocator {
    fast: StackFrameAllocator,
    slow: Option<StackFrameAllocator>,
    pinned_pages: BTreeSet<PhysPageNum>,

    /// Statistics
    pub fast_alloc_count: u64,
    pub slow_alloc_count: u64,
    pub fast_dealloc_count: u64,
    pub slow_dealloc_count: u64,
}

impl TieredFrameAllocator {
    pub fn new() -> Self {
        Self {
            fast: StackFrameAllocator::new(),
            slow: None,
            pinned_pages: BTreeSet::new(),
            fast_alloc_count: 0,
            slow_alloc_count: 0,
            fast_dealloc_count: 0,
            slow_dealloc_count: 0,
        }
    }
    /// Fast tier must be valid.
    pub fn init(&mut self, l: PhysPageNum, r: PhysPageNum) {
        self.fast.init(l, r);
    }
    pub fn add_slow(&mut self, l: PhysPageNum, r: PhysPageNum) {
        let mut slow = StackFrameAllocator::new();
        slow.init(l, r);
        self.slow = Some(slow);
    }
    pub fn alloc_fast(&mut self) -> Option<PhysPageNum> {
        self.fast_alloc_count += 1;
        self.fast.alloc()
    }
    pub fn alloc_slow(&mut self) -> Option<PhysPageNum> {
        self.slow_alloc_count += 1;
        self.slow.as_mut()?.alloc()
    }
    /// Alloc from fast first, then fallback to slow.
    pub fn alloc(&mut self) -> Option<PhysPageNum> {
        self.alloc_fast().or_else(|| self.alloc_slow())
    }
    pub fn alloc_more(&mut self, pages: usize) -> Option<Vec<PhysPageNum>> {
        let mut result = Vec::with_capacity(pages);
        for _ in 0..pages {
            match self.alloc() {
                Some(ppn) => result.push(ppn),
                None => {
                    for ppn in result.iter() {
                        self.dealloc(*ppn);
                    }
                    return None;
                }
            }
        }
        // sync with virtio.rs: return a decreasing order of ppns
        result.reverse();
        Some(result)
    }
    pub fn dealloc(&mut self, ppn: PhysPageNum) {
        self.pinned_pages.remove(&ppn);
        if self.fast.stack_range().contains(&ppn) {
            self.fast_dealloc_count += 1;
            self.fast.dealloc(ppn);
            return;
        } else if let Some(ref mut slow) = self.slow {
            if slow.stack_range().contains(&ppn) {
                self.slow_dealloc_count += 1;
                slow.dealloc(ppn);
                return;
            }
        }
        panic!("Frame ppn={:#x} is not in fast or slow tier", ppn.0);
    }
    pub fn mark_pinned(&mut self, ppn: PhysPageNum) {
        self.pinned_pages.insert(ppn);
    }
    pub fn is_pinned(&self, ppn: PhysPageNum) -> bool {
        self.pinned_pages.contains(&ppn)
    }
    /// return None if this is a kernal page
    pub fn page_tier(&self, ppn: PhysPageNum) -> Option<MemoryTier> {
        if self.fast.stack_range().contains(&ppn) {
            return Some(MemoryTier::Fast);
        } else if let Some(ref slow) = self.slow {
            if slow.stack_range().contains(&ppn) {
                return Some(MemoryTier::Slow);
            }
        }
        None
    }
}


type FrameAllocatorImpl = TieredFrameAllocator;

lazy_static! {
    pub static ref FRAME_ALLOCATOR: UPIntrFreeCell<FrameAllocatorImpl> =
        unsafe { UPIntrFreeCell::new(FrameAllocatorImpl::new()) };
}

pub fn init_frame_allocator() {
    unsafe extern "C" {
        safe fn ekernel();
    }
    // fast tier: all DRAM
    FRAME_ALLOCATOR.exclusive_access().init(
        PhysAddr::from(linker_symbol_addr!(ekernel)).ceil(),
        PhysAddr::from(MEMORY_END).floor(),
    );

    // CXL shared memory — try ivshmem, init if found
    let devices = pci_scan();
    if let Some(iv) = devices.iter().find(|d| is_ivshmem(d)) {
        config_ivshmem_bar(iv);
        let (my_id, _first) = crate::cxl::bootstrap::shm_init();
        println!("[CXL] ivshmem ready, instance {}", my_id);
    } else {
        println!("[CXL] no ivshmem found — shared-memory disabled");
    }
}

#[allow(unused)]
pub fn add_slow_frame_allocator(l: PhysPageNum, r: PhysPageNum) {
    FRAME_ALLOCATOR.exclusive_access().add_slow(l, r);
}

pub fn frame_alloc() -> Option<FrameTracker> {
    FRAME_ALLOCATOR
        .exclusive_access()
        .alloc()
        .map(FrameTracker::new)
}

pub fn frame_alloc_slow() -> Option<FrameTracker> {
    FRAME_ALLOCATOR
        .exclusive_access()
        .alloc_slow()
        .map(FrameTracker::new)
}

pub fn frame_alloc_more(num: usize) -> Option<Vec<FrameTracker>> {
    FRAME_ALLOCATOR
        .exclusive_access()
        .alloc_more(num)
        .map(|x| x.iter().map(|&t| FrameTracker::new(t)).collect())
}

pub fn frame_dealloc(ppn: PhysPageNum) {
    if crate::cxl::allocator::is_shm_page(ppn) {
        if let Some(idx) = crate::cxl::allocator::ppn_to_shm_idx(ppn) {
            crate::cxl::allocator::shm_free_page(idx);
            return;
        }
    }
    FRAME_ALLOCATOR.exclusive_access().dealloc(ppn);
}

#[allow(unused)]
pub fn frame_allocator_test() {
    let mut v: Vec<FrameTracker> = Vec::new();
    for i in 0..5 {
        let frame = frame_alloc().unwrap();
        println!("{:?}", frame);
        v.push(frame);
    }
    v.clear();
    for i in 0..5 {
        let frame = frame_alloc().unwrap();
        println!("{:?}", frame);
        v.push(frame);
    }
    drop(v);
    println!("frame_allocator_test passed!");
}

#[allow(unused)]
pub fn frame_allocator_alloc_more_test() {
    let mut v: Vec<FrameTracker> = Vec::new();
    let frames = frame_alloc_more(5).unwrap();
    for frame in &frames {
        println!("{:?}", frame);
    }
    v.extend(frames);
    v.clear();
    let frames = frame_alloc_more(5).unwrap();
    for frame in &frames {
        println!("{:?}", frame);
    }
    drop(v);
    println!("frame_allocator_test passed!");
}

#[allow(unused)]
pub fn tier_alloc_test() {
    let slow = FRAME_ALLOCATOR.exclusive_access().alloc_slow().unwrap();
    let tier = FRAME_ALLOCATOR.exclusive_access().page_tier(slow);
    println!("[CXL] slow ppn={:#x}, tier={:?}", slow.0, tier);

    let fast = FRAME_ALLOCATOR.exclusive_access().alloc_fast().unwrap();
    let tier = FRAME_ALLOCATOR.exclusive_access().page_tier(fast);
    println!("[CXL] fast ppn={:#x}, tier={:?}", fast.0, tier);
}