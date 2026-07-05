#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::boxed::Box;
use user_lib::{
    LockFreeRing, sys_ring_create, sys_ring_destroy,
    get_time, thread_create, waittid, exit, pipe, read, write,
};

const ITERS: usize = 2000;
const MSG_SIZE: usize = 64;
const RING_CAP: usize = 4096;

struct BenchArg {
    ring: *const LockFreeRing,
    fd: usize,
}

fn writer_spin(arg: usize) -> ! {
    let ring = unsafe { &*(arg as *const LockFreeRing) };
    let data = [0xabu8; MSG_SIZE];
    for _ in 0..ITERS { ring.push_spin(&data); }
    exit(0)
}
fn reader_spin(arg: usize) -> ! {
    let ring = unsafe { &*(arg as *const LockFreeRing) };
    let mut buf = [0u8; MSG_SIZE];
    for _ in 0..ITERS { ring.pop_spin(&mut buf); }
    exit(0)
}

fn writer_yield(arg: usize) -> ! {
    let ring = unsafe { &*(arg as *const LockFreeRing) };
    let data = [0xabu8; MSG_SIZE];
    for _ in 0..ITERS { ring.push_yield(&data); }
    exit(0)
}
fn reader_yield(arg: usize) -> ! {
    let ring = unsafe { &*(arg as *const LockFreeRing) };
    let mut buf = [0u8; MSG_SIZE];
    for _ in 0..ITERS { ring.pop_yield(&mut buf); }
    exit(0)
}

fn writer_hybrid(arg: usize) -> ! {
    let a = unsafe { &*(arg as *const BenchArg) };
    let ring = unsafe { &*a.ring };
    let data = [0xabu8; MSG_SIZE];
    for _ in 0..ITERS { ring.push_hybrid(&data, 10, a.fd); }
    exit(0)
}
fn reader_hybrid(arg: usize) -> ! {
    let a = unsafe { &*(arg as *const BenchArg) };
    let ring = unsafe { &*a.ring };
    let mut buf = [0u8; MSG_SIZE];
    for _ in 0..ITERS { ring.pop_hybrid(&mut buf, 10, a.fd); }
    exit(0)
}

fn pipe_writer(fd: usize) -> ! {
    let data = [0xabu8; MSG_SIZE];
    for _ in 0..ITERS { write(fd, &data); }
    exit(0)
}
fn pipe_reader(fd: usize) -> ! {
    let mut buf = [0u8; MSG_SIZE];
    for _ in 0..ITERS { read(fd, &mut buf); }
    exit(0)
}

fn run_bench(writer_fn: usize, reader_fn: usize, arg: usize) -> usize {
    let start = get_time();
    let w = thread_create(writer_fn, arg);
    let r = thread_create(reader_fn, arg);
    waittid(w as usize);
    waittid(r as usize);
    (get_time() - start) as usize
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Ring Buffer Benchmark (single-core QEMU) ===");
    println!("Iterations: {}, msg: {} B", ITERS, MSG_SIZE);

    // ── 1. LockFreeRing (yield) ──
    let mut vaddr: usize = 0;
    let fd = sys_ring_create(RING_CAP, &mut vaddr as *mut usize);
    assert!(fd >= 0);
    let ring = LockFreeRing::new(vaddr as *mut u8);
    let t_yield = run_bench(
        linker_symbol_addr!(writer_yield),
        linker_symbol_addr!(reader_yield),
        &ring as *const _ as usize);
    sys_ring_destroy(fd as usize);

    // ── 2. LockFreeRing (hybrid, spin=10) ──
    let mut vaddr2: usize = 0;
    let fd2 = sys_ring_create(RING_CAP, &mut vaddr2 as *mut usize);
    assert!(fd2 >= 0);
    let ring2 = LockFreeRing::new(vaddr2 as *mut u8);
    let arg2 = Box::into_raw(Box::new(BenchArg { ring: &ring2, fd: fd2 as usize }));
    let t_hybrid = run_bench(
        linker_symbol_addr!(writer_hybrid),
        linker_symbol_addr!(reader_hybrid),
        arg2 as usize);
    unsafe { drop(Box::from_raw(arg2)); }
    sys_ring_destroy(fd2 as usize);

    // ── 3. LockFreeRing (busy-poll) ──
    let mut vaddr3: usize = 0;
    let fd3 = sys_ring_create(RING_CAP, &mut vaddr3 as *mut usize);
    assert!(fd3 >= 0);
    let ring3 = LockFreeRing::new(vaddr3 as *mut u8);
    let t_spin = run_bench(
        linker_symbol_addr!(writer_spin),
        linker_symbol_addr!(reader_spin),
        &ring3 as *const _ as usize);
    sys_ring_destroy(fd3 as usize);

    // ── 4. Pipe ──
    let mut fds = [0usize; 2];
    pipe(&mut fds);
    let start = get_time();
    let pw = thread_create(linker_symbol_addr!(pipe_writer), fds[1]);
    let pr = thread_create(linker_symbol_addr!(pipe_reader), fds[0]);
    waittid(pw as usize);
    waittid(pr as usize);
    let t_pipe = (get_time() - start) as usize;

    // ── Results ──
    let total = (ITERS * MSG_SIZE) as u64;
    let total = total; // prevent unused mut
    macro_rules! bps {
        ($ms:expr) => { if $ms > 0 { total * 1000 / $ms as u64 } else { 0 } };
    }
    println!("─────────────────────────────────────────────");
    println!("Method                 ms         B/s");
    println!("─────────────────────────────────────────────");
    println!("LockFreeRing (yield)  {:>5}   {}", t_yield,  bps!(t_yield));
    println!("LockFreeRing (hybrid) {:>5}   {}", t_hybrid, bps!(t_hybrid));
    println!("LockFreeRing (busy)   {:>5}   {}", t_spin,   bps!(t_spin));
    println!("Pipe                  {:>5}   {}", t_pipe,   bps!(t_pipe));
    println!("─────────────────────────────────────────────");
    println!("Note: single-core QEMU — busy-poll wastes time slices.");
    println!("LockFreeRing(yield/hybrid) use zero-syscall data path.");
    println!("Done.");
    0
}
