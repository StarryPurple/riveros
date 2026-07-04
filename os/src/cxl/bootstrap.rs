//! ivshmem discovery and instance-ID assignment.

use super::layout::*;
use super::allocator;

/// Initialise ivshmem header OR claim an instance ID.
pub fn shm_init() -> (usize, bool) {
    let magic = unsafe { shm_read64(OFF_MAGIC) };

    if magic != SHM_MAGIC {
        unsafe {
            shm_write64(OFF_MAGIC, SHM_MAGIC);
            shm_write32(OFF_N_INSTANCES, 0);
            shm_write32(OFF_DATA_START, DATA_START as u32);
            shm_write32(OFF_TOTAL_PAGES, DATA_PAGE_COUNT as u32);
            allocator::shm_init_freelist();
        }
        let my_id = unsafe { claim_slot() };
        println!("[CXL] shm first-boot -- instance {}", my_id);
        (my_id, true)
    } else {
        let n_inst = unsafe { shm_read32(OFF_N_INSTANCES) };
        if n_inst as usize >= MAX_INSTANCES {
            // all slots full — wrap around (single-node dev reset)
            unsafe {
                shm_write32(OFF_N_INSTANCES, 0);
                for i in 0..MAX_INSTANCES {
                    reset_bakery(i);
                }
            }
        }
        let my_id = unsafe { claim_slot() };
        println!("[CXL] shm joined -- instance {}", my_id);
        (my_id, false)
    }
}

unsafe fn claim_slot() -> usize {
    // scan and clear stale bakery state from crashed previous instances
    let n_inst = unsafe { shm_read32(OFF_N_INSTANCES) };
    for i in 0..n_inst as usize {
        let n = unsafe { shm_read64(OFF_NUMBER + i * 8) };
        if n != 0 {
            unsafe {
                shm_write64(OFF_NUMBER + i * 8, 0);
                let p = (SHM_BASE + OFF_CHOOSING + i) as *mut u8;
                p.write_volatile(0u8);
            }
        }
    }
    let my_id = n_inst as usize;
    unsafe { shm_write32(OFF_N_INSTANCES, n_inst + 1); }
    unsafe { reset_bakery(my_id); }
    allocator::set_instance_id(my_id);
    my_id
}

unsafe fn reset_bakery(me: usize) {
    shm_write64(OFF_NUMBER + me * 8, 0);
    let p = (SHM_BASE + OFF_CHOOSING + me) as *mut u8;
    p.write_volatile(0u8);
}
