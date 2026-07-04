#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::vec::Vec;
use user_lib::{exit, thread_create, waittid, cxl_mmap, cxl_munmap};

struct Arg {
    pub pages: usize,
    pub pattern: u8,
}

fn cxl_worker(arg: *const Arg) -> ! {
    let arg = unsafe { &*arg };
    let size = arg.pages * 4096;
    let ptr = cxl_mmap(size) as *mut u8;
    if ptr.is_null() {
        exit(-1);
    }
    unsafe {
        for i in 0..size {
            ptr.add(i).write(arg.pattern);
        }
        for i in 0..size {
            if ptr.add(i).read() != arg.pattern {
                println!("  mismatch at offset {}", i);
                exit(-2);
            }
        }
    }
    cxl_munmap(ptr as usize, size);
    exit(0)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let mut tids = Vec::new();
    let args = [
        Arg { pages: 4, pattern: 0xaa },
        Arg { pages: 4, pattern: 0xbb },
        Arg { pages: 4, pattern: 0xcc },
    ];
    for arg in args.iter() {
        tids.push(thread_create(
            cxl_worker as usize,
            arg as *const _ as usize,
        ));
    }
    for tid in tids.iter() {
        let code = waittid(*tid as usize);
        println!("thread {} exited with {}", tid, code);
    }
    println!("threads_arg_cxl passed.");
    0
}
