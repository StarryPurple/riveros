#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicU64, Ordering};
use user_lib::{cxl_mmap, cxl_munmap, thread_create, waittid, exit, yield_, get_time};

const ROUNDS: usize = 5;
const WAL_ENTRIES: usize = 256;

struct WalEntry {
    seq: u64,
    op: u64,    // value to add
    checksum: u64,
}

struct Wal {
    counter: u64,
    head: u64,
    entries: [WalEntry; WAL_ENTRIES],
}

static WAL_VA: AtomicU64 = AtomicU64::new(0);

fn make_wal_entry(seq: u64, val: u64) -> WalEntry {
    WalEntry { seq, op: val, checksum: seq.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(val) }
}
fn checksum_ok(e: &WalEntry) -> bool {
    e.checksum == e.seq.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(e.op)
}

fn writer_worker(_id: usize) -> ! {
    let w = unsafe { &mut *(WAL_VA.load(Ordering::Relaxed) as *mut Wal) };
    for i in 0..ROUNDS {
        let val = (i * 100 + 13) as u64;
        let seq = w.head;

        let entry = make_wal_entry(seq, val);
        let idx = (seq % WAL_ENTRIES as u64) as usize;

        // Write log entry first (WAL principle)
        unsafe {
            core::ptr::write_volatile(&mut w.entries[idx], entry);
        }
        core::sync::atomic::fence(Ordering::SeqCst);

        // Then update counter
        w.counter = w.counter.wrapping_add(val);
        w.head = seq.wrapping_add(1);
        core::sync::atomic::fence(Ordering::SeqCst);

        yield_();
    }
    exit(0)
}

fn recovery_check() -> bool {
    let w = unsafe { &*(WAL_VA.load(Ordering::Relaxed) as *const Wal) };
    let counter_saved = w.counter;
    let head = w.head;

    // Replay WAL: compute what counter SHOULD be
    let mut replayed: u64 = 0;
    for idx in 0..head.min(WAL_ENTRIES as u64) {
        let i = (idx % WAL_ENTRIES as u64) as usize;
        let e = unsafe { core::ptr::read_volatile(&w.entries[i]) };
        if e.seq != idx { break; }         // sequence break → incomplete
        if !checksum_ok(&e) { break; }     // corrupt entry → stop
        replayed = replayed.wrapping_add(e.op);
    }
    replayed == counter_saved
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("=== CXL Write-Ahead Log ===");

    let sz = core::mem::size_of::<Wal>();
    let va = cxl_mmap(sz);
    assert!(va > 0, "cxl_mmap failed");
    WAL_VA.store(va as u64, Ordering::Release);
    core::sync::atomic::fence(Ordering::SeqCst);

    let w = unsafe { &mut *(va as *mut Wal) };
    w.counter = 0;
    w.head = 0;
    for i in 0..WAL_ENTRIES {
        w.entries[i] = WalEntry { seq: 0, op: 0, checksum: 0 };
    }
    core::sync::atomic::fence(Ordering::SeqCst);
    println!("  WAL at VA {:#x}, {} entries, {} rounds", va, WAL_ENTRIES, ROUNDS);

    // Run writer thread
    let t0 = get_time();
    let tid = thread_create(linker_symbol_addr!(writer_worker), 0);
    assert!(tid > 0, "create writer");
    waittid(tid as usize);
    let ms = get_time() - t0;
    println!("  writer done in {} ms, counter={}", ms, w.counter);

    // Run recovery check
    let ok = recovery_check();
    println!("  recovery check: {}", if ok { "PASS" } else { "FAIL" });

    // Simulate crash in the middle
    println!("--- Crash Simulation ---");
    w.counter = 0;
    w.head = 0;
    let tid2 = thread_create(linker_symbol_addr!(writer_worker), 0);
    assert!(tid2 > 0);
    waittid(tid2 as usize);

    // Insert a "partial" entry: write seq but NOT checksum → should be rejected
    let bad_idx = (w.head % WAL_ENTRIES as u64) as usize;
    let bad_seq = w.head;
    unsafe {
        core::ptr::write_volatile(&mut w.entries[bad_idx].seq, bad_seq);
        core::ptr::write_volatile(&mut w.entries[bad_idx].op, 999);
        core::ptr::write_volatile(&mut w.entries[bad_idx].checksum, 0xDEAD); // bad checksum
    }
    w.head = bad_seq.wrapping_add(1);
    core::sync::atomic::fence(Ordering::SeqCst);

    let ok2 = recovery_check();
    println!("  corrupt entry recovery: {}", if ok2 { "WARNING: bad checksum NOT detected" }
                                         else { "PASS: bad checksum rejected" });

    cxl_munmap(va as usize, sz);
    if ok && !ok2 { 0 } else { 1 }
}
