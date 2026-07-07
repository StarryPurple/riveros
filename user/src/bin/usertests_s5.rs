#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

// Single-instance test runner for s5-hosts branch.
// Multi-instance tests (shm_multi_*, shm_node*, thread_cxl_token_*, thread_cxl_phil_*,
// token_ring, calc_dist, mbox_3node, dist_sort_merge, thread_cxl_barrier,
// xfer_firehose, xfer_async_rpc, xfer_dataplane, ring_cross) require
// simultaneous Node0+Node1+Node2 → NOT auto-runnable. Run those manually.

static SUCC_TESTS: &[(&str, &str, &str, &str, i32)] = &[
    // SHM page-mgmt tests
    ("shm_test_rc\0", "\0", "\0", "\0", 0),
    ("shm_test_all\0", "\0", "\0", "\0", 0),
    ("shm_bench\0", "\0", "\0", "\0", 0),
    ("shm_test_gc\0", "\0", "\0", "\0", 0),
    ("shm_test_integ\0", "\0", "\0", "\0", 0),
    ("shm_test_cross\0", "\0", "\0", "\0", 0),

    // SHM stress tests
    ("shm_test_stress_concurrent\0", "\0", "\0", "\0", 0),
    ("shm_test_stress_gc\0", "\0", "\0", "\0", 0),
    ("shm_test_stress_refcount\0", "\0", "\0", "\0", 0),

    // CXL mmap tests
    ("cxl_info\0", "\0", "\0", "\0", 0),
    ("mmap_cxl\0", "\0", "\0", "\0", 0),
    ("peterson_cxl\0", "\0", "\0", "\0", 0),
    ("threads_arg_cxl\0", "\0", "\0", "\0", 0),

    // Ring / channel tests
    ("ring_basic\0", "\0", "\0", "\0", 0),
    ("ring_bench\0", "\0", "\0", "\0", 0),
    ("channel_bench_cxl\0", "\0", "\0", "\0", 0),

    // Multi-thread CXL business-logic tests
    ("thread_cxl_sum\0", "\0", "\0", "\0", 0),
    ("thread_cxl_maps\0", "\0", "\0", "\0", 0),
    ("thread_cxl_matmul\0", "\0", "\0", "\0", 0),
    ("thread_cxl_wal\0", "\0", "\0", "\0", 0),
];

static FAIL_TESTS: &[(&str, &str, &str, &str, i32)] = &[];

use user_lib::{exec, fork, waitpid};

fn run_tests(tests: &[(&str, &str, &str, &str, i32)]) -> i32 {
    let mut pass_num = 0;
    let mut arr: [*const u8; 4] = [
        core::ptr::null::<u8>(),
        core::ptr::null::<u8>(),
        core::ptr::null::<u8>(),
        core::ptr::null::<u8>(),
    ];

    for test in tests {
        println!("Usertests: Running {}", test.0);
        arr[0] = test.0.as_ptr();
        if test.1 != "\0" {
            arr[1] = test.1.as_ptr();
            arr[2] = core::ptr::null::<u8>();
            arr[3] = core::ptr::null::<u8>();
            if test.2 != "\0" {
                arr[2] = test.2.as_ptr();
                arr[3] = core::ptr::null::<u8>();
                if test.3 != "\0" {
                    arr[3] = test.3.as_ptr();
                } else {
                    arr[3] = core::ptr::null::<u8>();
                }
            } else {
                arr[2] = core::ptr::null::<u8>();
                arr[3] = core::ptr::null::<u8>();
            }
        } else {
            arr[1] = core::ptr::null::<u8>();
            arr[2] = core::ptr::null::<u8>();
            arr[3] = core::ptr::null::<u8>();
        }

        let pid = fork();
        if pid == 0 {
            exec(test.0, &arr[..]);
            panic!("unreachable!");
        } else {
            let mut exit_code: i32 = Default::default();
            let wait_pid = waitpid(pid as usize, &mut exit_code);
            assert_eq!(pid, wait_pid);
            if exit_code == test.4 {
                pass_num = pass_num + 1;
            }
            println!(
                "\x1b[32mUsertests: Test {} in Process {} exited with code {}\x1b[0m",
                test.0, pid, exit_code
            );
        }
    }
    pass_num
}

use user_lib::{CxlMemInfo, query_cxl_meminfo};
#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("========== s5-hosts Test Suite ==========");
    let succ_num = run_tests(SUCC_TESTS);
    let err_num = run_tests(FAIL_TESTS);
    let mut cxl_meminfo = CxlMemInfo::default();
    query_cxl_meminfo(&mut cxl_meminfo);
    println!("CXL MemInfo: {:?}", cxl_meminfo);
    if succ_num == SUCC_TESTS.len() as i32 && err_num == FAIL_TESTS.len() as i32 {
        println!(
            "{} / {} success tests, {} / {} fail tests → PASS",
            succ_num, SUCC_TESTS.len(),
            err_num, FAIL_TESTS.len()
        );
        return 0;
    }
    if succ_num != SUCC_TESTS.len() as i32 {
        println!("  SUCC: expected {} passed {}", SUCC_TESTS.len(), succ_num);
    }
    if err_num != FAIL_TESTS.len() as i32 {
        println!("  FAIL: expected {} passed {}", FAIL_TESTS.len(), err_num);
    }
    println!("Usertests FAILED");
    -1
}
