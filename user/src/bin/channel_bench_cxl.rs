#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;
extern crate core;

use user_lib::{cxl_mmap, cxl_munmap, pipe, read, thread_create, waittid, write, get_time, Channel, exit, connect, listen, accept};

const ITERS: usize = 1000;
const MSG_SIZE: usize = 64;
const BUF_SIZE: usize = 3 * MSG_SIZE;

fn channel_writer(buf: usize) -> ! {
    let ch = unsafe { &*(buf as *const Channel) };
    for _ in 0..ITERS {
        let data = [0xabu8; MSG_SIZE];
        ch.send(&data);
    }
    exit(0)
}

fn channel_reader(buf: usize) -> ! {
    let ch = unsafe { &*(buf as *const Channel) };
    let mut rbuf = [0u8; MSG_SIZE];
    for _ in 0..ITERS {
        ch.recv(&mut rbuf);
    }
    exit(0)
}

fn tcp_server(port: usize) -> ! {
    let listen_fd = listen(port as u16);
    if listen_fd < 0 { exit(1); }
    let fd = accept(listen_fd as usize);
    if fd < 0 { exit(1); }
    let mut buf = [0u8; MSG_SIZE];
    for _ in 0..ITERS { read(fd as usize, &mut buf); }
    exit(0)
}

fn tcp_client(port: usize) -> ! {
    let localhost: u32 = 0x7f00_0001; // 127.0.0.1
    let fd = connect(localhost, 0, port as u16);
    let data = [0xabu8; MSG_SIZE];
    for _ in 0..ITERS { write(fd as usize, &data); }
    exit(0)
}

fn pipe_writer(fd: usize) -> ! {
    let data = [0xabu8; MSG_SIZE];
    for _ in 0..ITERS {
        write(fd, &data);
    }
    exit(0)
}

fn pipe_reader(fd: usize) -> ! {
    let mut buf = [0u8; MSG_SIZE];
    for _ in 0..ITERS {
        read(fd, &mut buf);
    }
    exit(0)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== CXL Channel vs Pipe Benchmark ===");
    println!("Iterations: {}, message size: {} bytes", ITERS, MSG_SIZE);

    // ---- Channel benchmark (shared CXL memory) ----
    let cxl_buf = cxl_mmap(BUF_SIZE) as usize;
    let ch = Channel::new(cxl_buf as *mut u8, BUF_SIZE);
    let start = get_time();
    let w = thread_create(linker_symbol_addr!(channel_writer), &ch as *const _ as usize);
    let r = thread_create(linker_symbol_addr!(channel_reader), &ch as *const _ as usize);
    waittid(w as usize);
    waittid(r as usize);
    let ch_elapsed = get_time() - start;
    println!("Channel: {} ms (zero syscall data path)", ch_elapsed);

    // ---- Pipe benchmark ----
    let mut fds = [0usize; 2];
    pipe(&mut fds);
    let start = get_time();
    let w = thread_create(linker_symbol_addr!(pipe_writer), fds[1]);
    let r = thread_create(linker_symbol_addr!(pipe_reader), fds[0]);
    waittid(w as usize);
    waittid(r as usize);
    let pipe_elapsed = get_time() - start;
    println!("Pipe:   {} ms (2 syscalls per message)", pipe_elapsed);

    /*
    // ---- TCP loopback benchmark ----
    const TCP_PORT: u16 = 3333;
    let start = get_time();
    let srv = thread_create(linker_symbol_addr!(tcp_server), TCP_PORT as usize);
    let cli = thread_create(linker_symbol_addr!(tcp_client), TCP_PORT as usize);
    let srv_exit = waittid(srv as usize);
    let cli_exit = waittid(cli as usize);
    let tcp_elapsed = if srv_exit >= 0 && cli_exit >= 0 {
        let t = get_time() - start;
        println!("TCP:    {} ms (4+ syscalls, kernel TCP stack)", t);
        t
    } else {
        println!("TCP:    N/A (loopback not available)");
        9999
    };
    */
    println!("================================");
    println!("Channel: {} ms | Pipe: {} ms", ch_elapsed, pipe_elapsed);
    if ch_elapsed > pipe_elapsed {
        println!("Note: CXL is software-simulated in QEMU (no real latency overhead).");
        println!("On real CXL hardware, the channel avoids syscall cost entirely.");
    }
    println!("Done.");
    cxl_munmap(cxl_buf, BUF_SIZE);
    0
}
