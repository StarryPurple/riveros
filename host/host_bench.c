/**
 * host_bench — Cross-VM ring buffer benchmark (Host side)
 *
 * Maps the QEMU ivshmem backing file, then reads from / writes to
 * the fixed ring-buffer locations that the rCore guest also accesses.
 *
 * Usage:
 *   1. Start QEMU with ivshmem (make run)
 *   2. In the guest shell, start the "ring_cross" test
 *   3. Run ./host_bench on the host
 *
 * The guest will spin waiting for a message on ring 0 (Host→Guest).
 * This program sends a message, then waits for the echo on ring 1.
 */

#include "ring_adapter.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#define SHM_PATH "../backend-file/cxl.mm"
#define SHM_SIZE (64UL * 1024 * 1024) /* 64 MB */

/* Must match os/src/channel/cross.rs */
#define CROSS_BASE  0x3F00000UL
#define RING_SIZE   0x2000UL
#define CROSS_RING0 CROSS_BASE             /* Host→Guest */
#define CROSS_RING1 (CROSS_BASE + RING_SIZE) /* Guest→Host */

static uint64_t now_us(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000 + (uint64_t)ts.tv_nsec / 1000;
}

int main(int argc, char **argv) {
    int iters = 1000;
    if (argc > 1) iters = atoi(argv[1]);

    /* Map the ivshmem backing file */
    int fd = open(SHM_PATH, O_RDWR);
    if (fd < 0) { perror("open"); return 1; }
    void *base = mmap(NULL, SHM_SIZE, PROT_READ | PROT_WRITE,
                      MAP_SHARED, fd, 0);
    if (base == MAP_FAILED) { perror("mmap"); close(fd); return 1; }
    close(fd);

    ring_header_t *r0 = (ring_header_t *)((uint8_t *)base + CROSS_RING0);
    ring_header_t *r1 = (ring_header_t *)((uint8_t *)base + CROSS_RING1);

    printf("Host rings at: r0=%p  r1=%p\n", (void*)r0, (void*)r1);
    printf("Guest ring capacities: %u  %u\n", r0->capacity, r1->capacity);
    if (r0->capacity == 0) {
        fprintf(stderr, "Guest hasn't initialized rings yet.\n");
        fprintf(stderr, "Start ring_cross in the guest shell first.\n");
        goto out;
    }

    uint8_t msg[64];
    memset(msg, 0xAB, sizeof(msg));

    /* Warm-up: one round trip */
    ring_push_spin(r0, msg, sizeof(msg));
    uint8_t echo[64];
    ring_pop_spin(r1, echo, sizeof(echo));

    uint64_t start = now_us();
    for (int i = 0; i < iters; i++) {
        /* Stamp sequence number into payload */
        msg[0] = (uint8_t)(i & 0xFF);
        msg[1] = (uint8_t)((i >> 8) & 0xFF);

        ring_push_spin(r0, msg, sizeof(msg));
        ring_pop_spin(r1, echo, sizeof(echo));

        if (echo[0] != msg[0] || echo[1] != msg[1]) {
            fprintf(stderr, "Data mismatch at iter %d\n", i);
            goto out;
        }
    }
    uint64_t elapsed = now_us() - start;

    double us_per = (double)elapsed / (iters * 2); /* 2 ops per iter */
    double mbps = (double)(iters * 2 * sizeof(msg)) / elapsed;

    printf("\n=== Cross-VM Ring Benchmark ===\n");
    printf("Iterations: %d, msg: %zu B\n", iters, sizeof(msg));
    printf("Total time: %lu us\n", (unsigned long)elapsed);
    printf("Avg one-way: %.1f us\n", us_per);
    printf("Throughput: %.2f MB/s\n", mbps);

out:
    munmap(base, SHM_SIZE);
    return 0;
}
