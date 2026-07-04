use super::frame_allocator::{FRAME_ALLOCATOR, MemoryTier};
use super::page_table::{PageTable, PageTableEntry};
use super::{PhysPageNum, VirtPageNum, frame_dealloc, copy_page};
use crate::task::current_process;
use crate::timer::get_time_ms;
use alloc::collections::BTreeMap;
use core::arch::asm;
use lazy_static::*;
use crate::sync::UPIntrFreeCell;
use crate::task::for_each_process;
use crate::println_cxl;

pub struct PageMigrator {
    cold_count: BTreeMap<PhysPageNum, u8>,
    scan_interval_ms: usize,
    cold_threshold: u8,
    last_scan_ms: usize,

    /// Statistics
    pub promote_count: u64,
    pub demote_count: u64,
}

impl PageMigrator {
    pub fn new() -> Self {
        Self {
            cold_count: BTreeMap::new(),
            scan_interval_ms: 500,
            cold_threshold: 3,
            last_scan_ms: 0,
            promote_count: 0,
            demote_count: 0,
        }
    }
    pub fn tick(&mut self) {
        let now = get_time_ms();
        if now - self.last_scan_ms < self.scan_interval_ms {
            return;
        }
        self.last_scan_ms = now;
        let token = current_process().inner_exclusive_access().memory_set.token();
        let page_table = PageTable::from_token(token);
        let ptes = page_table.collect_ptes();
        for (vpn, pte) in ptes {
            let ppn = pte.ppn();
            let tier = FRAME_ALLOCATOR.exclusive_access().page_tier(ppn);
            match tier {
                Some(MemoryTier::Fast) => {
                    if !pte.accessed() {
                        let count = self.cold_count.entry(ppn).or_insert(0);
                        *count += 1;
                        if *count >= self.cold_threshold {
                            self.demote_page(ppn, vpn, &page_table, token);
                            self.cold_count.remove(&ppn);
                            break;
                        }
                    } else {
                        self.cold_count.remove(&ppn);
                        pte.clear_accessed();
                    }
                }
                Some(MemoryTier::Slow(card_idx)) => {
                    if pte.accessed() {
                        self.promote_page(ppn, vpn, &page_table, token);
                        self.cold_count.remove(&ppn);
                    } else {
                        let count = self.cold_count.entry(ppn).or_insert(0);
                        *count += 1;
                    }
                }
                None => { /* kernel page, trampoline shall handle it */ }
            }
        }
    }
    /// slow -> fast
    fn promote_page(
        &mut self,
        old_ppn: PhysPageNum,
        _vpn: VirtPageNum,
        _page_table: &PageTable,
        _token: usize,
    ) {
        self.promote_count += 1;
        let new_ppn = FRAME_ALLOCATOR.exclusive_access().alloc_fast().unwrap();
        copy_page(old_ppn, new_ppn);
        self.replace_ppn(old_ppn, new_ppn);
        unsafe {
            asm!("sfence.vma");
        }
        // println_cxl!("promote_page: old_ppn={:#x}, new_ppn={:#x}", old_ppn.0, new_ppn.0);
        frame_dealloc(old_ppn);
        if let Some(count) = self.cold_count.remove(&old_ppn) {
            self.cold_count.insert(new_ppn, count);
        }
    }
    /// fast -> slow
    fn demote_page(
        &mut self,
        old_ppn: PhysPageNum,
        _vpn: VirtPageNum,
        _page_table: &PageTable,
        _token: usize,
    ) {
        self.demote_count += 1;
        let new_ppn = FRAME_ALLOCATOR.exclusive_access().alloc_slow_any().unwrap();
        copy_page(old_ppn, new_ppn);
        self.replace_ppn(old_ppn, new_ppn);
        unsafe {
            asm!("sfence.vma");
        }
        // println_cxl!("demote_page: old_ppn={:#x}, new_ppn={:#x}", old_ppn.0, new_ppn.0);
        frame_dealloc(old_ppn);
        if let Some(count) = self.cold_count.remove(&old_ppn) {
            self.cold_count.insert(new_ppn, count);
        }
    }
    fn replace_ppn(&mut self, old_ppn: PhysPageNum, new_ppn: PhysPageNum) {
      for_each_process(|proc| {
        let mut proc_inner = proc.inner_exclusive_access();
        let token = proc_inner.memory_set.token();
        let page_table = PageTable::from_token(token);
        page_table.collect_ptes().iter_mut()
            .filter(|(_, pte)| pte.ppn() == old_ppn)
            .for_each(|(vpn, pte)| {
              **pte = PageTableEntry::new(new_ppn, pte.flags());
              proc_inner.memory_set.forget_frame(*vpn);
            });
      });
    }
}

lazy_static! {
    pub static ref PAGE_MIGRATOR: UPIntrFreeCell<PageMigrator> =
        unsafe { UPIntrFreeCell::new(PageMigrator::new()) };
}