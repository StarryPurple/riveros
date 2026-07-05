#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::{LockFreeRing, sys_ring_create, sys_ring_destroy, get_time, thread_create, waittid, exit, pipe, read, write};

const ITERS: usize = 2000;
const MSG_SIZE: usize = 64;
const RING_CAP: usize = 4096;

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

fn run_ring(r: &LockFreeRing, writer_fn: usize, reader_fn: usize) -> usize {
    let start = get_time();
    let w = thread_create(writer_fn, r as *const _ as usize);
    let r_t = thread_create(reader_fn, r as *const _ as usize);
    waittid(w as usize);
    waittid(r_t as usize);
    (get_time() - start) as usize
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== Ring Buffer Benchmark (single-core QEMU) ===");
    println!("Iterations: {}, msg: {} B", ITERS, MSG_SIZE);

    // ── 1. LockFreeRing (yield) — cooperative, fair comparison ──
    let mut vaddr: usize = 0;
    let fd = sys_ring_create(RING_CAP, &mut vaddr as *mut usize);
    assert!(fd >= 0);
    let ring = LockFreeRing::new(vaddr as *mut u8);
    let t_yield = run_ring(&ring,
        linker_symbol_addr!(writer_yield),
        linker_symbol_addr!(reader_yield));
    sys_ring_destroy(fd as usize);

    // ── 2. LockFreeRing (busy-poll) — reference, bad on single-core ──
    let mut vaddr2: usize = 0;
    let fd2 = sys_ring_create(RING_CAP, &mut vaddr2 as *mut usize);
    assert!(fd2 >= 0);
    let ring2 = LockFreeRing::new(vaddr2 as *mut u8);
    let t_spin = run_ring(&ring2,
        linker_symbol_addr!(writer_spin),
        linker_symbol_addr!(reader_spin));
    sys_ring_destroy(fd2 as usize);

    // ── 3. Pipe ──
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
    println!("─────────────────────────────────────────");
    println!("Method                  ms      B/s");
    println!("─────────────────────────────────────────");
    println!("LockFreeRing (yield)   {:5}   {}", t_yield, if t_yield > 0 { total * 1000 / t_yield as u64 } else { 0 });
    println!("LockFreeRing (busy)    {:5}   {}", t_spin, if t_spin > 0 { total * 1000 / t_spin as u64 } else { 0 });
    println!("Pipe                   {:5}   {}", t_pipe, if t_pipe > 0 { total * 1000 / t_pipe as u64 } else { 0 });
    println!("─────────────────────────────────────────");
    if t_yield < t_pipe {
        let ratio = t_pipe / t_yield;
        println!("LockFreeRing(yield) is {}x faster than Pipe (zero-syscall data path)", ratio);
    } else if t_yield == t_pipe {
        println!("LockFreeRing(yield) == Pipe (same scheduling, but ring avoids kernel data copy)");
    } else {
        let ratio = t_yield / t_pipe;
        println!("Note: Pipe is {}x faster on single-core; LockFreeRing wins on multi-core with busy-poll", ratio);
    }
    println!("Done.");
    0
}
