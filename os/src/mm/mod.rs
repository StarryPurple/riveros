mod address;
mod frame_allocator;
mod heap_allocator;
mod memory_set;
mod page_migrator;
mod page_table;

pub use address::VPNRange;
pub use address::{PhysAddr, PhysPageNum, StepByOne, VirtAddr, VirtPageNum};
pub use frame_allocator::{
    FRAME_ALLOCATOR, FrameTracker, MemoryTier, frame_alloc, frame_alloc_more, frame_alloc_slow,
    frame_dealloc,
};
pub use memory_set::{KERNEL_SPACE, MapArea, MapPermission, MapType, MemorySet, kernel_token};
pub use page_migrator::PAGE_MIGRATOR;
use page_table::PTEFlags;
pub use page_table::{
    PageTable, PageTableEntry, UserBuffer, translated_byte_buffer, translated_ref,
    translated_refmut, translated_str,
};

pub fn init() {
    heap_allocator::init_heap();
    frame_allocator::init_frame_allocator();
    KERNEL_SPACE.exclusive_access().activate();
    // frame_allocator::tier_alloc_test();
}

pub fn cxl_delay() {
    let expire = riscv::register::time::read() + crate::board::CLOCK_FREQ / 1_000_000; // 1us. Using this clock frequency is fine
    while riscv::register::time::read() < expire {
        riscv::asm::wfi();
    }
}

pub fn copy_page(src: PhysPageNum, dst: PhysPageNum) {
    dst.get_bytes_array().copy_from_slice(src.get_bytes_array());

    let alloc = FRAME_ALLOCATOR.exclusive_access();
    let is_slow = alloc
        .page_tier(src)
        .map_or(false, |t| matches!(t, MemoryTier::Slow(card_idx)))
        || alloc
            .page_tier(dst)
            .map_or(false, |t| matches!(t, MemoryTier::Slow(card_idx)));
    drop(alloc);

    if is_slow {
        // println_cxl!("cxl_delay before copy_page");
        cxl_delay();
        // println_cxl!("cxl_delay after copy_page");
    }
}
