pub mod cross;
pub mod layout;
pub mod syscall;

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use lazy_static::*;

use crate::config::PAGE_SIZE;
use crate::mm::{MapArea, MapPermission, MapType, PhysPageNum, VirtAddr, frame_alloc};
use crate::sync::UPIntrFreeCell;
use crate::task::{TaskControlBlock, block_current_and_run_next, current_task, wakeup_task};

use layout::RingHeader;

const HEADER_BYTES: usize = 24;

pub struct Channel {
    pub id: usize,
    pub capacity: usize,
    pub user_vaddr: VirtAddr,
    pub page_count: usize,
    pub wait_queue: VecDeque<Arc<TaskControlBlock>>,
}

lazy_static! {
    pub static ref CHANNEL_TABLE:
        UPIntrFreeCell<Vec<Option<Arc<UPIntrFreeCell<Channel>>>>> =
        unsafe { UPIntrFreeCell::new(Vec::new()) };
}

pub fn ring_create(capacity: usize) -> (isize, usize) {
    let total_bytes = HEADER_BYTES + capacity;
    let page_count = (total_bytes + PAGE_SIZE - 1) / PAGE_SIZE;

    let process = crate::task::current_process();
    let mut inner = process.inner_exclusive_access();

    let start_va = inner
        .memory_set
        .find_mmap_base(page_count)
        .expect("ring_create: no VA space");
    let end_va: VirtAddr = (start_va.0 + page_count * PAGE_SIZE).into();

    // MapArea with Framed — allocates and maps pages automatically
    let area = MapArea::new(
        start_va,
        end_va,
        MapType::Framed,
        MapPermission::R | MapPermission::W | MapPermission::U,
    );
    inner.memory_set.push(area, None);

    // Retrieve the PPN of the first mapped page via page table
    let pte = inner.memory_set.translate(start_va.floor()).unwrap();
    let ppn = pte.ppn();
    drop(inner);

    // Initialize ring header
    let pa = ppn.0 << 12;
    let header = unsafe { &*(pa as *const RingHeader) };
    header.init(capacity as u32);

    // Register in global table
    let mut table = CHANNEL_TABLE.exclusive_access();
    let id = table
        .iter()
        .position(|x| x.is_none())
        .unwrap_or_else(|| {
            let id = table.len();
            table.push(None);
            id
        });
    let channel = Arc::new(unsafe {
        UPIntrFreeCell::new(Channel {
            id,
            capacity,
            user_vaddr: start_va,
            page_count,
            wait_queue: VecDeque::new(),
        })
    });
    table[id] = Some(channel);
    drop(table);

    (id as isize, start_va.0)
}

pub fn ring_mmap(id: usize) -> isize {
    let table = CHANNEL_TABLE.exclusive_access();
    match table.get(id).and_then(|x| x.as_ref()) {
        Some(ch) => ch.exclusive_access().user_vaddr.0 as isize,
        None => -1,
    }
}

pub fn ring_destroy(id: usize) -> isize {
    let mut table = CHANNEL_TABLE.exclusive_access();
    if id >= table.len() || table[id].is_none() {
        return -1;
    }
    let channel = table[id].take().unwrap();
    drop(table);

    let ch = channel.exclusive_access();
    let process = crate::task::current_process();
    let mut inner = process.inner_exclusive_access();
    // The MapArea was created with start_va as VirtAddr.
    // remove_area_with_start_vpn takes VirtPageNum.
    inner
        .memory_set
        .remove_area_with_start_vpn(ch.user_vaddr.floor());
    drop(inner);
    0
}

pub fn ring_wait(id: usize) -> isize {
    let table = CHANNEL_TABLE.exclusive_access();
    let channel = match table.get(id).and_then(|x| x.as_ref()) {
        Some(ch) => ch.clone(),
        None => return -1,
    };
    drop(table);

    let mut ch = channel.exclusive_access();
    // Wake up check — if shutdown flag was set, don't block
    ch.wait_queue.push_back(current_task().unwrap());
    drop(ch);
    block_current_and_run_next();
    0
}

pub fn ring_notify(id: usize) -> isize {
    let table = CHANNEL_TABLE.exclusive_access();
    let channel = match table.get(id).and_then(|x| x.as_ref()) {
        Some(ch) => ch.clone(),
        None => return -1,
    };
    drop(table);

    let mut ch = channel.exclusive_access();
    while let Some(task) = ch.wait_queue.pop_front() {
        wakeup_task(task);
    }
    0
}
