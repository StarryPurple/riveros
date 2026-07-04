use super::{PhysAddr, PhysPageNum, VirtPageNum};
use crate::config::{CXL_CARD_COUNT, CXL_MEMORY_RANGES, DRAM_MEMORY_END};
use crate::drivers::pci::PciDevice;
use crate::sync::UPIntrFreeCell;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use core::fmt::{self, Debug, Formatter};
use core::ops::Range;
use lazy_static::*;
use crate::drivers::bus::pci::{pci_scan, is_cxl_type3};
use crate::cxl::{CxlCardId, CXL_CARD_MANAGER};
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
    Slow(CxlCardId),
}

pub struct TieredFrameAllocator {
    fast: StackFrameAllocator,
    slow: BTreeMap<CxlCardId, StackFrameAllocator>,

    /// Statistics
    pub fast_alloc_count: u64,
    pub slow_alloc_count: [u64; CXL_CARD_COUNT],
    pub fast_dealloc_count: u64,
    pub slow_dealloc_count: [u64; CXL_CARD_COUNT],
}

impl TieredFrameAllocator {
    pub fn new() -> Self {
        Self {
            fast: StackFrameAllocator::new(),
            slow: BTreeMap::new(),
            fast_alloc_count: 0,
            slow_alloc_count: [0; CXL_CARD_COUNT],
            fast_dealloc_count: 0,
            slow_dealloc_count: [0; CXL_CARD_COUNT],
        }
    }
    /// Fast tier must be valid.
    pub fn init(&mut self, l: PhysPageNum, r: PhysPageNum) {
        self.fast.init(l, r);
    }
    pub fn add_slow(&mut self, card_id: CxlCardId, l: PhysPageNum, r: PhysPageNum) {
        let mut slow = StackFrameAllocator::new();
        slow.init(l, r);
        self.slow.insert(card_id, slow);
        /* now demotion to this slow tier is allowed */

        /* Now migrate certain data in the hash ring to this cxl card node */
    }
    /// Return true for success.
    /// False for reject: possibly for insufficient space for page / hashring data migration detected.
    pub fn try_eject_slow(&mut self, card_id: CxlCardId) -> bool {
        /* promote all pages to fast tier */

        /* migrate data to other cxl card nodes */

        /* migration complete, remove this card */
        self.slow.remove(&card_id);
        true
    }
    pub fn alloc_fast(&mut self) -> Option<PhysPageNum> {
        self.fast_alloc_count += 1;
        self.fast.alloc()
    }
    pub fn alloc_slow(&mut self, card_id: CxlCardId) -> Option<PhysPageNum> {
        self.slow_alloc_count[card_id.0 as usize] += 1;
        self.slow.get_mut(&card_id).and_then(|slow| slow.alloc())
    }
    /// Alloc from fast first, then fallback to slow.
    pub fn alloc(&mut self) -> Option<PhysPageNum> {
        if let Some(ppn) = self.alloc_fast() {
            return Some(ppn);
        }
        let card_ids = self.slow.keys().copied().collect::<Vec<_>>();
        for card_id in card_ids {
            if let Some(ppn) = self.alloc_slow(card_id) {
                return Some(ppn);
            }
        }
        None
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
        if self.fast.stack_range().contains(&ppn) {
            self.fast_dealloc_count += 1;
            self.fast.dealloc(ppn);
            return;
        } 
        for (card_id, slow) in self.slow.iter_mut() {
            if slow.stack_range().contains(&ppn) {
                self.slow_dealloc_count[card_id.0 as usize] += 1;
                slow.dealloc(ppn);
                return;
            }
        }
        panic!("Frame ppn={:#x} is not in fast or slow tier", ppn.0);
    }
    /// return None if this is a kernal page
    pub fn page_tier(&self, ppn: PhysPageNum) -> Option<MemoryTier> {
        if self.fast.stack_range().contains(&ppn) {
            return Some(MemoryTier::Fast);
        }
        for (card_id, slow) in self.slow.iter() {
            if slow.stack_range().contains(&ppn) {
                return Some(MemoryTier::Slow(*card_id));
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
    FRAME_ALLOCATOR.exclusive_access().init(
        PhysAddr::from(linker_symbol_addr!(ekernel)).ceil(),
        PhysAddr::from(DRAM_MEMORY_END).floor(),
    );
    /*
    let cxl_devices: Vec<PciDevice> = pci_scan().into_iter().filter(|d| is_cxl_type3(d)).collect();
    if !cxl_devices.is_empty() {
        for dev in cxl_devices {
            let bar = dev.bars[0];
            if let Some((base, size)) = bar {
                println_cxl!("CXL Type3 at {:#x}-{:#x}", base, base + size);
                FRAME_ALLOCATOR.exclusive_access().add_slow(
                  CxlCardId(dev.device_id as usize),
                  PhysAddr::from(base as usize).ceil(),
                  PhysAddr::from((base + size) as usize).floor()
                );
            }
        }
    } else {
      for (i, (start, end)) in CXL_MEMORY_RANGES.iter().enumerate() {
        FRAME_ALLOCATOR.exclusive_access().add_slow(
          CxlCardId(i),
          PhysAddr::from(*start as usize).ceil(),
          PhysAddr::from(*end as usize).floor()
        );
      }
    } 
    */
}

#[allow(unused)]
pub fn add_slow_frame_allocator(card_id: CxlCardId, l: PhysPageNum, r: PhysPageNum) {
    FRAME_ALLOCATOR.exclusive_access().add_slow(card_id, l, r);
}

pub fn frame_alloc_fast() -> Option<FrameTracker> {
    FRAME_ALLOCATOR
        .exclusive_access()
        .alloc_fast()
        .map(FrameTracker::new)
}

pub fn frame_alloc() -> Option<FrameTracker> {
    FRAME_ALLOCATOR
        .exclusive_access()
        .alloc()
        .map(FrameTracker::new)
}

pub fn frame_alloc_slow(card_id: CxlCardId) -> Option<FrameTracker> {
    let ppn = FRAME_ALLOCATOR.exclusive_access().alloc_slow(card_id)?;
    CXL_CARD_MANAGER.exclusive_access().track_card_ppn(ppn, card_id);
    Some(FrameTracker::new(ppn))
}

/// route on hash ring via vpn
pub fn frame_alloc_slow_route(vpn: VirtPageNum) -> Option<FrameTracker> {
    let card_id = {
        let mgr = CXL_CARD_MANAGER.exclusive_access();
        mgr.hash_ring.route(vpn.0 as u64).copied()?
    };
    frame_alloc_slow(card_id)
}

pub fn frame_alloc_more(num: usize) -> Option<Vec<FrameTracker>> {
    FRAME_ALLOCATOR
        .exclusive_access()
        .alloc_more(num)
        .map(|x| x.iter().map(|&t| FrameTracker::new(t)).collect())
}

pub fn frame_dealloc(ppn: PhysPageNum) {
    {
        let mut mgr = CXL_CARD_MANAGER.exclusive_access();
        if mgr.get_ppn_card(ppn).is_some() {
            mgr.untrack_page(ppn);
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
    let slow = FRAME_ALLOCATOR.exclusive_access().alloc_slow(CxlCardId(0)).unwrap();
    let tier = FRAME_ALLOCATOR.exclusive_access().page_tier(slow);
    println!("[CXL] slow ppn={:#x}, tier={:?}", slow.0, tier);

    let fast = FRAME_ALLOCATOR.exclusive_access().alloc_fast().unwrap();
    let tier = FRAME_ALLOCATOR.exclusive_access().page_tier(fast);
    println!("[CXL] fast ppn={:#x}, tier={:?}", fast.0, tier);
}