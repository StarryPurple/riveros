#[allow(unused)]

pub const USER_STACK_SIZE: usize = 4096 * 2;
pub const KERNEL_STACK_SIZE: usize = 4096 * 2;
pub const KERNEL_HEAP_SIZE: usize = 0x100_0000;
pub const PAGE_SIZE: usize = 0x1000;
pub const PAGE_SIZE_BITS: usize = 0xc;

pub const TRAMPOLINE: usize = usize::MAX - PAGE_SIZE + 1;
pub const TRAP_CONTEXT_BASE: usize = TRAMPOLINE - PAGE_SIZE;
pub const CXL_RESERVED_MEMORY_START: usize = 0x100_000_000;

pub use crate::board::{CLOCK_FREQ, MEMORY_END, MMIO, CXL_MEMORY_RANGES, CXL_CARD_COUNT, DRAM_MEMORY_END};