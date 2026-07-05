#![no_std]
#![no_main]

macro_rules! linker_symbol_addr {
    ($symbol:path) => {
        ($symbol as *const ()).addr()
    };
}

//use crate::drivers::{GPU_DEVICE, KEYBOARD_DEVICE, MOUSE_DEVICE, INPUT_CONDVAR};
use crate::drivers::{GPU_DEVICE, KEYBOARD_DEVICE, MOUSE_DEVICE};
extern crate alloc;

#[macro_use]
extern crate bitflags;

use log::*;

#[path = "boards/qemu.rs"]
mod board;

#[macro_use]
mod console;
mod channel;
mod config;
mod cxl;
mod drivers;
mod fs;
mod lang_items;
mod logging;
mod mm;
mod net;
mod sbi;
mod sync;
mod syscall;
mod task;
mod timer;
mod trap;

use crate::drivers::chardev::CharDevice;
use crate::drivers::chardev::UART;

core::arch::global_asm!(include_str!("entry.asm"));

fn clear_bss() {
    unsafe extern "C" {
        safe fn sbss();
        safe fn ebss();
    }
    unsafe {
        core::slice::from_raw_parts_mut(
            linker_symbol_addr!(sbss) as *mut u8,
            linker_symbol_addr!(ebss) - linker_symbol_addr!(sbss),
        )
        .fill(0);
    }
}

use lazy_static::*;
use sync::UPIntrFreeCell;

lazy_static! {
    pub static ref DEV_NON_BLOCKING_ACCESS: UPIntrFreeCell<bool> =
        unsafe { UPIntrFreeCell::new(false) };
}

#[unsafe(no_mangle)]
pub fn rust_main() -> ! {
    clear_bss();
    logging::init();

    // Instance ID from build-time env var RINGS_INSTANCE_ID.
    let my_id = crate::cxl::instance_id::INSTANCE_ID;
    crate::cxl::allocator::set_instance_id(my_id);
    if my_id > 0 {
        println!("[CXL] instance {} (partial mode)", my_id);
    }

    mm::init();
    UART.init();
    info!("KERN: init gpu");
    let _gpu = GPU_DEVICE.clone();
    info!("KERN: init keyboard");
    let _keyboard = KEYBOARD_DEVICE.clone();
    info!("KERN: init mouse");
    let _mouse = MOUSE_DEVICE.clone();
    info!("KERN: init trap");
    trap::init();
    trap::enable_timer_interrupt();
    timer::set_next_trigger();
    board::device_init();
    use drivers::bus::pci::{pci_scan, is_cxl_type3, is_ivshmem};
    let pci_devices = pci_scan();
    let pci_count = pci_devices.len();
    let cxl_count = pci_devices.iter().filter(|d| is_cxl_type3(d)).count();
    let iv_count = pci_devices.iter().filter(|d| is_ivshmem(d)).count();
    println!("PCI: {} device(s) found, {} CXL Type 3, {} ivshmem", pci_count, cxl_count, iv_count);

    // print ivshmem BARs for debug
    for dev in &pci_devices {
        if is_ivshmem(dev) {
            for (i, bar) in dev.bars.iter().enumerate() {
                if let Some((base, size)) = bar {
                    println!("  ivshmem BAR[{}]: base={:#x} size={:#x}", i, base, size);
                }
            }
        }
    }
    fs::list_apps();
    task::add_initproc();
    *DEV_NON_BLOCKING_ACCESS.exclusive_access() = true;
    task::run_tasks();
    panic!("Unreachable in rust_main!");
}
