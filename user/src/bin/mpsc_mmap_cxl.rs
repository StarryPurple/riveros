// mem mapped CXL pages might be migrated to fast memory during operation
#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;
extern crate core;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
  panic!("Not implemented");
}